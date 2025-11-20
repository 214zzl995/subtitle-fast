use std::cell::UnsafeCell;

use rayon::prelude::*;
use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_validator::subtitle_detection::RoiConfig;

use crate::comparators::SubtitleComparator;
use crate::pipeline::preprocess::extract_masked_patch;
use crate::pipeline::{
    ComparisonReport, FeatureBlob, MaskedPatch, PreprocessSettings, ReportMetric,
};

const TAG: &str = "bitset-cover";
const TOLERANCE_PX: usize = 2;
const MISS_THRESHOLD: f32 = 0.035;
const PARALLEL_MIN_WORDS: usize = 1024;

thread_local! {
    static TLS_SCRATCH: UnsafeCell<BitsetScratch> = UnsafeCell::new(BitsetScratch::new());
}

pub struct BitsetCoverComparator {
    settings: PreprocessSettings,
}

impl BitsetCoverComparator {
    pub fn new(settings: PreprocessSettings) -> Self {
        Self { settings }
    }

    fn build_features(&self, patch: &MaskedPatch) -> Option<BitsetFeatures> {
        BitsetFeatures::from_patch(patch)
    }

    fn compare_features(&self, a: &BitsetFeatures, b: &BitsetFeatures) -> Option<(f32, f32, bool)> {
        if a.width != b.width || a.height != b.height || a.words_per_row != b.words_per_row {
            return None;
        }
        let total_words = a.bits.len();
        if total_words == 0 {
            return Some((1.0, 0.0, false));
        }
        let parallel = should_parallel(total_words);
        let mut similarity = 0.0f32;
        let mut miss_fraction = 1.0f32;
        self.with_scratch(total_words, |scratch| {
            let ScratchSlices {
                tmp_a,
                tmp_b,
                buf_a,
                buf_b,
            } = scratch.slices(total_words);
            dilate_chebyshev_u64(
                &a.bits,
                a.width,
                a.height,
                a.words_per_row,
                a.last_word_mask,
                TOLERANCE_PX,
                tmp_a,
                tmp_b,
                buf_a,
                parallel,
            );
            dilate_chebyshev_u64(
                &b.bits,
                b.width,
                b.height,
                b.words_per_row,
                b.last_word_mask,
                TOLERANCE_PX,
                tmp_a,
                tmp_b,
                buf_b,
                parallel,
            );

            let (miss, union) =
                reduce_miss_union(&a.bits, &b.bits, buf_a, buf_b, parallel, total_words);
            if union == 0 {
                similarity = 1.0;
                miss_fraction = 0.0;
            } else {
                miss_fraction = (miss as f32) / (union as f32);
                similarity = (1.0 - miss_fraction).clamp(0.0, 1.0);
            }
        });
        Some((similarity, miss_fraction, parallel))
    }

    fn with_scratch<F, R>(&self, _len: usize, f: F) -> R
    where
        F: FnOnce(&mut BitsetScratch) -> R,
    {
        TLS_SCRATCH.with(|cell| unsafe { f(&mut *cell.get()) })
    }
}

impl SubtitleComparator for BitsetCoverComparator {
    fn name(&self) -> &'static str {
        TAG
    }

    fn extract(&self, frame: &YPlaneFrame, roi: &RoiConfig) -> Option<FeatureBlob> {
        let patch = extract_masked_patch(frame, roi, self.settings)?;
        let features = self.build_features(&patch)?;
        Some(FeatureBlob::new(TAG, features))
    }

    fn compare(&self, reference: &FeatureBlob, candidate: &FeatureBlob) -> ComparisonReport {
        let Some(reference) = reference.downcast::<BitsetFeatures>(TAG) else {
            return ComparisonReport::new(0.0, false);
        };
        let Some(candidate) = candidate.downcast::<BitsetFeatures>(TAG) else {
            return ComparisonReport::new(0.0, false);
        };
        let reference = reference.as_ref();
        let candidate = candidate.as_ref();
        let Some((similarity, miss_fraction, parallel)) =
            self.compare_features(reference, candidate)
        else {
            return ComparisonReport::new(0.0, false);
        };
        let same = miss_fraction <= MISS_THRESHOLD;
        ComparisonReport::with_details(
            similarity,
            same,
            vec![
                ReportMetric::new("miss_fraction", miss_fraction),
                ReportMetric::new("threshold_miss", MISS_THRESHOLD),
                ReportMetric::new("tolerance_px", TOLERANCE_PX as f32),
                ReportMetric::new("parallel_min_words", PARALLEL_MIN_WORDS as f32),
                ReportMetric::new("parallel_used", parallel as i32 as f32),
                ReportMetric::new("parallel_threads", rayon::current_num_threads() as f32),
            ],
        )
    }
}

#[derive(Clone)]
struct BitsetFeatures {
    width: usize,
    height: usize,
    words_per_row: usize,
    last_word_mask: u64,
    bits: Vec<u64>,
}

impl BitsetFeatures {
    fn from_patch(patch: &MaskedPatch) -> Option<Self> {
        if patch.width == 0 || patch.height == 0 {
            return None;
        }
        let width = patch.width;
        let height = patch.height;
        let area = width.checked_mul(height)?;
        let words_per_row = (width + 63) / 64;
        if words_per_row == 0 {
            return None;
        }
        if patch.mask.len() != area {
            return None;
        }
        let total_words = words_per_row.checked_mul(height)?;
        if total_words == 0 {
            return None;
        }
        let last_word_mask = if width % 64 == 0 {
            !0u64
        } else {
            (1u64 << (width % 64)) - 1
        };
        let bits = pack_mask_bits(
            &patch.mask,
            width,
            height,
            words_per_row,
            last_word_mask,
            should_parallel(total_words),
        );
        Some(Self {
            width,
            height,
            words_per_row,
            last_word_mask,
            bits,
        })
    }
}

struct ScratchBuffer {
    vec: Vec<u64>,
}

impl ScratchBuffer {
    fn new() -> Self {
        Self { vec: Vec::new() }
    }

    fn ensure(&mut self, len: usize) -> &mut [u64] {
        if self.vec.len() < len {
            self.vec.resize(len, 0);
        } else {
            self.vec.truncate(len);
        }
        &mut self.vec[..]
    }
}

struct BitsetScratch {
    tmp_a: ScratchBuffer,
    tmp_b: ScratchBuffer,
    buf_a: ScratchBuffer,
    buf_b: ScratchBuffer,
}

impl BitsetScratch {
    fn new() -> Self {
        Self {
            tmp_a: ScratchBuffer::new(),
            tmp_b: ScratchBuffer::new(),
            buf_a: ScratchBuffer::new(),
            buf_b: ScratchBuffer::new(),
        }
    }

    fn slices(&mut self, len: usize) -> ScratchSlices<'_> {
        ScratchSlices {
            tmp_a: self.tmp_a.ensure(len),
            tmp_b: self.tmp_b.ensure(len),
            buf_a: self.buf_a.ensure(len),
            buf_b: self.buf_b.ensure(len),
        }
    }
}

struct ScratchSlices<'a> {
    tmp_a: &'a mut [u64],
    tmp_b: &'a mut [u64],
    buf_a: &'a mut [u64],
    buf_b: &'a mut [u64],
}

fn dilate_chebyshev_u64(
    src: &[u64],
    width: usize,
    height: usize,
    words_per_row: usize,
    last_word_mask: u64,
    iterations: usize,
    tmp_a: &mut [u64],
    tmp_b: &mut [u64],
    out: &mut [u64],
    use_parallel: bool,
) {
    if iterations == 0 {
        out.copy_from_slice(src);
        return;
    }
    if iterations == 1 {
        dilate3x3_u64_once(
            src,
            width,
            height,
            words_per_row,
            last_word_mask,
            tmp_a,
            out,
            use_parallel,
        );
        return;
    }

    dilate3x3_u64_once(
        src,
        width,
        height,
        words_per_row,
        last_word_mask,
        tmp_a,
        tmp_b,
        use_parallel,
    );

    let mut remaining = iterations - 1;
    let mut src_buf: &mut [u64] = tmp_b;
    let mut dst_buf: &mut [u64] = out;
    while remaining > 0 {
        dilate3x3_u64_once(
            src_buf,
            width,
            height,
            words_per_row,
            last_word_mask,
            tmp_a,
            dst_buf,
            use_parallel,
        );
        remaining -= 1;
        if remaining == 0 {
            break;
        }
        std::mem::swap(&mut src_buf, &mut dst_buf);
    }
}

fn dilate3x3_u64_once(
    src: &[u64],
    width: usize,
    height: usize,
    words_per_row: usize,
    last_word_mask: u64,
    tmp: &mut [u64],
    dst: &mut [u64],
    use_parallel: bool,
) {
    if width == 0 || height == 0 || words_per_row == 0 {
        return;
    }
    debug_assert_eq!(src.len(), words_per_row * height);
    debug_assert_eq!(tmp.len(), src.len());
    debug_assert_eq!(dst.len(), src.len());

    // Horizontal 3-neighbour OR into tmp.
    if use_parallel {
        tmp.par_chunks_exact_mut(words_per_row)
            .enumerate()
            .for_each(|(y, tmp_row)| {
                let row_off = y * words_per_row;
                let src_row = &src[row_off..row_off + words_per_row];
                let mut prev = 0u64;
                for (w, slot) in tmp_row.iter_mut().enumerate() {
                    let cur = src_row[w];
                    let next = if w + 1 < words_per_row {
                        src_row[w + 1]
                    } else {
                        0
                    };
                    let left = (cur << 1) | (prev >> 63);
                    let right = (cur >> 1) | (next << 63);
                    *slot = cur | left | right;
                    prev = cur;
                }
                tmp_row[words_per_row - 1] &= last_word_mask;
            });
    } else {
        for y in 0..height {
            let row = y * words_per_row;
            let src_row = &src[row..row + words_per_row];
            let tmp_row = &mut tmp[row..row + words_per_row];
            let mut prev = 0u64;
            for (w, slot) in tmp_row.iter_mut().enumerate() {
                let cur = src_row[w];
                let next = if w + 1 < words_per_row {
                    src_row[w + 1]
                } else {
                    0
                };
                let left = (cur << 1) | (prev >> 63);
                let right = (cur >> 1) | (next << 63);
                *slot = cur | left | right;
                prev = cur;
            }
            tmp_row[words_per_row - 1] &= last_word_mask;
        }
    }

    // Vertical 3-neighbour OR into dst.
    if use_parallel {
        dst.par_chunks_exact_mut(words_per_row)
            .enumerate()
            .for_each(|(y, dst_row)| {
                let row_off = y * words_per_row;
                let cur_row = &tmp[row_off..row_off + words_per_row];
                let prev_row = if y > 0 {
                    Some(&tmp[row_off - words_per_row..row_off])
                } else {
                    None
                };
                let next_row = if y + 1 < height {
                    Some(&tmp[row_off + words_per_row..row_off + 2 * words_per_row])
                } else {
                    None
                };
                for (w, slot) in dst_row.iter_mut().enumerate() {
                    let mut value = cur_row[w];
                    if let Some(prev_row) = prev_row {
                        value |= prev_row[w];
                    }
                    if let Some(next_row) = next_row {
                        value |= next_row[w];
                    }
                    *slot = value;
                }
                dst_row[words_per_row - 1] &= last_word_mask;
            });
    } else {
        for y in 0..height {
            let row = y * words_per_row;
            let cur_row = &tmp[row..row + words_per_row];
            let dst_row = &mut dst[row..row + words_per_row];
            for (w, slot) in dst_row.iter_mut().enumerate() {
                let mut value = cur_row[w];
                if y > 0 {
                    value |= tmp[row + w - words_per_row];
                }
                if y + 1 < height {
                    value |= tmp[row + w + words_per_row];
                }
                *slot = value;
            }
            dst_row[words_per_row - 1] &= last_word_mask;
        }
    }
}

fn reduce_miss_union(
    a_bits: &[u64],
    b_bits: &[u64],
    ad: &[u64],
    bd: &[u64],
    use_parallel: bool,
    total_words: usize,
) -> (usize, usize) {
    if use_parallel {
        (0..total_words)
            .into_par_iter()
            .map(|i| {
                let abit = a_bits[i];
                let bbit = b_bits[i];
                let ad_bit = ad[i];
                let bd_bit = bd[i];
                let miss =
                    (abit & !bd_bit).count_ones() as usize + (bbit & !ad_bit).count_ones() as usize;
                let union = (abit | bbit).count_ones() as usize;
                (miss, union)
            })
            .reduce(
                || (0usize, 0usize),
                |acc, item| (acc.0 + item.0, acc.1 + item.1),
            )
    } else {
        let mut miss = 0usize;
        let mut union = 0usize;
        for i in 0..total_words {
            let abit = a_bits[i];
            let bbit = b_bits[i];
            let ad_bit = ad[i];
            let bd_bit = bd[i];
            miss += (abit & !bd_bit).count_ones() as usize;
            miss += (bbit & !ad_bit).count_ones() as usize;
            union += (abit | bbit).count_ones() as usize;
        }
        (miss, union)
    }
}

fn should_parallel(total_words: usize) -> bool {
    total_words >= PARALLEL_MIN_WORDS && rayon::current_num_threads() > 1
}

fn pack_mask_bits(
    mask: &[f32],
    width: usize,
    height: usize,
    words_per_row: usize,
    last_word_mask: u64,
    use_parallel: bool,
) -> Vec<u64> {
    let mut bits = vec![0u64; words_per_row * height];
    if use_parallel {
        mask.par_chunks_exact(width)
            .zip(bits.par_chunks_mut(words_per_row))
            .for_each(|(mask_row, bits_row)| pack_row(mask_row, bits_row, last_word_mask));
    } else {
        for (mask_row, bits_row) in mask.chunks_exact(width).zip(bits.chunks_mut(words_per_row)) {
            pack_row(mask_row, bits_row, last_word_mask);
        }
    }
    bits
}

fn pack_row(mask_row: &[f32], bits_row: &mut [u64], last_word_mask: u64) {
    let mut dst = 0usize;
    for chunk in mask_row.chunks_exact(64) {
        let mut word = 0u64;
        for (i, &v) in chunk.iter().enumerate() {
            if v >= 0.5 {
                word |= 1u64 << i;
            }
        }
        bits_row[dst] = word;
        dst += 1;
    }
    let rem = mask_row.chunks_exact(64).remainder();
    if !rem.is_empty() {
        let mut word = 0u64;
        for (i, &v) in rem.iter().enumerate() {
            if v >= 0.5 {
                word |= 1u64 << i;
            }
        }
        bits_row[dst] = word & last_word_mask;
    } else if dst > 0 {
        bits_row[dst - 1] &= last_word_mask;
    }
}

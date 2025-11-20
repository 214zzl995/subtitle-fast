use std::cell::UnsafeCell;

use rayon::prelude::*;
use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_validator::subtitle_detection::RoiConfig;

use crate::comparators::SubtitleComparator;
use crate::pipeline::{ComparisonReport, FeatureBlob, PreprocessSettings, ReportMetric};

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

    fn build_features(&self, frame: &YPlaneFrame, roi: &RoiConfig) -> Option<BitsetFeatures> {
        let (x0, y0, x1, y1) = roi_bounds(frame, roi)?;
        let width = x1 - x0;
        let height = y1 - y0;
        let words_per_row = (width + 63) / 64;
        let total_words = words_per_row.checked_mul(height)?;
        if total_words == 0 {
            return None;
        }
        let last_word_mask = if width % 64 == 0 {
            !0u64
        } else {
            (1u64 << (width % 64)) - 1
        };
        let parallel_pack = should_parallel(total_words);
        let bits = pack_mask_bits_fast(
            frame,
            x0,
            y0,
            width,
            height,
            words_per_row,
            last_word_mask,
            &self.settings,
            parallel_pack,
        );
        self.build_features_from_bits(bits, width, height, words_per_row, last_word_mask)
    }

    fn build_features_from_bits(
        &self,
        bits: Vec<u64>,
        width: usize,
        height: usize,
        words_per_row: usize,
        last_word_mask: u64,
    ) -> Option<BitsetFeatures> {
        let total_words = bits.len();
        if total_words == 0 {
            return None;
        }
        let parallel_dilate = should_parallel(total_words);
        let mut dilated = vec![0u64; total_words];
        self.with_scratch(total_words, |scratch| {
            let ScratchSlices { tmp_a, tmp_b } = scratch.slices(total_words);
            dilate_chebyshev_u64(
                &bits,
                width,
                height,
                words_per_row,
                last_word_mask,
                TOLERANCE_PX,
                tmp_a,
                tmp_b,
                dilated.as_mut_slice(),
                parallel_dilate,
            );
        });
        Some(BitsetFeatures {
            width,
            height,
            words_per_row,
            bits,
            dilated,
        })
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
        let (miss, union) = reduce_miss_union(
            &a.bits,
            &b.bits,
            &a.dilated,
            &b.dilated,
            parallel,
            total_words,
        );
        let (similarity, miss_fraction) = if union == 0 {
            (1.0, 0.0)
        } else {
            let miss_fraction = (miss as f32) / (union as f32);
            ((1.0 - miss_fraction).clamp(0.0, 1.0), miss_fraction)
        };
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
        let features = self.build_features(frame, roi)?;
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
    bits: Vec<u64>,
    dilated: Vec<u64>,
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
}

impl BitsetScratch {
    fn new() -> Self {
        Self {
            tmp_a: ScratchBuffer::new(),
            tmp_b: ScratchBuffer::new(),
        }
    }

    fn slices(&mut self, len: usize) -> ScratchSlices<'_> {
        ScratchSlices {
            tmp_a: self.tmp_a.ensure(len),
            tmp_b: self.tmp_b.ensure(len),
        }
    }
}

struct ScratchSlices<'a> {
    tmp_a: &'a mut [u64],
    tmp_b: &'a mut [u64],
}

fn roi_bounds(frame: &YPlaneFrame, roi: &RoiConfig) -> Option<(usize, usize, usize, usize)> {
    let frame_w = frame.width() as usize;
    let frame_h = frame.height() as usize;
    if frame_w == 0 || frame_h == 0 {
        return None;
    }
    let mut x0 = (roi.x.clamp(0.0, 1.0) * frame_w as f32).floor() as isize;
    let mut y0 = (roi.y.clamp(0.0, 1.0) * frame_h as f32).floor() as isize;
    let mut x1 = ((roi.x + roi.width).clamp(0.0, 1.0) * frame_w as f32).ceil() as isize;
    let mut y1 = ((roi.y + roi.height).clamp(0.0, 1.0) * frame_h as f32).ceil() as isize;

    x0 = x0.clamp(0, frame_w as isize - 1);
    y0 = y0.clamp(0, frame_h as isize - 1);
    x1 = x1.clamp(x0 + 1, frame_w as isize);
    y1 = y1.clamp(y0 + 1, frame_h as isize);

    if x1 <= x0 || y1 <= y0 {
        return None;
    }

    Some((x0 as usize, y0 as usize, x1 as usize, y1 as usize))
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

fn pack_mask_bits_fast(
    frame: &YPlaneFrame,
    x0: usize,
    y0: usize,
    width: usize,
    height: usize,
    words_per_row: usize,
    last_word_mask: u64,
    settings: &PreprocessSettings,
    use_parallel: bool,
) -> Vec<u64> {
    let mut bits = vec![0u64; words_per_row * height];
    let stride = frame.stride();
    let data = frame.data();
    let lo = settings.target.saturating_sub(settings.delta.max(1)) as u8;
    let hi = settings
        .target
        .saturating_add(settings.delta.max(1))
        .min(255) as u8;

    if use_parallel {
        bits.par_chunks_mut(words_per_row)
            .enumerate()
            .for_each(|(row_idx, bits_row)| {
                let row_start = (y0 + row_idx) * stride + x0;
                pack_row_bytes(
                    &data[row_start..row_start + width],
                    bits_row,
                    lo,
                    hi,
                    last_word_mask,
                );
            });
    } else {
        for (row_idx, bits_row) in bits.chunks_mut(words_per_row).enumerate() {
            let row_start = (y0 + row_idx) * stride + x0;
            pack_row_bytes(
                &data[row_start..row_start + width],
                bits_row,
                lo,
                hi,
                last_word_mask,
            );
        }
    }
    bits
}

fn pack_row_bytes(row: &[u8], bits_row: &mut [u64], lo: u8, hi: u8, last_word_mask: u64) {
    let mut dst = 0usize;
    for chunk in row.chunks_exact(64) {
        let mut word = 0u64;
        for (i, &v) in chunk.iter().enumerate() {
            if v >= lo && v <= hi {
                word |= 1u64 << i;
            }
        }
        bits_row[dst] = word;
        dst += 1;
    }
    let rem = row.chunks_exact(64).remainder();
    if !rem.is_empty() {
        let mut word = 0u64;
        for (i, &v) in rem.iter().enumerate() {
            if v >= lo && v <= hi {
                word |= 1u64 << i;
            }
        }
        bits_row[dst] = word & last_word_mask;
    } else if dst > 0 {
        bits_row[dst - 1] &= last_word_mask;
    }
}

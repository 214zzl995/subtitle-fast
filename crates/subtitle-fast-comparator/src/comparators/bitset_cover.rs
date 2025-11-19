use std::cell::RefCell;

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

thread_local! {
    static TLS_SCRATCH: RefCell<BitsetScratch> = RefCell::new(BitsetScratch::new());
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

    fn compare_features(&self, a: &BitsetFeatures, b: &BitsetFeatures) -> Option<(f32, f32)> {
        if a.width != b.width || a.height != b.height || a.words_per_row != b.words_per_row {
            return None;
        }
        let total_words = a.bits.len();
        if total_words == 0 {
            return Some((1.0, 0.0));
        }
        let mut similarity = 0.0f32;
        let mut miss_fraction = 1.0f32;
        self.with_scratch(total_words, |scratch| {
            let (tmp_a, tmp_b, dilated_a, dilated_b) = scratch.buffers(total_words);
            dilate_chebyshev_u64(
                &a.bits,
                a.width,
                a.height,
                a.words_per_row,
                a.last_word_mask,
                TOLERANCE_PX,
                tmp_a,
                tmp_b,
                dilated_a,
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
                dilated_b,
            );

            let mut miss = 0usize;
            let mut union = 0usize;
            for i in 0..total_words {
                let abit = a.bits[i];
                let bbit = b.bits[i];
                let ad = dilated_a[i];
                let bd = dilated_b[i];
                miss += (abit & !bd).count_ones() as usize;
                miss += (bbit & !ad).count_ones() as usize;
                union += (abit | bbit).count_ones() as usize;
            }
            if union == 0 {
                similarity = 1.0;
                miss_fraction = 0.0;
            } else {
                miss_fraction = (miss as f32) / (union as f32);
                similarity = (1.0 - miss_fraction).clamp(0.0, 1.0);
            }
        });
        Some((similarity, miss_fraction))
    }

    fn with_scratch<F, R>(&self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut BitsetScratch) -> R,
    {
        TLS_SCRATCH.with(|cell| {
            let mut scratch = cell.borrow_mut();
            scratch.ensure(len);
            f(&mut scratch)
        })
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
        let Some((similarity, miss_fraction)) = self.compare_features(reference, candidate) else {
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
        let mut bits = vec![0u64; total_words];
        for y in 0..height {
            let row_off = y * words_per_row;
            for x in 0..width {
                let idx = y * width + x;
                if patch.mask[idx] >= 0.5 {
                    let word = row_off + x / 64;
                    let bit = x % 64;
                    bits[word] |= 1u64 << bit;
                }
            }
            let last = row_off + words_per_row - 1;
            bits[last] &= last_word_mask;
        }
        Some(Self {
            width,
            height,
            words_per_row,
            last_word_mask,
            bits,
        })
    }
}

struct BitsetScratch {
    tmp_a: Vec<u64>,
    tmp_b: Vec<u64>,
    buf_a: Vec<u64>,
    buf_b: Vec<u64>,
}

impl BitsetScratch {
    fn new() -> Self {
        Self {
            tmp_a: Vec::new(),
            tmp_b: Vec::new(),
            buf_a: Vec::new(),
            buf_b: Vec::new(),
        }
    }

    fn ensure(&mut self, len: usize) {
        if self.tmp_a.len() < len {
            self.tmp_a.resize(len, 0);
        }
        if self.tmp_b.len() < len {
            self.tmp_b.resize(len, 0);
        }
        if self.buf_a.len() < len {
            self.buf_a.resize(len, 0);
        }
        if self.buf_b.len() < len {
            self.buf_b.resize(len, 0);
        }
    }

    fn buffers(&mut self, len: usize) -> (&mut [u64], &mut [u64], &mut [u64], &mut [u64]) {
        (
            &mut self.tmp_a[..len],
            &mut self.tmp_b[..len],
            &mut self.buf_a[..len],
            &mut self.buf_b[..len],
        )
    }
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
) {
    if iterations == 0 {
        out.copy_from_slice(src);
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
    );
    if iterations == 1 {
        out.copy_from_slice(tmp_b);
        return;
    }
    let mut remaining = iterations - 1;
    let mut src_is_scratch = true;
    let mut dst_is_out = true;
    while remaining > 0 {
        if src_is_scratch && dst_is_out {
            dilate3x3_u64_once(
                tmp_b,
                width,
                height,
                words_per_row,
                last_word_mask,
                tmp_a,
                out,
            );
        } else if !src_is_scratch && !dst_is_out {
            dilate3x3_u64_once(
                out,
                width,
                height,
                words_per_row,
                last_word_mask,
                tmp_a,
                tmp_b,
            );
        } else {
            unreachable!("unexpected dilation buffer state");
        }
        remaining -= 1;
        if remaining == 0 {
            if !dst_is_out {
                out.copy_from_slice(tmp_b);
            }
            break;
        }
        src_is_scratch = !src_is_scratch;
        dst_is_out = !dst_is_out;
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
) {
    if width == 0 || height == 0 || words_per_row == 0 {
        return;
    }
    debug_assert_eq!(src.len(), words_per_row * height);
    debug_assert_eq!(tmp.len(), src.len());
    debug_assert_eq!(dst.len(), src.len());

    // Horizontal 3-neighbour OR into tmp.
    for y in 0..height {
        let row = y * words_per_row;
        let mut prev = 0u64;
        for w in 0..words_per_row {
            let cur = src[row + w];
            let next = if w + 1 < words_per_row {
                src[row + w + 1]
            } else {
                0
            };
            let left = (cur << 1) | (prev >> 63);
            let right = (cur >> 1) | (next << 63);
            tmp[row + w] = cur | left | right;
            prev = cur;
        }
        tmp[row + words_per_row - 1] &= last_word_mask;
    }

    // Vertical 3-neighbour OR into dst.
    for y in 0..height {
        let row = y * words_per_row;
        for w in 0..words_per_row {
            let mut value = tmp[row + w];
            if y > 0 {
                value |= tmp[row + w - words_per_row];
            }
            if y + 1 < height {
                value |= tmp[row + w + words_per_row];
            }
            dst[row + w] = value;
        }
        dst[row + words_per_row - 1] &= last_word_mask;
    }
}

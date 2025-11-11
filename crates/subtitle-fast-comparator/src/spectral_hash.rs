use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_validator::subtitle_detection::RoiConfig;

use crate::SubtitleComparator;
use crate::feature::{ComparisonReport, FeatureBlob, ReportMetric};
use crate::ops::{dct2, resize_average};
use crate::preprocess::{MaskedPatch, PreprocessSettings, extract_masked_patch};

const TAG: &str = "spectral-hash";
const SAMPLE_SIZE: usize = 32;
const BLOCK_SIZE: usize = 8;
const SIMILARITY_THRESHOLD: f32 = 0.85;

#[derive(Clone)]
struct SpectralHashFeatures {
    hash: u64,
}

pub struct SpectralHashComparator {
    settings: PreprocessSettings,
}

impl SpectralHashComparator {
    pub fn new(settings: PreprocessSettings) -> Self {
        Self { settings }
    }

    fn compute_hash(&self, patch: &MaskedPatch) -> u64 {
        let resized = resize_average(
            &patch.masked,
            patch.width,
            patch.height,
            SAMPLE_SIZE,
            SAMPLE_SIZE,
        );
        let spectrum = dct2(&resized, SAMPLE_SIZE, SAMPLE_SIZE);
        let mut bits: [f32; BLOCK_SIZE * BLOCK_SIZE] = [0.0; BLOCK_SIZE * BLOCK_SIZE];
        for by in 0..BLOCK_SIZE {
            for bx in 0..BLOCK_SIZE {
                bits[by * BLOCK_SIZE + bx] = spectrum[by * SAMPLE_SIZE + bx];
            }
        }
        let mean: f32 = bits.iter().skip(1).copied().sum::<f32>() / (bits.len() as f32 - 1.0);
        let mut hash = 0u64;
        for (idx, value) in bits.iter().enumerate() {
            if idx == 0 {
                continue;
            }
            if *value > mean {
                hash |= 1u64 << idx;
            }
        }
        hash
    }
}

impl SubtitleComparator for SpectralHashComparator {
    fn name(&self) -> &'static str {
        "spectral-hash"
    }

    fn extract(&self, frame: &YPlaneFrame, roi: &RoiConfig) -> Option<FeatureBlob> {
        let patch = extract_masked_patch(frame, roi, self.settings)?;
        if patch.len() < BLOCK_SIZE * BLOCK_SIZE {
            return None;
        }
        let hash = self.compute_hash(&patch);
        Some(FeatureBlob::new(TAG, SpectralHashFeatures { hash }))
    }

    fn compare(&self, reference: &FeatureBlob, candidate: &FeatureBlob) -> ComparisonReport {
        let Some(reference) = reference.downcast::<SpectralHashFeatures>(TAG) else {
            return ComparisonReport::new(0.0, false);
        };
        let Some(candidate) = candidate.downcast::<SpectralHashFeatures>(TAG) else {
            return ComparisonReport::new(0.0, false);
        };
        let distance = (reference.hash ^ candidate.hash).count_ones() as f32;
        let similarity = 1.0 - (distance / 64.0);
        let same = similarity >= SIMILARITY_THRESHOLD;
        ComparisonReport::with_details(
            similarity,
            same,
            vec![
                ReportMetric::new("hamming", distance),
                ReportMetric::new("threshold", SIMILARITY_THRESHOLD),
            ],
        )
    }
}

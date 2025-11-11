use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_validator::subtitle_detection::RoiConfig;

use crate::SubtitleComparator;
use crate::feature::{ComparisonReport, FeatureBlob, ReportMetric};
use crate::ops::resize_average;
use crate::preprocess::{MaskedPatch, PreprocessSettings, extract_masked_patch};

const TAG: &str = "structural-dssim";
const MAX_DIM: usize = 64;
const SIMILARITY_THRESHOLD: f32 = 0.90;

#[derive(Clone)]
struct StructuralFeatures {
    pixels: Vec<f32>,
}

pub struct StructuralDssimComparator {
    settings: PreprocessSettings,
}

impl StructuralDssimComparator {
    pub fn new(settings: PreprocessSettings) -> Self {
        Self { settings }
    }

    fn normalize_patch(&self, patch: &MaskedPatch) -> StructuralFeatures {
        if patch.width <= MAX_DIM && patch.height <= MAX_DIM {
            return StructuralFeatures {
                pixels: patch.masked.clone(),
            };
        }
        let (new_w, new_h) = if patch.width >= patch.height {
            (
                MAX_DIM,
                ((patch.height as f32 / patch.width as f32) * MAX_DIM as f32).ceil() as usize,
            )
        } else {
            (
                ((patch.width as f32 / patch.height as f32) * MAX_DIM as f32).ceil() as usize,
                MAX_DIM,
            )
        };
        let clamped_w = new_w.max(1);
        let clamped_h = new_h.max(1);
        StructuralFeatures {
            pixels: resize_average(
                &patch.masked,
                patch.width,
                patch.height,
                clamped_w,
                clamped_h,
            ),
        }
    }

    fn compute_similarity(&self, a: &StructuralFeatures, b: &StructuralFeatures) -> (f32, f32) {
        if a.pixels.is_empty() || b.pixels.is_empty() {
            return (0.0, 0.0);
        }
        let len = a.pixels.len().min(b.pixels.len());
        let mut sum_x = 0.0;
        let mut sum_y = 0.0;
        for idx in 0..len {
            sum_x += a.pixels[idx];
            sum_y += b.pixels[idx];
        }
        let mean_x = sum_x / len as f32;
        let mean_y = sum_y / len as f32;
        let mut var_x = 0.0;
        let mut var_y = 0.0;
        let mut cov = 0.0;
        for idx in 0..len {
            let dx = a.pixels[idx] - mean_x;
            let dy = b.pixels[idx] - mean_y;
            var_x += dx * dx;
            var_y += dy * dy;
            cov += dx * dy;
        }
        let denom = len.saturating_sub(1).max(1) as f32;
        var_x /= denom;
        var_y /= denom;
        cov /= denom;
        let c1 = 0.01f32 * 0.01;
        let c2 = 0.03f32 * 0.03;
        let numerator = (2.0 * mean_x * mean_y + c1) * (2.0 * cov + c2);
        let denominator = (mean_x * mean_x + mean_y * mean_y + c1) * (var_x + var_y + c2);
        if denominator <= f32::EPSILON {
            return (0.0, mean_x - mean_y);
        }
        let ssim = (numerator / denominator).clamp(-1.0, 1.0);
        let similarity = ((ssim + 1.0) * 0.5).clamp(0.0, 1.0);
        (similarity, mean_x - mean_y)
    }
}

impl SubtitleComparator for StructuralDssimComparator {
    fn name(&self) -> &'static str {
        "structural-dssim"
    }

    fn extract(&self, frame: &YPlaneFrame, roi: &RoiConfig) -> Option<FeatureBlob> {
        let patch = extract_masked_patch(frame, roi, self.settings)?;
        if patch.len() < 4 {
            return None;
        }
        let features = self.normalize_patch(&patch);
        Some(FeatureBlob::new(TAG, features))
    }

    fn compare(&self, reference: &FeatureBlob, candidate: &FeatureBlob) -> ComparisonReport {
        let Some(reference) = reference.downcast::<StructuralFeatures>(TAG) else {
            return ComparisonReport::new(0.0, false);
        };
        let Some(candidate) = candidate.downcast::<StructuralFeatures>(TAG) else {
            return ComparisonReport::new(0.0, false);
        };
        let (similarity, mean_delta) = self.compute_similarity(&reference, &candidate);
        let same = similarity >= SIMILARITY_THRESHOLD;
        ComparisonReport::with_details(
            similarity,
            same,
            vec![
                ReportMetric::new("ssim", similarity),
                ReportMetric::new("mean_delta", mean_delta),
                ReportMetric::new("threshold", SIMILARITY_THRESHOLD),
            ],
        )
    }
}

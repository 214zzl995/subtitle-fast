use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_validator::subtitle_detection::RoiConfig;

use crate::comparators::SubtitleComparator;
use crate::pipeline::ops::{gaussian_blur_3x3, normalize, sobel_magnitude};
use crate::pipeline::preprocess::extract_masked_patch;
use crate::pipeline::{
    ComparisonReport, FeatureBlob, MaskedPatch, PreprocessSettings, ReportMetric,
};

const TAG: &str = "hybrid-mask";
const SHIFT_RADIUS: isize = 3;
const SIMILARITY_THRESHOLD: f32 = 0.78;

#[derive(Clone)]
struct HybridMaskFeatures {
    width: usize,
    height: usize,
    mask: Vec<f32>,
    edges: Vec<f32>,
    pixels: Vec<f32>,
    subtitle_mean: f32,
    background_mean: f32,
}

pub struct HybridMaskComparator {
    settings: PreprocessSettings,
}

impl HybridMaskComparator {
    pub fn new(settings: PreprocessSettings) -> Self {
        Self { settings }
    }

    fn build_mask(&self, patch: &MaskedPatch) -> Vec<f32> {
        let target = self.settings.target_f32();
        let delta = self.settings.delta_f32().max(0.01);
        patch
            .original
            .iter()
            .map(|&value| {
                let distance = (value - target).abs();
                let x = (distance - delta) / (delta + 1e-3);
                1.0 / (1.0 + (x * 6.0).exp())
            })
            .collect()
    }

    fn build_features(&self, patch: &MaskedPatch) -> HybridMaskFeatures {
        let mask = self.build_mask(patch);
        let blurred = gaussian_blur_3x3(&patch.masked, patch.width, patch.height);
        let mut edges = sobel_magnitude(&blurred, patch.width, patch.height);
        normalize(&mut edges);
        let mut masked_pixels = patch.masked.clone();
        for (value, &m) in masked_pixels.iter_mut().zip(mask.iter()) {
            *value *= m;
        }
        let mut subtitle_sum = 0.0;
        let mut subtitle_weight = 0.0;
        let mut background_sum = 0.0;
        let mut background_weight = 0.0;
        for ((&_value, &m), &orig) in masked_pixels
            .iter()
            .zip(mask.iter())
            .zip(patch.original.iter())
        {
            subtitle_sum += orig * m;
            subtitle_weight += m;
            let inv = 1.0 - m;
            background_sum += orig * inv;
            background_weight += inv;
        }
        let subtitle_mean = if subtitle_weight > 0.0 {
            subtitle_sum / subtitle_weight
        } else {
            patch.original.iter().copied().sum::<f32>() / patch.len() as f32
        };
        let background_mean = if background_weight > 0.0 {
            background_sum / background_weight
        } else {
            subtitle_mean
        };
        HybridMaskFeatures {
            width: patch.width,
            height: patch.height,
            mask,
            edges,
            pixels: masked_pixels,
            subtitle_mean,
            background_mean,
        }
    }

    fn fuzzy_iou(
        &self,
        reference: &HybridMaskFeatures,
        candidate: &HybridMaskFeatures,
    ) -> (f32, f32, isize, isize) {
        let mut best_iou = 0.0;
        let mut best_dx = 0;
        let mut best_dy = 0;
        let mut best_intersection = 0.0;
        for dy in -SHIFT_RADIUS..=SHIFT_RADIUS {
            for dx in -SHIFT_RADIUS..=SHIFT_RADIUS {
                let (intersection, union) = overlap_metrics(
                    &reference.mask,
                    &candidate.mask,
                    reference.width,
                    reference.height,
                    dx,
                    dy,
                );
                if union <= f32::EPSILON {
                    continue;
                }
                let iou = intersection / union;
                if iou > best_iou {
                    best_iou = iou;
                    best_dx = dx;
                    best_dy = dy;
                    best_intersection = intersection;
                }
            }
        }
        (best_iou, best_intersection, best_dx, best_dy)
    }

    fn compute_metrics(
        &self,
        reference: &HybridMaskFeatures,
        candidate: &HybridMaskFeatures,
        dx: isize,
        dy: isize,
        mask_iou: f32,
    ) -> (f32, f32, f32) {
        let scale = if candidate.subtitle_mean > 1e-3 {
            (reference.subtitle_mean / candidate.subtitle_mean).clamp(0.5, 2.0)
        } else {
            1.0
        };
        let mut ref_sum = 0.0;
        let mut cand_sum = 0.0;
        let mut ref_sq = 0.0;
        let mut cand_sq = 0.0;
        let mut cross = 0.0;
        let mut count = 0.0;

        let mut edge_dot = 0.0;
        let mut ref_edge_norm = 0.0;
        let mut cand_edge_norm = 0.0;

        for y in 0..reference.height {
            let cy = y as isize + dy;
            if cy < 0 || cy >= reference.height as isize {
                continue;
            }
            for x in 0..reference.width {
                let cx = x as isize + dx;
                if cx < 0 || cx >= reference.width as isize {
                    continue;
                }
                let idx = y * reference.width + x;
                let c_idx = cy as usize * reference.width + cx as usize;
                let ref_value = reference.pixels[idx];
                let cand_value = (candidate.pixels[c_idx] * scale).clamp(0.0, 1.0);
                ref_sum += ref_value;
                cand_sum += cand_value;
                ref_sq += ref_value * ref_value;
                cand_sq += cand_value * cand_value;
                cross += ref_value * cand_value;
                count += 1.0;

                let ref_edge = reference.edges[idx];
                let cand_edge = candidate.edges[c_idx];
                edge_dot += ref_edge * cand_edge;
                ref_edge_norm += ref_edge * ref_edge;
                cand_edge_norm += cand_edge * cand_edge;
            }
        }

        let ssim = if count < 16.0 {
            0.0
        } else {
            let mean_ref = ref_sum / count;
            let mean_cand = cand_sum / count;
            let var_ref = (ref_sq / count) - (mean_ref * mean_ref);
            let var_cand = (cand_sq / count) - (mean_cand * mean_cand);
            let cov = (cross / count) - (mean_ref * mean_cand);
            let c1 = 0.01f32 * 0.01;
            let c2 = 0.03f32 * 0.03;
            let numerator = (2.0 * mean_ref * mean_cand + c1) * (2.0 * cov + c2);
            let denominator =
                (mean_ref * mean_ref + mean_cand * mean_cand + c1) * (var_ref + var_cand + c2);
            if denominator <= f32::EPSILON {
                0.0
            } else {
                ((numerator / denominator) + 1.0) * 0.5
            }
        };

        let edge_similarity = if ref_edge_norm <= f32::EPSILON || cand_edge_norm <= f32::EPSILON {
            0.0
        } else {
            (edge_dot / (ref_edge_norm.sqrt() * cand_edge_norm.sqrt())).clamp(-1.0, 1.0)
        };

        let bg_gap = (reference.background_mean - candidate.background_mean * scale)
            .abs()
            .min(0.25);
        let bg_score = 1.0 - (bg_gap / 0.25);

        let similarity =
            0.45 * mask_iou + 0.25 * ssim + 0.2 * (edge_similarity.max(0.0)) + 0.1 * bg_score;
        (similarity, ssim, edge_similarity)
    }
}

impl SubtitleComparator for HybridMaskComparator {
    fn name(&self) -> &'static str {
        "hybrid-mask"
    }

    fn extract(&self, frame: &YPlaneFrame, roi: &RoiConfig) -> Option<FeatureBlob> {
        let patch = extract_masked_patch(frame, roi, self.settings)?;
        if patch.len() < 16 {
            return None;
        }
        let features = self.build_features(&patch);
        Some(FeatureBlob::new(TAG, features))
    }

    fn compare(&self, reference: &FeatureBlob, candidate: &FeatureBlob) -> ComparisonReport {
        let Some(reference) = reference.downcast::<HybridMaskFeatures>(TAG) else {
            return ComparisonReport::new(0.0, false);
        };
        let Some(candidate) = candidate.downcast::<HybridMaskFeatures>(TAG) else {
            return ComparisonReport::new(0.0, false);
        };
        let (mask_iou, _, dx, dy) = self.fuzzy_iou(&reference, &candidate);
        if mask_iou <= 0.01 {
            return ComparisonReport::with_details(
                0.0,
                false,
                vec![
                    ReportMetric::new("mask_iou", mask_iou),
                    ReportMetric::new("threshold", SIMILARITY_THRESHOLD),
                ],
            );
        }
        let (similarity, ssim, edge_similarity) =
            self.compute_metrics(&reference, &candidate, dx, dy, mask_iou);
        let same = similarity >= SIMILARITY_THRESHOLD;
        let bg_gap = (reference.background_mean - candidate.background_mean).abs();
        ComparisonReport::with_details(
            similarity,
            same,
            vec![
                ReportMetric::new("mask_iou", mask_iou),
                ReportMetric::new("ssim", ssim),
                ReportMetric::new("edge", edge_similarity),
                ReportMetric::new("bg_gap", bg_gap),
                ReportMetric::new("threshold", SIMILARITY_THRESHOLD),
            ],
        )
    }
}

fn overlap_metrics(
    reference: &[f32],
    candidate: &[f32],
    width: usize,
    height: usize,
    dx: isize,
    dy: isize,
) -> (f32, f32) {
    let mut intersection = 0.0;
    let mut union = 0.0;
    for y in 0..height {
        let cy = y as isize + dy;
        if cy < 0 || cy >= height as isize {
            continue;
        }
        for x in 0..width {
            let cx = x as isize + dx;
            if cx < 0 || cx >= width as isize {
                continue;
            }
            let idx = y * width + x;
            let c_idx = cy as usize * width + cx as usize;
            let a = reference[idx];
            let b = candidate[c_idx];
            intersection += a.min(b);
            union += a.max(b);
        }
    }
    (intersection, union.max(f32::EPSILON))
}

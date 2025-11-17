use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_validator::subtitle_detection::RoiConfig;

use crate::comparators::SubtitleComparator;
use crate::pipeline::ops::{
    dilate_binary, distance_transform, erode_binary, gaussian_blur_3x3, normalize, percentile,
    sobel_magnitude,
};
use crate::pipeline::preprocess::extract_masked_patch;
use crate::pipeline::{
    ComparisonReport, FeatureBlob, MaskedPatch, PreprocessSettings, ReportMetric,
};

const TAG: &str = "chamfer-edge";
const SHIFT_RADIUS: isize = 4;
const SIMILARITY_THRESHOLD: f32 = 0.65;
const SIGMA: f32 = 2.2;

#[derive(Clone)]
struct ChamferFeatures {
    width: usize,
    height: usize,
    edges: Vec<u8>,
    distance_map: Vec<f32>,
    edge_count: usize,
    stroke_mean: f32,
    stroke_std: f32,
}

pub struct ChamferEdgeComparator {
    settings: PreprocessSettings,
}

impl ChamferEdgeComparator {
    pub fn new(settings: PreprocessSettings) -> Self {
        Self { settings }
    }

    fn build_mask(&self, patch: &MaskedPatch) -> Vec<u8> {
        patch
            .mask
            .iter()
            .map(|&value| if value >= 0.5 { 1 } else { 0 })
            .collect()
    }

    fn stroke_stats(&self, mask: &[u8], width: usize, height: usize) -> (f32, f32) {
        let mut boundary = Vec::with_capacity(mask.len());
        for &value in mask {
            boundary.push(if value == 0 { 1 } else { 0 });
        }
        let dist = distance_transform(&boundary, width, height);
        let mut samples = Vec::new();
        for (idx, &value) in mask.iter().enumerate() {
            if value > 0 {
                samples.push(dist[idx]);
            }
        }
        if samples.is_empty() {
            return (0.0, 0.0);
        }
        let mean = samples.iter().copied().sum::<f32>() / samples.len() as f32;
        let variance = samples
            .iter()
            .map(|value| {
                let delta = *value - mean;
                delta * delta
            })
            .sum::<f32>()
            / samples.len() as f32;
        (mean, variance.sqrt())
    }

    fn build_features(&self, patch: &MaskedPatch) -> ChamferFeatures {
        let mut mask = self.build_mask(patch);
        if !mask.is_empty() {
            mask = erode_binary(
                &dilate_binary(&mask, patch.width, patch.height, 1),
                patch.width,
                patch.height,
                1,
            );
        }
        let blurred = gaussian_blur_3x3(&patch.original, patch.width, patch.height);
        let mut magnitude = sobel_magnitude(&blurred, patch.width, patch.height);
        normalize(&mut magnitude);
        let threshold = percentile(&magnitude, 0.7).max(0.05);
        let mut edges = vec![0u8; magnitude.len()];
        let mut edge_count = 0usize;
        for idx in 0..magnitude.len() {
            if magnitude[idx] >= threshold && mask[idx] > 0 {
                edges[idx] = 1;
                edge_count += 1;
            }
        }
        let distance_map = distance_transform(&edges, patch.width, patch.height);
        let (stroke_mean, stroke_std) = self.stroke_stats(&mask, patch.width, patch.height);
        ChamferFeatures {
            width: patch.width,
            height: patch.height,
            edges,
            distance_map,
            edge_count,
            stroke_mean,
            stroke_std,
        }
    }

    fn chamfer_search(
        &self,
        reference: &ChamferFeatures,
        candidate: &ChamferFeatures,
    ) -> (f32, f32) {
        let mut best_cost = f32::MAX;
        let mut best_match = 0.0;
        for dy in -SHIFT_RADIUS..=SHIFT_RADIUS {
            for dx in -SHIFT_RADIUS..=SHIFT_RADIUS {
                let mut total_cost = 0.0;
                let mut samples = 0.0;
                let mut matches = 0.0;
                for y in 0..candidate.height {
                    let ry = y as isize + dy;
                    if ry < 0 || ry >= reference.height as isize {
                        continue;
                    }
                    for x in 0..candidate.width {
                        let rx = x as isize + dx;
                        if rx < 0 || rx >= reference.width as isize {
                            continue;
                        }
                        let idx = y * candidate.width + x;
                        if candidate.edges[idx] == 0 {
                            continue;
                        }
                        samples += 1.0;
                        let r_idx = ry as usize * reference.width + rx as usize;
                        let dist = reference.distance_map[r_idx];
                        total_cost += dist;
                        if dist < 1.0 {
                            matches += 1.0;
                        }
                    }
                }
                if samples <= 0.0 {
                    continue;
                }
                let avg_cost = total_cost / samples;
                let match_fraction = matches / samples;
                if avg_cost < best_cost
                    || (avg_cost - best_cost).abs() < 1e-3 && match_fraction > best_match
                {
                    best_cost = avg_cost;
                    best_match = match_fraction;
                }
            }
        }
        if best_cost.is_infinite() {
            (f32::MAX, 0.0)
        } else {
            (best_cost, best_match)
        }
    }
}

impl SubtitleComparator for ChamferEdgeComparator {
    fn name(&self) -> &'static str {
        "edge-chamfer"
    }

    fn extract(&self, frame: &YPlaneFrame, roi: &RoiConfig) -> Option<FeatureBlob> {
        let patch = extract_masked_patch(frame, roi, self.settings)?;
        if patch.len() < 16 {
            return None;
        }
        let features = self.build_features(&patch);
        if features.edge_count == 0 {
            return None;
        }
        Some(FeatureBlob::new(TAG, features))
    }

    fn compare(&self, reference: &FeatureBlob, candidate: &FeatureBlob) -> ComparisonReport {
        let Some(reference) = reference.downcast::<ChamferFeatures>(TAG) else {
            return ComparisonReport::new(0.0, false);
        };
        let Some(candidate) = candidate.downcast::<ChamferFeatures>(TAG) else {
            return ComparisonReport::new(0.0, false);
        };
        let (cost, match_fraction) = self.chamfer_search(&reference, &candidate);
        if !cost.is_finite() {
            return ComparisonReport::new(0.0, false);
        }
        let similarity_base = (-((cost / SIGMA).powi(2))).exp();
        let stroke_delta = (reference.stroke_mean - candidate.stroke_mean).abs();
        let stroke_norm = if reference.stroke_mean <= 1e-3 {
            0.0
        } else {
            (stroke_delta / (reference.stroke_mean + 1e-3)).min(1.0)
        };
        let stroke_std_delta = (reference.stroke_std - candidate.stroke_std).abs();
        let stroke_std_norm = if reference.stroke_std <= 1e-3 {
            0.0
        } else {
            (stroke_std_delta / (reference.stroke_std + 1e-3)).min(1.0)
        };
        let stroke_penalty = (1.0 - 0.5 * stroke_norm - 0.25 * stroke_std_norm).clamp(0.0, 1.0);
        let similarity = similarity_base * (0.5 + 0.5 * match_fraction) * stroke_penalty;
        let same = similarity >= SIMILARITY_THRESHOLD;
        ComparisonReport::with_details(
            similarity,
            same,
            vec![
                ReportMetric::new("chamfer_cost", cost),
                ReportMetric::new("match_fraction", match_fraction),
                ReportMetric::new("stroke_penalty", stroke_penalty),
                ReportMetric::new("threshold", SIMILARITY_THRESHOLD),
            ],
        )
    }
}

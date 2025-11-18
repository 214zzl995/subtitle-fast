use std::cmp::Ordering;

use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_validator::subtitle_detection::RoiConfig;

use crate::comparators::SubtitleComparator;
use crate::pipeline::ops::{distance_transform, percentile, percentile_in_place, sobel_magnitude};
use crate::pipeline::preprocess::extract_masked_patch;
use crate::pipeline::{
    ComparisonReport, FeatureBlob, MaskedPatch, PreprocessSettings, ReportMetric,
};

const TAG: &str = "sparse-chamfer";
const GRID_STEP: usize = 2;
const MAX_POINTS: usize = 800;
const KEEP_QUANTILE: f32 = 0.8;
const CLIP_PX: f32 = 4.0;
const TIGHT_PX: f32 = 1.5;
const SHIFT_RADIUS: isize = 3;
const SIM_THRESHOLD: f32 = 0.60;
const MATCH_THRESHOLD: f32 = 0.55;
const SIGMA_SCALE: f32 = 0.03;

#[derive(Clone)]
struct Point {
    x: usize,
    y: usize,
}

#[derive(Clone)]
struct SparseChamferFeatures {
    width: usize,
    height: usize,
    points: Vec<Point>,
    distance_map: Vec<f32>,
    stroke_width: f32,
    diag: f32,
}

pub struct SparseChamferComparator {
    settings: PreprocessSettings,
}

impl SparseChamferComparator {
    pub fn new(settings: PreprocessSettings) -> Self {
        Self { settings }
    }

    fn build_mask(&self, patch: &MaskedPatch) -> Vec<u8> {
        let base: Vec<u8> = patch
            .mask
            .iter()
            .map(|&value| if value >= 0.5 { 1 } else { 0 })
            .collect();
        if base.is_empty() {
            return base;
        }
        let base_on = base.iter().filter(|&&v| v > 0).count();
        let mut mask = base.clone();
        // Light open then close to connect thin strokes without over-smoothing.
        mask = self.erode(&mask, patch.width, patch.height, 1);
        mask = self.dilate(&mask, patch.width, patch.height, 1);
        mask = self.dilate(&mask, patch.width, patch.height, 1);
        mask = self.erode(&mask, patch.width, patch.height, 1);
        let mask_on = mask.iter().filter(|&&v| v > 0).count();
        // If morphology wipes out or severely shrinks the mask, fall back.
        if mask_on == 0 || mask_on * 10 < base_on {
            return base;
        }
        mask
    }

    fn erode(&self, mask: &[u8], width: usize, height: usize, iterations: usize) -> Vec<u8> {
        let mut current = mask.to_vec();
        let mut next = vec![0u8; mask.len()];
        for _ in 0..iterations {
            for y in 0..height {
                for x in 0..width {
                    let mut value = 1u8;
                    'outer: for ky in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                        for kx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                            if current[ky * width + kx] == 0 {
                                value = 0;
                                break 'outer;
                            }
                        }
                    }
                    next[y * width + x] = value;
                }
            }
            current.copy_from_slice(&next);
        }
        current
    }

    fn dilate(&self, mask: &[u8], width: usize, height: usize, iterations: usize) -> Vec<u8> {
        let mut current = mask.to_vec();
        let mut next = vec![0u8; mask.len()];
        for _ in 0..iterations {
            for y in 0..height {
                for x in 0..width {
                    let mut value = 0u8;
                    'outer: for ky in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                        for kx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                            if current[ky * width + kx] > 0 {
                                value = 1;
                                break 'outer;
                            }
                        }
                    }
                    next[y * width + x] = value;
                }
            }
            current.copy_from_slice(&next);
        }
        current
    }

    fn adaptive_edges(&self, patch: &MaskedPatch, mask: &[u8]) -> (Vec<u8>, usize) {
        let magnitude = sobel_magnitude(&patch.original, patch.width, patch.height);
        debug_assert_eq!(magnitude.len(), mask.len());
        let mut masked_values = Vec::with_capacity(magnitude.len());
        for (idx, &m) in magnitude.iter().enumerate() {
            if mask[idx] > 0 {
                masked_values.push(m);
            }
        }
        let threshold = if !masked_values.is_empty() {
            percentile_in_place(&mut masked_values, 0.7)
        } else {
            percentile(&magnitude, 0.7)
        };
        let mut edges = vec![0u8; magnitude.len()];
        let mut count = 0usize;
        for (idx, &m) in magnitude.iter().enumerate() {
            if m >= threshold && mask[idx] > 0 {
                edges[idx] = 1;
                count += 1;
            }
        }
        (edges, count)
    }

    fn sample_points(&self, edges: &[u8], width: usize, height: usize) -> Vec<Point> {
        let grid_w = (width + GRID_STEP - 1) / GRID_STEP;
        let grid_h = (height + GRID_STEP - 1) / GRID_STEP;
        let max_points = grid_w.saturating_mul(grid_h).min(MAX_POINTS);
        let mut points = Vec::with_capacity(max_points);
        for y in (0..height).step_by(GRID_STEP) {
            for x in (0..width).step_by(GRID_STEP) {
                let idx = y * width + x;
                if edges[idx] > 0 {
                    points.push(Point { x, y });
                }
            }
        }
        if points.len() > MAX_POINTS {
            points.truncate(MAX_POINTS);
        }
        points
    }

    fn skeleton_median_width(
        &self,
        mask: &[u8],
        inv_distance_map: &[f32],
        width: usize,
        height: usize,
    ) -> f32 {
        let mut widths = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                if mask[idx] == 0 {
                    continue;
                }
                let dist = inv_distance_map[idx];
                if dist <= f32::EPSILON {
                    continue;
                }
                let mut is_peak = true;
                for ky in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                    for kx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                        let n_idx = ky * width + kx;
                        if inv_distance_map[n_idx] > dist {
                            is_peak = false;
                            break;
                        }
                    }
                    if !is_peak {
                        break;
                    }
                }
                if is_peak {
                    widths.push(dist * 2.0);
                }
            }
        }
        if widths.is_empty() {
            return 0.0;
        }
        let mid = widths.len() / 2;
        widths.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        widths[mid]
    }

    fn build_features(&self, patch: &MaskedPatch) -> Option<SparseChamferFeatures> {
        let mask = self.build_mask(patch);
        if mask.is_empty() {
            return None;
        }
        let (edges, edge_count) = self.adaptive_edges(patch, &mask);
        if edge_count == 0 {
            return None;
        }
        let points = self.sample_points(&edges, patch.width, patch.height);
        if points.is_empty() {
            return None;
        }
        let distance_map = distance_transform(&edges, patch.width, patch.height);
        let mut inv_mask = vec![0u8; mask.len()];
        for (idx, &m) in mask.iter().enumerate() {
            inv_mask[idx] = if m == 0 { 1 } else { 0 };
        }
        let inv_distance_map = distance_transform(&inv_mask, patch.width, patch.height);
        let stroke_width =
            self.skeleton_median_width(&mask, &inv_distance_map, patch.width, patch.height);
        let diag = ((patch.width * patch.width + patch.height * patch.height) as f32).sqrt();
        Some(SparseChamferFeatures {
            width: patch.width,
            height: patch.height,
            points,
            distance_map,
            stroke_width,
            diag,
        })
    }

    fn one_way_partial_chamfer(
        &self,
        points: &[Point],
        target_dt: &[f32],
        target_width: usize,
        target_height: usize,
        dx: isize,
        dy: isize,
        distances: &mut Vec<f32>,
    ) -> (f32, f32) {
        distances.clear();
        let mut tight = 0usize;
        for point in points {
            let tx = point.x as isize + dx;
            let ty = point.y as isize + dy;
            if tx < 0 || ty < 0 || tx >= target_width as isize || ty >= target_height as isize {
                continue;
            }
            let idx = ty as usize * target_width + tx as usize;
            let mut dist = target_dt[idx];
            if dist.is_infinite() {
                continue;
            }
            if dist > CLIP_PX {
                dist = CLIP_PX;
            }
            if dist <= TIGHT_PX {
                tight += 1;
            }
            distances.push(dist);
        }
        if distances.is_empty() {
            return (f32::INFINITY, 0.0);
        }
        let total = distances.len();
        let keep = ((total as f32 * KEEP_QUANTILE).round() as usize).max(1);
        distances.select_nth_unstable_by(keep.min(total - 1), |a, b| {
            a.partial_cmp(b).unwrap_or(Ordering::Equal)
        });
        distances.truncate(keep);
        let sum: f32 = distances.iter().copied().sum();
        let mean = sum / keep as f32;
        let match_fraction = tight as f32 / total as f32;
        (mean, match_fraction)
    }

    fn search_best_shift(
        &self,
        a: &SparseChamferFeatures,
        b: &SparseChamferFeatures,
    ) -> (f32, f32, isize, isize) {
        let mut best_cost = f32::INFINITY;
        let mut best_match = 0.0;
        let mut best_dx = 0isize;
        let mut best_dy = 0isize;
        let mut distances_ab = Vec::with_capacity(a.points.len());
        let mut distances_ba = Vec::with_capacity(b.points.len());
        for dy in -SHIFT_RADIUS..=SHIFT_RADIUS {
            for dx in -SHIFT_RADIUS..=SHIFT_RADIUS {
                let (cost_ab, match_ab) = self.one_way_partial_chamfer(
                    &a.points,
                    &b.distance_map,
                    b.width,
                    b.height,
                    dx,
                    dy,
                    &mut distances_ab,
                );
                let (cost_ba, match_ba) = self.one_way_partial_chamfer(
                    &b.points,
                    &a.distance_map,
                    a.width,
                    a.height,
                    -dx,
                    -dy,
                    &mut distances_ba,
                );
                if !cost_ab.is_finite() || !cost_ba.is_finite() {
                    continue;
                }
                let avg_cost = 0.5 * (cost_ab + cost_ba);
                let avg_match = 0.5 * (match_ab + match_ba);
                if avg_cost < best_cost
                    || (avg_cost - best_cost).abs() < 1e-5 && avg_match > best_match
                {
                    best_cost = avg_cost;
                    best_match = avg_match;
                    best_dx = dx;
                    best_dy = dy;
                }
            }
        }
        (best_cost, best_match, best_dx, best_dy)
    }
}

impl SubtitleComparator for SparseChamferComparator {
    fn name(&self) -> &'static str {
        TAG
    }

    fn extract(&self, frame: &YPlaneFrame, roi: &RoiConfig) -> Option<FeatureBlob> {
        let patch = extract_masked_patch(frame, roi, self.settings)?;
        if patch.len() < 16 {
            return None;
        }
        let features = self.build_features(&patch)?;
        Some(FeatureBlob::new(TAG, features))
    }

    fn compare(&self, reference: &FeatureBlob, candidate: &FeatureBlob) -> ComparisonReport {
        let Some(reference) = reference.downcast::<SparseChamferFeatures>(TAG) else {
            return ComparisonReport::new(0.0, false);
        };
        let Some(candidate) = candidate.downcast::<SparseChamferFeatures>(TAG) else {
            return ComparisonReport::new(0.0, false);
        };
        let (cost, match_fraction, dx, dy) = self.search_best_shift(&reference, &candidate);
        if !cost.is_finite() {
            return ComparisonReport::new(0.0, false);
        }
        let diag = 0.5 * (reference.diag + candidate.diag);
        let sigma = (SIGMA_SCALE * diag).max(1e-3);
        let core_similarity = (-((cost / sigma).powi(2))).exp();
        let stroke_delta = (reference.stroke_width - candidate.stroke_width).abs();
        let stroke_penalty = (-(stroke_delta / 2.0).powi(2)).exp();
        let similarity = core_similarity * stroke_penalty;
        let same = similarity >= SIM_THRESHOLD && match_fraction >= MATCH_THRESHOLD;
        ComparisonReport::with_details(
            similarity,
            same,
            vec![
                ReportMetric::new("best_cost_px", cost),
                ReportMetric::new("match_fraction", match_fraction),
                ReportMetric::new("stroke_penalty", stroke_penalty),
                ReportMetric::new("shift_dx", dx as f32),
                ReportMetric::new("shift_dy", dy as f32),
                ReportMetric::new("threshold_similarity", SIM_THRESHOLD),
                ReportMetric::new("threshold_match", MATCH_THRESHOLD),
            ],
        )
    }
}

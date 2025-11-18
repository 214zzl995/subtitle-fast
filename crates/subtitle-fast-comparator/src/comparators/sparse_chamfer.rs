use rayon::prelude::*;
use std::cmp::Ordering;
use std::sync::Mutex;

use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_validator::subtitle_detection::RoiConfig;

use crate::comparators::SubtitleComparator;
use crate::pipeline::ops::sobel_magnitude;
use crate::pipeline::preprocess::extract_masked_patch;
use crate::pipeline::{
    ComparisonReport, FeatureBlob, MaskedPatch, PreprocessSettings, ReportMetric,
};

const TAG: &str = "sparse-chamfer";
const GRID_STEP: usize = 3;
const MAX_POINTS: usize = 400;
const KEEP_QUANTILE: f32 = 0.7;
const CLIP_PX: f32 = 4.0;
const TIGHT_PX: f32 = 1.5;
const SHIFT_RADIUS: isize = 2;
const SIM_THRESHOLD: f32 = 0.60;
const MATCH_THRESHOLD: f32 = 0.55;
const SIGMA_SCALE: f32 = 0.03;
const STROKE_SIGMA: f32 = 1.8;
const PARALLEL_MIN_POINTS: usize = 256;

#[inline]
fn dilate3x3_bin(src: &[u8], width: usize, height: usize, tmp: &mut [u8], dst: &mut [u8]) {
    debug_assert_eq!(src.len(), width * height);
    debug_assert_eq!(tmp.len(), src.len());
    debug_assert_eq!(dst.len(), src.len());
    // Horizontal OR3 into tmp.
    for y in 0..height {
        let row = y * width;
        if width == 0 {
            continue;
        }
        // Left edge.
        if width == 1 {
            tmp[row] = src[row];
        } else {
            tmp[row] = src[row] | src[row + 1];
            // Middle.
            for x in 1..width - 1 {
                let i = row + x;
                tmp[i] = (src[i - 1] | src[i]) | src[i + 1];
            }
            // Right edge.
            tmp[row + width - 1] = src[row + width - 2] | src[row + width - 1];
        }
    }
    // Vertical OR3 into dst.
    for x in 0..width {
        if height == 0 {
            continue;
        }
        // Top row.
        if height == 1 {
            let i = x;
            dst[i] = tmp[i];
        } else {
            let top = x;
            let below = x + width;
            dst[top] = tmp[top] | tmp[below];
            // Middle rows.
            for y in 1..height - 1 {
                let i = y * width + x;
                dst[i] = (tmp[i - width] | tmp[i]) | tmp[i + width];
            }
            // Bottom row.
            let i = (height - 1) * width + x;
            dst[i] = tmp[i] | tmp[i - width];
        }
    }
}

#[inline]
fn erode3x3_bin(src: &[u8], width: usize, height: usize, tmp: &mut [u8], dst: &mut [u8]) {
    debug_assert_eq!(src.len(), width * height);
    debug_assert_eq!(tmp.len(), src.len());
    debug_assert_eq!(dst.len(), src.len());
    // Horizontal AND3 into tmp.
    for y in 0..height {
        let row = y * width;
        if width == 0 {
            continue;
        }
        if width == 1 {
            tmp[row] = src[row];
        } else {
            tmp[row] = src[row] & src[row + 1];
            for x in 1..width - 1 {
                let i = row + x;
                tmp[i] = (src[i - 1] & src[i]) & src[i + 1];
            }
            tmp[row + width - 1] = src[row + width - 2] & src[row + width - 1];
        }
    }
    // Vertical AND3 into dst.
    for x in 0..width {
        if height == 0 {
            continue;
        }
        if height == 1 {
            let i = x;
            dst[i] = tmp[i];
        } else {
            let top = x;
            let below = x + width;
            dst[top] = tmp[top] & tmp[below];
            for y in 1..height - 1 {
                let i = y * width + x;
                dst[i] = (tmp[i - width] & tmp[i]) & tmp[i + width];
            }
            let i = (height - 1) * width + x;
            dst[i] = tmp[i] & tmp[i - width];
        }
    }
}

fn percentile70_histogram(magnitude: &[f32], mask: Option<&[u8]>, hist: &mut [u32; 256]) -> f32 {
    const BINS: usize = 256;
    if magnitude.is_empty() {
        return 0.0;
    }
    let mut vmax = 0.0f32;
    let mut count: u32 = 0;
    match mask {
        Some(mask) => {
            debug_assert_eq!(mask.len(), magnitude.len());
            for (idx, &m) in magnitude.iter().enumerate() {
                if mask[idx] == 0 {
                    continue;
                }
                if m > 0.0 {
                    if m > vmax {
                        vmax = m;
                    }
                    count = count.saturating_add(1);
                }
            }
        }
        None => {
            for &m in magnitude {
                if m > 0.0 {
                    if m > vmax {
                        vmax = m;
                    }
                    count = count.saturating_add(1);
                }
            }
        }
    }
    if count == 0 {
        return 0.0;
    }
    if vmax <= f32::EPSILON {
        return vmax;
    }
    let scale = (BINS as f32 - 1.0) / vmax;
    for h in hist.iter_mut() {
        *h = 0;
    }
    match mask {
        Some(mask) => {
            for (idx, &m) in magnitude.iter().enumerate() {
                if mask[idx] == 0 || m <= 0.0 {
                    continue;
                }
                let mut bin = (m * scale) as usize;
                if bin >= BINS {
                    bin = BINS - 1;
                }
                hist[bin] = hist[bin].saturating_add(1);
            }
        }
        None => {
            for &m in magnitude {
                if m <= 0.0 {
                    continue;
                }
                let mut bin = (m * scale) as usize;
                if bin >= BINS {
                    bin = BINS - 1;
                }
                hist[bin] = hist[bin].saturating_add(1);
            }
        }
    }
    let target = (0.7 * count as f32).round().clamp(1.0, count as f32) as u32;
    let mut acc: u32 = 0;
    for (i, &c) in hist.iter().enumerate() {
        acc = acc.saturating_add(c);
        if acc >= target {
            return (i as f32) / scale;
        }
    }
    vmax
}

#[inline]
fn dt_chamfer3x4_clipped(
    mask_ones_is_fg: &[u8],
    width: usize,
    height: usize,
    clip_px: f32,
    scratch: &mut Scratch,
) -> Vec<f32> {
    let len = width.saturating_mul(height);
    if len == 0 || mask_ones_is_fg.is_empty() {
        return Vec::new();
    }

    let inf = u16::MAX / 4;
    let clip_units = ((clip_px * 3.0).ceil() as u16).min(inf);
    scratch.ensure_dt_capacity(len);
    let d = &mut scratch.dt_u16[..len];
    for i in 0..len {
        d[i] = if mask_ones_is_fg[i] > 0 {
            0
        } else {
            clip_units
        };
    }

    // 前向扫描
    for y in 0..height {
        let row = y * width;
        for x in 0..width {
            let i = row + x;
            let mut v = d[i];
            if x > 0 {
                v = v.min(d[i - 1].saturating_add(3));
            }
            if y > 0 {
                v = v.min(d[i - width].saturating_add(3));
            }
            if x > 0 && y > 0 {
                v = v.min(d[i - width - 1].saturating_add(4));
            }
            if x + 1 < width && y > 0 {
                v = v.min(d[i - width + 1].saturating_add(4));
            }
            d[i] = v.min(clip_units);
        }
    }

    // 反向扫描
    for y in (0..height).rev() {
        let row = y * width;
        for x in (0..width).rev() {
            let i = row + x;
            let mut v = d[i];
            if x + 1 < width {
                v = v.min(d[i + 1].saturating_add(3));
            }
            if y + 1 < height {
                v = v.min(d[i + width].saturating_add(3));
            }
            if x + 1 < width && y + 1 < height {
                v = v.min(d[i + width + 1].saturating_add(4));
            }
            if x > 0 && y + 1 < height {
                v = v.min(d[i + width - 1].saturating_add(4));
            }
            d[i] = v.min(clip_units);
        }
    }

    let inv3 = 1.0 / 3.0;
    let mut out = Vec::with_capacity(len);
    for &v in d.iter().take(len) {
        out.push((v.min(clip_units) as f32) * inv3);
    }
    out
}

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

struct Scratch {
    tmp_a: Vec<u8>,
    tmp_b: Vec<u8>,
    hist: [u32; 256],
    distances_ab: Vec<f32>,
    distances_ba: Vec<f32>,
    dt_u16: Vec<u16>,
}

impl Scratch {
    fn new() -> Self {
        Self {
            tmp_a: Vec::new(),
            tmp_b: Vec::new(),
            hist: [0; 256],
            distances_ab: Vec::new(),
            distances_ba: Vec::new(),
            dt_u16: Vec::new(),
        }
    }

    fn ensure_mask_capacity(&mut self, len: usize) {
        if self.tmp_a.len() < len {
            self.tmp_a.resize(len, 0);
        }
        if self.tmp_b.len() < len {
            self.tmp_b.resize(len, 0);
        }
    }

    fn ensure_distance_capacity(&mut self, len_ab: usize, len_ba: usize) {
        if self.distances_ab.capacity() < len_ab {
            self.distances_ab = Vec::with_capacity(len_ab);
        }
        if self.distances_ba.capacity() < len_ba {
            self.distances_ba = Vec::with_capacity(len_ba);
        }
    }

    fn ensure_dt_capacity(&mut self, len: usize) {
        if self.dt_u16.len() < len {
            self.dt_u16.resize(len, u16::MAX / 4);
        }
    }
}

pub struct SparseChamferComparator {
    settings: PreprocessSettings,
    scratch: Mutex<Scratch>,
}

impl SparseChamferComparator {
    pub fn new(settings: PreprocessSettings) -> Self {
        Self {
            settings,
            scratch: Mutex::new(Scratch::new()),
        }
    }

    fn build_mask(&self, patch: &MaskedPatch, scratch: &mut Scratch) -> Vec<u8> {
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
        scratch.ensure_mask_capacity(mask.len());
        let tmp_a = &mut scratch.tmp_a[..mask.len()];
        let tmp_b = &mut scratch.tmp_b[..mask.len()];

        // Light open then close to connect thin strokes without over-smoothing:
        // erode -> dilate -> dilate -> erode, with an early stop for the second dilate.
        erode3x3_bin(&mask, patch.width, patch.height, tmp_a, tmp_b);
        mask.copy_from_slice(tmp_b);

        dilate3x3_bin(&mask, patch.width, patch.height, tmp_a, tmp_b);
        let changed_after_first_dilate = tmp_b != mask;
        mask.copy_from_slice(tmp_b);

        if changed_after_first_dilate {
            dilate3x3_bin(&mask, patch.width, patch.height, tmp_a, tmp_b);
            mask.copy_from_slice(tmp_b);
        }

        erode3x3_bin(&mask, patch.width, patch.height, tmp_a, tmp_b);
        mask.copy_from_slice(tmp_b);

        let mask_on = mask.iter().filter(|&&v| v > 0).count();
        // If morphology wipes out or severely shrinks the mask, fall back.
        if mask_on == 0 || mask_on * 10 < base_on {
            return base;
        }
        mask
    }

    fn adaptive_edges(
        &self,
        patch: &MaskedPatch,
        mask: &[u8],
        scratch: &mut Scratch,
    ) -> (Vec<u8>, usize) {
        let magnitude = sobel_magnitude(&patch.original, patch.width, patch.height);
        debug_assert_eq!(magnitude.len(), mask.len());
        let has_masked = mask.iter().any(|&v| v > 0);
        let threshold = if has_masked {
            percentile70_histogram(&magnitude, Some(mask), &mut scratch.hist)
        } else {
            percentile70_histogram(&magnitude, None, &mut scratch.hist)
        };
        let mut edges = vec![0u8; magnitude.len()];
        let mut count = 0usize;
        for (idx, &m) in magnitude.iter().enumerate() {
            if m >= threshold && mask[idx] > 0 {
                edges[idx] = 1;
                count += 1;
            } else {
                edges[idx] = 0;
            }
        }
        (edges, count)
    }

    fn sample_points_step(
        &self,
        edges: &[u8],
        width: usize,
        height: usize,
        step: usize,
    ) -> Vec<Point> {
        if step == 0 {
            return Vec::new();
        }
        let grid_w = (width + step - 1) / step;
        let grid_h = (height + step - 1) / step;
        let max_points = grid_w.saturating_mul(grid_h).min(MAX_POINTS);
        let mut points = Vec::with_capacity(max_points);
        for y in (0..height).step_by(step) {
            for x in (0..width).step_by(step) {
                let idx = y * width + x;
                if edges[idx] > 0 {
                    points.push(Point { x, y });
                    if points.len() == MAX_POINTS {
                        return points;
                    }
                }
            }
        }
        points
    }

    fn sample_points(&self, edges: &[u8], width: usize, height: usize) -> Vec<Point> {
        let mut points = self.sample_points_step(edges, width, height, GRID_STEP);
        if points.is_empty() && GRID_STEP > 1 {
            points = self.sample_points_step(edges, width, height, 1);
        }
        points
    }

    fn build_features(&self, patch: &MaskedPatch) -> Option<SparseChamferFeatures> {
        let mut scratch = self
            .scratch
            .lock()
            .expect("sparse-chamfer scratch mutex poisoned");
        let mask = self.build_mask(patch, &mut scratch);
        if mask.is_empty() {
            return None;
        }
        let (edges, edge_count) = self.adaptive_edges(patch, &mask, &mut scratch);
        if edge_count == 0 {
            return None;
        }
        let points = self.sample_points(&edges, patch.width, patch.height);
        if points.is_empty() {
            return None;
        }
        let distance_map =
            dt_chamfer3x4_clipped(&edges, patch.width, patch.height, CLIP_PX, &mut scratch);
        let area = mask.iter().map(|&v| v as usize).sum::<usize>();
        let stroke_width = if edge_count > 0 {
            (2.0 * area as f32) / (edge_count as f32)
        } else {
            0.0
        };
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
        let point_count = a.points.len().max(b.points.len());
        if point_count >= PARALLEL_MIN_POINTS {
            self.search_best_shift_parallel(a, b)
        } else {
            self.search_best_shift_sequential(a, b)
        }
    }

    fn search_best_shift_sequential(
        &self,
        a: &SparseChamferFeatures,
        b: &SparseChamferFeatures,
    ) -> (f32, f32, isize, isize) {
        let mut scratch = self
            .scratch
            .lock()
            .expect("sparse-chamfer scratch mutex poisoned");
        let mut best_cost = f32::INFINITY;
        let mut best_match = 0.0;
        let mut best_dx = 0isize;
        let mut best_dy = 0isize;
        scratch.ensure_distance_capacity(a.points.len(), b.points.len());
        for dy in -SHIFT_RADIUS..=SHIFT_RADIUS {
            for dx in -SHIFT_RADIUS..=SHIFT_RADIUS {
                let (cost_ab, match_ab) = self.one_way_partial_chamfer(
                    &a.points,
                    &b.distance_map,
                    b.width,
                    b.height,
                    dx,
                    dy,
                    &mut scratch.distances_ab,
                );
                if best_cost.is_finite() && cost_ab >= 2.0 * best_cost {
                    continue;
                }
                let (cost_ba, match_ba) = self.one_way_partial_chamfer(
                    &b.points,
                    &a.distance_map,
                    a.width,
                    a.height,
                    -dx,
                    -dy,
                    &mut scratch.distances_ba,
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

    fn search_best_shift_parallel(
        &self,
        a: &SparseChamferFeatures,
        b: &SparseChamferFeatures,
    ) -> (f32, f32, isize, isize) {
        let shifts: Vec<(isize, isize)> = (-SHIFT_RADIUS..=SHIFT_RADIUS)
            .flat_map(|dy| (-SHIFT_RADIUS..=SHIFT_RADIUS).map(move |dx| (dx, dy)))
            .collect();

        let (best_cost, best_match, best_dx, best_dy) = shifts
            .par_iter()
            .map(|&(dx, dy)| {
                let mut distances_ab = Vec::with_capacity(a.points.len());
                let mut distances_ba = Vec::with_capacity(b.points.len());
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
                    return (f32::INFINITY, 0.0, dx, dy);
                }
                let avg_cost = 0.5 * (cost_ab + cost_ba);
                let avg_match = 0.5 * (match_ab + match_ba);
                (avg_cost, avg_match, dx, dy)
            })
            .reduce(
                || (f32::INFINITY, 0.0, 0isize, 0isize),
                |best, candidate| {
                    let (best_cost, best_match, best_dx, best_dy) = best;
                    let (cost, m, dx, dy) = candidate;
                    if cost < best_cost || (cost - best_cost).abs() < 1e-5 && m > best_match {
                        (cost, m, dx, dy)
                    } else {
                        (best_cost, best_match, best_dx, best_dy)
                    }
                },
            );

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
        let stroke_penalty = (-(stroke_delta / STROKE_SIGMA).powi(2)).exp();
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

use rayon::prelude::*;
use std::cell::RefCell;
use std::mem;

use subtitle_fast_types::{RoiConfig, YPlaneFrame};

use crate::comparators::SubtitleComparator;
use crate::pipeline::ops::sobel_magnitude_into;
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

thread_local! {
    static TLS_SCRATCH: RefCell<Scratch> = RefCell::new(Scratch::new());
}

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

#[inline(always)]
fn dt_euclidean_clipped(
    mask_ones_is_fg: &[u8],
    width: usize,
    height: usize,
    clip_px: f32,
    scratch: &mut Scratch,
) -> Vec<f32> {
    // --- 基本健壮性 ---
    let len = match width.checked_mul(height) {
        Some(v) if v > 0 && mask_ones_is_fg.len() == v => v,
        _ => return Vec::new(),
    };

    let clip = clip_px.max(0.0);
    let fg_count = mask_ones_is_fg.iter().filter(|&&v| v > 0).count();
    if fg_count == 0 {
        return vec![clip; len];
    }

    // --- 小 ROI / 小半径：强制走 stencil ---
    // 半径（整数像素），向上取整并+1，确保覆盖
    let r: i32 = clip.ceil() as i32;
    let r2: i32 = r.saturating_mul(r);

    // dist2：初始化为 clip^2，避免传播无意义大值；最终会 sqrt 后再 clip。
    scratch.ensure_dt_i32_capacity(len);
    let dist2: &mut [i32] = &mut scratch.dt_i32[..len];
    let clip2_i32: i32 = (clip.ceil() as i32).saturating_mul(clip.ceil() as i32);
    dist2.fill(clip2_i32);

    // --- 预计算表：dx^2, 每个 |dy| 对应的 x 半径 xr(|dy|) ---
    // 这些表都非常小（<= 9~11），可以放栈上；如需零分配可放 Scratch。
    let max_r = r as usize;
    let mut dx2 = [0i32; 64]; // 够用：R 一般 <= 8
    let mut xr_by_dy = [0i32; 64]; // 每个 |dy| 的水平半径
    for (i, value) in dx2.iter_mut().enumerate().take(max_r + 1) {
        let i = i as i32;
        *value = i * i;
    }
    for (ady, xr) in xr_by_dy.iter_mut().enumerate().take(max_r + 1) {
        // xr = floor(sqrt(r^2 - dy^2))
        let rem = r2 - (ady as i32) * (ady as i32);
        *xr = if rem <= 0 {
            0
        } else {
            (rem as f32).sqrt().floor() as i32
        };
    }

    // --- 收集前景像素索引（小 ROI 下这步成本很低） ---
    // 可复用 Scratch 的临时缓冲避免分配：这里直接小 Vec 即可。
    let mut fg_idx = Vec::with_capacity(fg_count.min(2048));
    for (i, &m) in mask_ones_is_fg.iter().enumerate() {
        if m > 0 {
            fg_idx.push(i);
        }
    }

    // --- 核心：对每个前景点，用圆盘 stencil 更新 dist2 ---
    unsafe {
        let w = width as i32;
        let h = height as i32;
        let dptr = dist2.as_mut_ptr();

        for &idx in &fg_idx {
            let x0 = (idx % width) as i32;
            let y0 = (idx / width) as i32;

            // 遍历 dy = -r..=r
            let mut dy = -r;
            while dy <= r {
                let y = y0 + dy;
                if (0..h).contains(&y) {
                    let ady = (if dy < 0 { -dy } else { dy }) as usize;
                    let xr = xr_by_dy[ady];
                    if xr > 0 {
                        // 本行更新区间 [xL, xR]
                        let mut x_l = x0 - xr;
                        let mut x_r = x0 + xr;
                        if x_l < 0 {
                            x_l = 0;
                        }
                        if x_r >= w {
                            x_r = w - 1;
                        }
                        if x_l <= x_r {
                            // 行首指针
                            let row_ptr = dptr.add((y as usize) * width);

                            // 常量部分：dy^2
                            let dy2 = dy * dy;

                            // 从 xL 到 xR 线性扫，利用预计算 dx^2 表
                            let mut x = x_l;
                            while x <= x_r {
                                let adx = (x - x0).unsigned_abs() as usize;
                                let v = dy2 + dx2[adx];
                                let cell = row_ptr.add(x as usize);
                                // cell = min(cell, v)，避免 bounds-check
                                let old = *cell;
                                *cell = if v < old { v } else { old };
                                x += 1;
                            }
                        }
                    } else {
                        // xr == 0，只更新 x0（若在边界内）
                        if (0..w).contains(&x0) {
                            let dy2 = dy * dy;
                            let cell = dptr.add((y as usize) * width + (x0 as usize));
                            let old = *cell;
                            let v = dy2; // dx=0
                            *cell = if v < old { v } else { old };
                        }
                    }
                }
                dy += 1;
            }
        }
    }

    // --- 输出：sqrt + clip（小 ROI 用标量就足够快） ---
    let mut out = vec![0.0f32; len];
    for (o, &d2) in out.iter_mut().zip(dist2.iter()) {
        // d2 已经被限制在 clip^2 内（初始就是 clip^2，更新只会更小）
        *o = (d2 as f32).sqrt();
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
    mask: Vec<u8>,
    edges: Vec<u8>,
    magnitude: Vec<f32>,
    hist: [u32; 256],
    dt_i32: Vec<i32>,
}

impl Scratch {
    fn new() -> Self {
        Self {
            tmp_a: Vec::new(),
            tmp_b: Vec::new(),
            mask: Vec::new(),
            edges: Vec::new(),
            magnitude: Vec::new(),
            hist: [0; 256],
            dt_i32: Vec::new(),
        }
    }

    fn ensure_mask_capacity(&mut self, len: usize) {
        if self.tmp_a.len() < len {
            self.tmp_a.resize(len, 0);
        }
        if self.tmp_b.len() < len {
            self.tmp_b.resize(len, 0);
        }
        if self.mask.len() < len {
            self.mask.resize(len, 0);
        }
        if self.edges.len() < len {
            self.edges.resize(len, 0);
        }
    }

    fn ensure_dt_i32_capacity(&mut self, len: usize) {
        if self.dt_i32.len() < len {
            self.dt_i32.resize(len, 0);
        }
    }
}

pub struct SparseChamferComparator {
    settings: PreprocessSettings,
}

impl SparseChamferComparator {
    pub fn new(settings: PreprocessSettings) -> Self {
        Self { settings }
    }

    fn with_scratch<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Scratch) -> R,
    {
        TLS_SCRATCH.with(|cell| {
            let mut scratch = cell.borrow_mut();
            f(&mut scratch)
        })
    }

    fn build_mask(&self, patch: &MaskedPatch, scratch: &mut Scratch, mask: &mut Vec<u8>) -> bool {
        let len = patch.len();
        if len == 0 {
            mask.clear();
            return false;
        }
        mask.resize(len, 0);
        let mut base_on = 0usize;
        for (dst, &value) in mask.iter_mut().zip(&patch.mask) {
            let bit = if value >= 0.5 { 1 } else { 0 };
            *dst = bit;
            base_on += bit as usize;
        }
        if base_on == 0 {
            return true;
        }
        scratch.ensure_mask_capacity(len);
        let tmp_a = &mut scratch.tmp_a[..len];
        let tmp_b = &mut scratch.tmp_b[..len];

        // Light open then close to connect thin strokes without over-smoothing:
        // erode -> dilate -> dilate -> erode, with an early stop for the second dilate.
        erode3x3_bin(mask, patch.width, patch.height, tmp_a, tmp_b);
        mask.copy_from_slice(tmp_b);

        dilate3x3_bin(mask, patch.width, patch.height, tmp_a, tmp_b);
        let changed_after_first_dilate = tmp_b != mask.as_slice();
        mask.copy_from_slice(tmp_b);

        if changed_after_first_dilate {
            dilate3x3_bin(mask, patch.width, patch.height, tmp_a, tmp_b);
            mask.copy_from_slice(tmp_b);
        }

        erode3x3_bin(mask, patch.width, patch.height, tmp_a, tmp_b);
        mask.copy_from_slice(tmp_b);

        let mask_on = mask.iter().filter(|&&v| v > 0).count();
        if mask_on == 0 || mask_on * 10 < base_on {
            for (dst, &value) in mask.iter_mut().zip(&patch.mask) {
                *dst = if value >= 0.5 { 1 } else { 0 };
            }
        }
        true
    }

    fn adaptive_edges(
        &self,
        patch: &MaskedPatch,
        mask: &[u8],
        scratch: &mut Scratch,
        edges: &mut Vec<u8>,
    ) -> usize {
        sobel_magnitude_into(
            &patch.original,
            patch.width,
            patch.height,
            &mut scratch.magnitude,
        );
        debug_assert_eq!(scratch.magnitude.len(), mask.len());
        let magnitude = &scratch.magnitude;
        let has_masked = mask.iter().any(|&v| v > 0);
        let threshold = if has_masked {
            percentile70_histogram(magnitude, Some(mask), &mut scratch.hist)
        } else {
            percentile70_histogram(magnitude, None, &mut scratch.hist)
        };
        edges.resize(magnitude.len(), 0);
        let mut count = 0usize;
        for (idx, &m) in magnitude.iter().enumerate() {
            if m >= threshold && mask[idx] > 0 {
                edges[idx] = 1;
                count += 1;
            } else {
                edges[idx] = 0;
            }
        }
        count
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
        let grid_w = width.div_ceil(step);
        let grid_h = height.div_ceil(step);
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
        self.with_scratch(|scratch| {
            let mut mask = mem::take(&mut scratch.mask);
            if !self.build_mask(patch, scratch, &mut mask) {
                scratch.mask = mask;
                return None;
            }
            let mut edges = mem::take(&mut scratch.edges);
            let edge_count = self.adaptive_edges(patch, &mask, scratch, &mut edges);
            if edge_count == 0 {
                scratch.mask = mask;
                scratch.edges = edges;
                return None;
            }
            let points = self.sample_points(&edges, patch.width, patch.height);
            if points.is_empty() {
                scratch.mask = mask;
                scratch.edges = edges;
                return None;
            }
            let distance_map: Vec<f32> =
                dt_euclidean_clipped(&edges, patch.width, patch.height, CLIP_PX, scratch);
            let area = mask.iter().map(|&v| v as usize).sum::<usize>();
            let stroke_width = if edge_count > 0 {
                (2.0 * area as f32) / (edge_count as f32)
            } else {
                0.0
            };
            scratch.mask = mask;
            scratch.edges = edges;
            let diag = ((patch.width * patch.width + patch.height * patch.height) as f32).sqrt();
            Some(SparseChamferFeatures {
                width: patch.width,
                height: patch.height,
                points,
                distance_map,
                stroke_width,
                diag,
            })
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
    ) -> (f32, f32) {
        let clip_units: usize = ((CLIP_PX * 3.0).ceil() as usize).min(12);
        let mut bins_cnt = [0usize; 13];
        let mut bins_sum = [0f32; 13];
        let mut tight = 0usize;
        for point in points {
            let tx = point.x as isize + dx;
            let ty = point.y as isize + dy;
            if tx < 0 || ty < 0 || tx >= target_width as isize || ty >= target_height as isize {
                continue;
            }
            let idx = ty as usize * target_width + tx as usize;
            let mut dist = target_dt[idx];
            if !dist.is_finite() {
                continue;
            }
            if dist > CLIP_PX {
                dist = CLIP_PX;
            }
            if dist <= TIGHT_PX {
                tight += 1;
            }
            let bin = ((dist * 3.0) + 0.5).floor() as usize;
            let bucket = bin.min(clip_units);
            bins_cnt[bucket] = bins_cnt[bucket].saturating_add(1);
            bins_sum[bucket] += dist;
        }
        let total: usize = bins_cnt.iter().sum();
        if total == 0 {
            return (f32::INFINITY, 0.0);
        }
        let keep = ((total as f32 * KEEP_QUANTILE).round() as usize).max(1);
        let mut acc = 0usize;
        let mut sum = 0f32;
        for b in 0..=clip_units {
            let count = bins_cnt[b];
            if acc + count < keep {
                sum += bins_sum[b];
                acc += count;
            } else {
                let take = keep - acc;
                if take > 0 {
                    sum += take as f32 * (b as f32 / 3.0);
                }
                break;
            }
        }
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
        self.with_scratch(|_scratch| {
            let mut best_cost = f32::INFINITY;
            let mut best_match = 0.0;
            let mut best_dx = 0isize;
            let mut best_dy = 0isize;
            for dy in -SHIFT_RADIUS..=SHIFT_RADIUS {
                for dx in -SHIFT_RADIUS..=SHIFT_RADIUS {
                    let (cost_ab, match_ab) = self.one_way_partial_chamfer(
                        &a.points,
                        &b.distance_map,
                        b.width,
                        b.height,
                        dx,
                        dy,
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
        })
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
                let (cost_ab, match_ab) = self.one_way_partial_chamfer(
                    &a.points,
                    &b.distance_map,
                    b.width,
                    b.height,
                    dx,
                    dy,
                );
                let (cost_ba, match_ba) = self.one_way_partial_chamfer(
                    &b.points,
                    &a.distance_map,
                    a.width,
                    a.height,
                    -dx,
                    -dy,
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

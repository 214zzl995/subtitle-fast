use std::cmp::{self, Ordering};

#[cfg(target_arch = "aarch64")]
use std::arch::is_aarch64_feature_detected;
#[cfg(target_arch = "x86_64")]
use std::arch::is_x86_feature_detected;

use crate::config::FrameMetadata;

use super::{
    DetectionRegion, LumaBandConfig, RoiConfig, SubtitleDetectionConfig, SubtitleDetectionError,
    SubtitleDetectionResult, SubtitleDetector,
};

const RLSA_H_GAP: usize = 18;
const RLSA_V_GAP: usize = 3;
const MIN_AREA: usize = 400;
const MAX_AREA_RATIO: f32 = 0.35;
const MIN_ASPECT_RATIO: f32 = 2.0;
const VMR_K: usize = 4;
const Y_MERGE_TOL: usize = 10;
const IOU_MERGE: f32 = 0.15;
const NEAR_GAP: usize = 16;
const MAX_OUTPUT_REGIONS: usize = 4;

#[derive(Clone, Copy)]
struct RoiRect {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

pub struct LumaBandDetector {
    config: SubtitleDetectionConfig,
    roi: RoiRect,
    required_len: usize,
}

impl LumaBandDetector {
    pub fn new(config: SubtitleDetectionConfig) -> Result<Self, SubtitleDetectionError> {
        let required_len = required_len(&config)?;
        let roi = compute_roi_rect(config.frame_width, config.frame_height, config.roi)?;
        Ok(Self {
            config,
            roi,
            required_len,
        })
    }
}

impl SubtitleDetector for LumaBandDetector {
    fn ensure_available(config: &SubtitleDetectionConfig) -> Result<(), SubtitleDetectionError> {
        required_len(config)?;
        let _ = compute_roi_rect(config.frame_width, config.frame_height, config.roi)?;
        Ok(())
    }

    fn detect(
        &self,
        y_plane: &[u8],
        _metadata: &FrameMetadata,
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        if y_plane.len() < self.required_len {
            return Err(SubtitleDetectionError::InsufficientData {
                data_len: y_plane.len(),
                required: self.required_len,
            });
        }

        if self.roi.width == 0 || self.roi.height == 0 {
            let result = SubtitleDetectionResult {
                has_subtitle: false,
                max_score: 0.0,
                regions: Vec::new(),
            };
            return Ok(result);
        }

        let mut mask = threshold_mask(y_plane, self.config.stride, self.roi, self.config.luma_band);

        rlsa_horizontal(&mut mask, self.roi.width, self.roi.height, RLSA_H_GAP);
        rlsa_vertical(&mut mask, self.roi.width, self.roi.height, RLSA_V_GAP);

        let components = connected_components(&mask, self.roi.width, self.roi.height);
        if components.is_empty() {
            let result = SubtitleDetectionResult {
                has_subtitle: false,
                max_score: 0.0,
                regions: Vec::new(),
            };
            return Ok(result);
        }

        let integral = integral_image(&mask, self.roi.width, self.roi.height);
        let frame_area = self
            .config
            .frame_width
            .saturating_mul(self.config.frame_height) as f32;
        let max_rect_area = frame_area * MAX_AREA_RATIO;

        let mut candidates = Vec::new();
        for comp in components {
            if comp.area < MIN_AREA {
                continue;
            }
            let width = comp.max_x - comp.min_x + 1;
            let height = comp.max_y - comp.min_y + 1;
            if width == 0 || height == 0 {
                continue;
            }
            let rect_area = (width * height) as f32;
            if rect_area > max_rect_area {
                continue;
            }
            let aspect = width as f32 / height.max(1) as f32;
            if aspect < MIN_ASPECT_RATIO {
                continue;
            }

            let (fill, vmr, score) = evaluate_region(
                &integral,
                self.roi.width,
                comp.min_x,
                comp.min_y,
                width,
                height,
                VMR_K,
            );
            candidates.push(Candidate {
                x: comp.min_x,
                y: comp.min_y,
                width,
                height,
                _fill: fill,
                _vmr: vmr,
                score,
            });
        }

        if candidates.is_empty() {
            let result = SubtitleDetectionResult {
                has_subtitle: false,
                max_score: 0.0,
                regions: Vec::new(),
            };
            return Ok(result);
        }

        let mut merged = merge_line_candidates(candidates, &integral, self.roi.width);
        if merged.is_empty() {
            let result = SubtitleDetectionResult {
                has_subtitle: false,
                max_score: 0.0,
                regions: Vec::new(),
            };
            return Ok(result);
        }

        merged.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));

        let max_score = merged.first().map(|c| c.score).unwrap_or(0.0);

        let mut regions = Vec::new();
        for cand in merged.iter().take(MAX_OUTPUT_REGIONS) {
            regions.push(DetectionRegion {
                x: (cand.x + self.roi.x) as f32,
                y: (cand.y + self.roi.y) as f32,
                width: cand.width as f32,
                height: cand.height as f32,
                score: cand.score,
            });
        }

        let result = SubtitleDetectionResult {
            has_subtitle: !regions.is_empty(),
            max_score,
            regions,
        };
        Ok(result)
    }
}

fn required_len(config: &SubtitleDetectionConfig) -> Result<usize, SubtitleDetectionError> {
    config
        .stride
        .checked_mul(config.frame_height)
        .ok_or_else(|| SubtitleDetectionError::InsufficientData {
            data_len: 0,
            required: usize::MAX,
        })
}

fn compute_roi_rect(
    frame_width: usize,
    frame_height: usize,
    roi: RoiConfig,
) -> Result<RoiRect, SubtitleDetectionError> {
    let start_x = (roi.x * frame_width as f32).round() as isize;
    let start_y = (roi.y * frame_height as f32).round() as isize;
    let end_x = ((roi.x + roi.width) * frame_width as f32).round() as isize;
    let end_y = ((roi.y + roi.height) * frame_height as f32).round() as isize;

    let start_x = start_x.clamp(0, frame_width as isize);
    let start_y = start_y.clamp(0, frame_height as isize);
    let end_x = end_x.clamp(start_x, frame_width as isize);
    let end_y = end_y.clamp(start_y, frame_height as isize);

    let width = (end_x - start_x).max(0) as usize;
    let height = (end_y - start_y).max(0) as usize;
    if width == 0 || height == 0 {
        return Err(SubtitleDetectionError::EmptyRoi);
    }

    Ok(RoiRect {
        x: start_x as usize,
        y: start_y as usize,
        width,
        height,
    })
}

fn threshold_mask(data: &[u8], stride: usize, roi: RoiRect, params: LumaBandConfig) -> Vec<u8> {
    let mut mask = vec![0u8; roi.width * roi.height];
    if mask.is_empty() {
        return mask;
    }

    let lo = params.target_luma.saturating_sub(params.delta);
    let hi = params.target_luma.saturating_add(params.delta);

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("sse2") {
            unsafe {
                threshold_mask_sse2(data, stride, roi, lo, hi, &mut mask);
            }
            return mask;
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if is_aarch64_feature_detected!("neon") {
            unsafe {
                threshold_mask_neon(data, stride, roi, lo, hi, &mut mask);
            }
            return mask;
        }
    }

    threshold_mask_scalar(data, stride, roi, lo, hi, &mut mask);
    mask
}

fn threshold_mask_scalar(
    data: &[u8],
    stride: usize,
    roi: RoiRect,
    lo: u8,
    hi: u8,
    mask: &mut [u8],
) {
    for row in 0..roi.height {
        let src_offset = (roi.y + row) * stride + roi.x;
        let dst_offset = row * roi.width;
        let src = &data[src_offset..src_offset + roi.width];
        let dst = &mut mask[dst_offset..dst_offset + roi.width];
        threshold_mask_scalar_row(src, dst, lo, hi);
    }
}

#[inline(always)]
fn threshold_mask_scalar_row(src: &[u8], dst: &mut [u8], lo: u8, hi: u8) {
    for (value, out) in src.iter().zip(dst.iter_mut()) {
        *out = u8::from(*value >= lo && *value <= hi);
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn threshold_mask_sse2(
    data: &[u8],
    stride: usize,
    roi: RoiRect,
    lo: u8,
    hi: u8,
    mask: &mut [u8],
) {
    use std::arch::x86_64::{
        __m128i, _mm_and_si128, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_max_epu8, _mm_min_epu8,
        _mm_set1_epi8, _mm_storeu_si128,
    };

    let lo_vec = _mm_set1_epi8(lo as i8);
    let hi_vec = _mm_set1_epi8(hi as i8);
    let ones = _mm_set1_epi8(1);
    let width = roi.width;

    for row in 0..roi.height {
        let src_ptr = data.as_ptr().add((roi.y + row) * stride + roi.x);
        let dst_ptr = mask.as_mut_ptr().add(row * width);

        let mut x = 0usize;
        while x + 16 <= width {
            let pixels = _mm_loadu_si128(src_ptr.add(x) as *const __m128i);
            let ge_lo = _mm_cmpeq_epi8(pixels, _mm_max_epu8(pixels, lo_vec));
            let le_hi = _mm_cmpeq_epi8(pixels, _mm_min_epu8(pixels, hi_vec));
            let mut mask_vec = _mm_and_si128(ge_lo, le_hi);
            mask_vec = _mm_and_si128(mask_vec, ones);
            _mm_storeu_si128(dst_ptr.add(x) as *mut __m128i, mask_vec);
            x += 16;
        }

        if x < width {
            let remaining = width - x;
            let src_tail = std::slice::from_raw_parts(src_ptr.add(x), remaining);
            let dst_tail = std::slice::from_raw_parts_mut(dst_ptr.add(x), remaining);
            threshold_mask_scalar_row(src_tail, dst_tail, lo, hi);
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn threshold_mask_neon(
    data: &[u8],
    stride: usize,
    roi: RoiRect,
    lo: u8,
    hi: u8,
    mask: &mut [u8],
) {
    use std::arch::aarch64::{
        uint8x16_t, vandq_u8, vceqq_u8, vdupq_n_u8, vld1q_u8, vmaxq_u8, vminq_u8, vst1q_u8,
    };

    let lo_vec: uint8x16_t = vdupq_n_u8(lo);
    let hi_vec: uint8x16_t = vdupq_n_u8(hi);
    let ones: uint8x16_t = vdupq_n_u8(1);
    let width = roi.width;

    for row in 0..roi.height {
        let src_ptr = data.as_ptr().add((roi.y + row) * stride + roi.x);
        let dst_ptr = mask.as_mut_ptr().add(row * width);

        let mut x = 0usize;
        while x + 16 <= width {
            let pixels = vld1q_u8(src_ptr.add(x));
            let ge_lo = vceqq_u8(pixels, vmaxq_u8(pixels, lo_vec));
            let le_hi = vceqq_u8(pixels, vminq_u8(pixels, hi_vec));
            let mask_vec = vandq_u8(vandq_u8(ge_lo, le_hi), ones);
            vst1q_u8(dst_ptr.add(x), mask_vec);
            x += 16;
        }

        if x < width {
            let remaining = width - x;
            let src_tail = std::slice::from_raw_parts(src_ptr.add(x), remaining);
            let dst_tail = std::slice::from_raw_parts_mut(dst_ptr.add(x), remaining);
            threshold_mask_scalar_row(src_tail, dst_tail, lo, hi);
        }
    }
}

fn rlsa_horizontal(mask: &mut [u8], width: usize, height: usize, gap: usize) {
    if gap == 0 || width == 0 {
        return;
    }
    for y in 0..height {
        let start = y * width;
        let row = &mut mask[start..start + width];
        let mut x = 0usize;
        while x < width {
            if row[x] != 0 {
                x += 1;
                continue;
            }
            let mut span_end = x;
            while span_end < width && row[span_end] == 0 {
                span_end += 1;
            }
            let left_connected = x > 0 && row[x - 1] != 0;
            let right_connected = span_end < width && row[span_end] != 0;
            if left_connected && right_connected && (span_end - x) <= gap {
                for value in &mut row[x..span_end] {
                    *value = 1;
                }
            }
            x = span_end;
        }
    }
}

fn rlsa_vertical(mask: &mut [u8], width: usize, height: usize, gap: usize) {
    if gap == 0 || height == 0 {
        return;
    }
    for x in 0..width {
        let mut y = 0usize;
        while y < height {
            let idx = y * width + x;
            if mask[idx] != 0 {
                y += 1;
                continue;
            }
            let mut span_end = y;
            while span_end < height && mask[span_end * width + x] == 0 {
                span_end += 1;
            }
            let top_connected = y > 0 && mask[(y - 1) * width + x] != 0;
            let bottom_connected = span_end < height && mask[span_end * width + x] != 0;
            if top_connected && bottom_connected && (span_end - y) <= gap {
                for fill_y in y..span_end {
                    mask[fill_y * width + x] = 1;
                }
            }
            y = span_end;
        }
    }
}

#[derive(Clone, Copy)]
struct ComponentStats {
    area: usize,
    min_x: usize,
    max_x: usize,
    min_y: usize,
    max_y: usize,
}

impl ComponentStats {
    fn new(x: usize, y: usize) -> Self {
        Self {
            area: 0,
            min_x: x,
            max_x: x,
            min_y: y,
            max_y: y,
        }
    }
}

fn connected_components(mask: &[u8], width: usize, height: usize) -> Vec<ComponentStats> {
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let mut labels = vec![0u32; width * height];
    let mut dsu = DisjointSet::new();

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if mask[idx] == 0 {
                continue;
            }

            let mut neighbors = [0u32; 4];
            let mut count = 0usize;

            if y > 0 {
                let up = labels[idx - width];
                if up != 0 {
                    neighbors[count] = up;
                    count += 1;
                }
            }
            if x > 0 {
                let left = labels[idx - 1];
                if left != 0 {
                    neighbors[count] = left;
                    count += 1;
                }
            }
            if y > 0 && x > 0 {
                let up_left = labels[idx - width - 1];
                if up_left != 0 {
                    neighbors[count] = up_left;
                    count += 1;
                }
            }
            if y > 0 && x + 1 < width {
                let up_right = labels[idx - width + 1];
                if up_right != 0 {
                    neighbors[count] = up_right;
                    count += 1;
                }
            }

            let label = if count == 0 {
                dsu.make_set()
            } else {
                let base = neighbors[0];
                for &n in neighbors.iter().take(count).skip(1) {
                    dsu.union(base, n);
                }
                base
            };
            labels[idx] = label;
        }
    }

    let mut stats = vec![None; dsu.len()];
    let mut components = Vec::new();

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            let label = labels[idx];
            if label == 0 {
                continue;
            }
            let root = dsu.find(label);
            labels[idx] = root;
            let entry = stats[root as usize].get_or_insert_with(|| ComponentStats::new(x, y));
            entry.area += 1;
            entry.min_x = entry.min_x.min(x);
            entry.max_x = entry.max_x.max(x);
            entry.min_y = entry.min_y.min(y);
            entry.max_y = entry.max_y.max(y);
        }
    }

    for entry in stats.into_iter().flatten() {
        components.push(entry);
    }
    components
}

#[derive(Default)]
struct DisjointSet {
    parent: Vec<u32>,
    rank: Vec<u8>,
}

impl DisjointSet {
    fn new() -> Self {
        Self {
            parent: vec![0],
            rank: vec![0],
        }
    }

    fn len(&self) -> usize {
        self.parent.len()
    }

    fn make_set(&mut self) -> u32 {
        let idx = self.parent.len() as u32;
        self.parent.push(idx);
        self.rank.push(0);
        idx
    }

    fn find(&mut self, x: u32) -> u32 {
        let idx = x as usize;
        let parent = self.parent[idx];
        if parent == x {
            return x;
        }
        let root = self.find(parent);
        self.parent[idx] = root;
        root
    }

    fn union(&mut self, a: u32, b: u32) {
        let mut root_a = self.find(a);
        let mut root_b = self.find(b);
        if root_a == root_b {
            return;
        }
        let rank_a = self.rank[root_a as usize];
        let rank_b = self.rank[root_b as usize];
        if rank_a < rank_b {
            std::mem::swap(&mut root_a, &mut root_b);
        }
        self.parent[root_b as usize] = root_a;
        if rank_a == rank_b {
            self.rank[root_a as usize] = rank_a + 1;
        }
    }
}

fn integral_image(mask: &[u8], width: usize, height: usize) -> Vec<u32> {
    let stride = width + 1;
    let mut integral = vec![0u32; stride * (height + 1)];
    for y in 0..height {
        let mut row_sum = 0u32;
        let src_offset = y * width;
        let dst_offset = (y + 1) * stride;
        for x in 0..width {
            row_sum += mask[src_offset + x] as u32;
            integral[dst_offset + x + 1] = integral[dst_offset - stride + x + 1] + row_sum;
        }
    }
    integral
}

fn rect_sum(integral: &[u32], width: usize, x0: usize, y0: usize, x1: usize, y1: usize) -> u32 {
    let stride = width + 1;
    let idx = |x: usize, y: usize| -> usize { y * stride + x };
    integral[idx(x1, y1)]
        .wrapping_sub(integral[idx(x0, y1)])
        .wrapping_sub(integral[idx(x1, y0)])
        .wrapping_add(integral[idx(x0, y0)])
}

fn evaluate_region(
    integral: &[u32],
    width: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    vmr_k: usize,
) -> (f32, f32, f32) {
    let x1 = x + w;
    let y1 = y + h;
    let hits = rect_sum(integral, width, x, y, x1, y1) as f32;
    let area = (w * h) as f32 + 1e-6;
    let fill = hits / area;

    let mut vmr = 0.0f32;
    if w >= vmr_k && h >= vmr_k {
        let dx = cmp::max(w / vmr_k, 1);
        let dy = cmp::max(h / vmr_k, 1);
        let mut count = 0.0f32;
        let mut mean = 0.0f32;
        let mut m2 = 0.0f32;

        let mut yy = y;
        while yy < y1 {
            let yb = cmp::min(yy + dy, y1);
            let mut xx = x;
            while xx < x1 {
                let xb = cmp::min(xx + dx, x1);
                let value = rect_sum(integral, width, xx, yy, xb, yb) as f32;
                count += 1.0;
                let delta = value - mean;
                mean += delta / count;
                let delta2 = value - mean;
                m2 += delta * delta2;
                xx += dx;
            }
            yy += dy;
        }

        if count > 0.0 {
            let variance = if count <= 1.0 { 0.0 } else { m2 / count };
            vmr = variance / (mean + 1e-6);
        }
    }

    let score = fill - 0.1 * vmr;
    (fill, vmr, score)
}

#[derive(Clone)]
struct Candidate {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    _fill: f32,
    _vmr: f32,
    score: f32,
}

fn merge_line_candidates(
    mut candidates: Vec<Candidate>,
    integral: &[u32],
    width: usize,
) -> Vec<Candidate> {
    if candidates.is_empty() {
        return Vec::new();
    }

    candidates.sort_by(|a, b| a.y.cmp(&b.y));
    let mut merged = Vec::with_capacity(candidates.len());
    let mut line_group = vec![candidates[0].clone()];

    for cand in candidates.into_iter().skip(1) {
        if same_line(line_group.last().unwrap(), &cand) {
            line_group.push(cand);
        } else {
            merged.extend(merge_group(line_group, integral, width));
            line_group = vec![cand];
        }
    }

    merged.extend(merge_group(line_group, integral, width));
    merged
}

fn same_line(a: &Candidate, b: &Candidate) -> bool {
    let cy1 = a.y + a.height / 2;
    let cy2 = b.y + b.height / 2;
    let diff = if cy1 >= cy2 { cy1 - cy2 } else { cy2 - cy1 };
    diff <= Y_MERGE_TOL
}

fn merge_group(mut group: Vec<Candidate>, integral: &[u32], width: usize) -> Vec<Candidate> {
    if group.is_empty() {
        return Vec::new();
    }
    group.sort_by(|a, b| a.x.cmp(&b.x));
    let mut result = Vec::with_capacity(group.len());
    let mut iter = group.into_iter();
    let mut current = iter.next().unwrap();

    for candidate in iter {
        if should_merge(&current, &candidate) {
            current = merge_candidates(&current, &candidate, integral, width);
        } else {
            result.push(current);
            current = candidate;
        }
    }
    result.push(current);
    result
}

fn should_merge(a: &Candidate, b: &Candidate) -> bool {
    let overlap = candidate_iou(a, b);
    let near = b.x <= a.x + a.width + NEAR_GAP;
    overlap >= IOU_MERGE || near
}

fn candidate_iou(a: &Candidate, b: &Candidate) -> f32 {
    let x0 = cmp::max(a.x, b.x);
    let y0 = cmp::max(a.y, b.y);
    let x1 = cmp::min(a.x + a.width, b.x + b.width);
    let y1 = cmp::min(a.y + a.height, b.y + b.height);
    let inter_w = x1.saturating_sub(x0);
    let inter_h = y1.saturating_sub(y0);
    if inter_w == 0 || inter_h == 0 {
        return 0.0;
    }
    let intersection = (inter_w * inter_h) as f32;
    let area_a = (a.width * a.height) as f32;
    let area_b = (b.width * b.height) as f32;
    let union = area_a + area_b - intersection + 1e-6;
    intersection / union
}

fn merge_candidates(a: &Candidate, b: &Candidate, integral: &[u32], width: usize) -> Candidate {
    let x0 = cmp::min(a.x, b.x);
    let y0 = cmp::min(a.y, b.y);
    let x1 = cmp::max(a.x + a.width, b.x + b.width);
    let y1 = cmp::max(a.y + a.height, b.y + b.height);
    let new_width = x1.saturating_sub(x0);
    let new_height = y1.saturating_sub(y0);
    let (fill, vmr, score) = evaluate_region(integral, width, x0, y0, new_width, new_height, VMR_K);
    Candidate {
        x: x0,
        y: y0,
        width: new_width,
        height: new_height,
        _fill: fill,
        _vmr: vmr,
        score,
    }
}

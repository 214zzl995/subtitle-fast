use std::cmp::{self, Ordering};

#[cfg(target_arch = "aarch64")]
use std::arch::is_aarch64_feature_detected;
#[cfg(target_arch = "x86_64")]
use std::arch::is_x86_feature_detected;

use super::{
    resolve_roi, DetectionRegion, LumaBandConfig, PixelRect, SubtitleDetectionConfig,
    SubtitleDetectionError, SubtitleDetectionResult, SubtitleDetector,
};

const RLSA_H_GAP: usize = 18;
const RLSA_V_GAP: usize = 3;
const MIN_AREA: usize = 400;
const MAX_AREA_RATIO: f32 = 0.35;
const MIN_ASPECT_RATIO: f32 = 2.0;
const VMR_K: usize = 4;
const NEAR_GAP: usize = 16;
const MAX_OUTPUT_REGIONS: usize = 4;

pub struct LumaBandDetector {
    config: SubtitleDetectionConfig,
    required_len: usize,
}

impl LumaBandDetector {
    pub fn new(config: SubtitleDetectionConfig) -> Result<Self, SubtitleDetectionError> {
        let required_len = required_len(&config)?;
        let _ = resolve_roi(config.frame_width, config.frame_height, config.roi, None)?;
        Ok(Self {
            config,
            required_len,
        })
    }
}

impl SubtitleDetector for LumaBandDetector {
    fn ensure_available(config: &SubtitleDetectionConfig) -> Result<(), SubtitleDetectionError> {
        required_len(config)?;
        let _ = resolve_roi(config.frame_width, config.frame_height, config.roi, None)?;
        Ok(())
    }

    fn detect(&self, frame_data: &[u8]) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        if frame_data.len() < self.required_len {
            return Err(SubtitleDetectionError::InsufficientData {
                data_len: frame_data.len(),
                required: self.required_len,
            });
        }

        let roi = resolve_roi(
            self.config.frame_width,
            self.config.frame_height,
            self.config.roi,
            None,
        )?;

        let mut mask = threshold_mask(frame_data, self.config.stride, roi, self.config.luma_band);

        rlsa_horizontal(&mut mask, roi.width, roi.height, RLSA_H_GAP);
        rlsa_vertical(&mut mask, roi.width, roi.height, RLSA_V_GAP);

        let components = connected_components(&mask, roi.width, roi.height);
        if components.is_empty() {
            return Ok(SubtitleDetectionResult::empty());
        }

        let integral = integral_image(&mask, roi.width, roi.height);
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
                &integral, roi.width, comp.min_x, comp.min_y, width, height, VMR_K,
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
            return Ok(SubtitleDetectionResult::empty());
        }

        let mut merged = merge_line_candidates(candidates, &integral, roi.width);
        if merged.is_empty() {
            return Ok(SubtitleDetectionResult::empty());
        }

        merged.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));

        let max_score = merged.first().map(|c| c.score).unwrap_or(0.0);

        let mut regions = Vec::new();
        for cand in merged.iter().take(MAX_OUTPUT_REGIONS) {
            regions.push(DetectionRegion {
                x: (cand.x + roi.x) as f32,
                y: (cand.y + roi.y) as f32,
                width: cand.width as f32,
                height: cand.height as f32,
                score: cand.score,
            });
        }

        if regions.is_empty() {
            return Ok(SubtitleDetectionResult::empty());
        }

        Ok(SubtitleDetectionResult {
            has_subtitle: true,
            max_score,
            regions,
        })
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

fn threshold_mask(data: &[u8], stride: usize, roi: PixelRect, params: LumaBandConfig) -> Vec<u8> {
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
    roi: PixelRect,
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
    roi: PixelRect,
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

    for row in 0..roi.height {
        let src_ptr = data.as_ptr().add((roi.y + row) * stride + roi.x);
        let dst_ptr = mask.as_mut_ptr().add(row * roi.width);

        let mut x = 0usize;
        while x + 16 <= roi.width {
            let pixels = _mm_loadu_si128(src_ptr.add(x) as *const __m128i);
            let ge_lo = _mm_cmpeq_epi8(pixels, _mm_max_epu8(pixels, lo_vec));
            let le_hi = _mm_cmpeq_epi8(pixels, _mm_min_epu8(pixels, hi_vec));
            let mut mask_vec = _mm_and_si128(ge_lo, le_hi);
            mask_vec = _mm_and_si128(mask_vec, ones);
            _mm_storeu_si128(dst_ptr.add(x) as *mut __m128i, mask_vec);
            x += 16;
        }

        if x < roi.width {
            let remaining = roi.width - x;
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
    roi: PixelRect,
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

    for row in 0..roi.height {
        let src_ptr = data.as_ptr().add((roi.y + row) * stride + roi.x);
        let dst_ptr = mask.as_mut_ptr().add(row * roi.width);

        let mut x = 0usize;
        while x + 16 <= roi.width {
            let pixels = vld1q_u8(src_ptr.add(x));
            let ge_lo = vceqq_u8(pixels, vmaxq_u8(pixels, lo_vec));
            let le_hi = vceqq_u8(pixels, vminq_u8(pixels, hi_vec));
            let mask_vec = vandq_u8(vandq_u8(ge_lo, le_hi), ones);
            vst1q_u8(dst_ptr.add(x), mask_vec);
            x += 16;
        }

        if x < roi.width {
            let remaining = roi.width - x;
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

            let above_connected = y > 0 && mask[(y - 1) * width + x] != 0;
            let below_connected = span_end < height && mask[span_end * width + x] != 0;
            if above_connected && below_connected && (span_end - y) <= gap {
                for fill_y in y..span_end {
                    mask[fill_y * width + x] = 1;
                }
            }
            y = span_end;
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Component {
    min_x: usize,
    max_x: usize,
    min_y: usize,
    max_y: usize,
    area: usize,
}

#[derive(Clone, Copy, Debug)]
struct Candidate {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    _fill: f32,
    _vmr: f32,
    score: f32,
}

fn connected_components(mask: &[u8], width: usize, height: usize) -> Vec<Component> {
    let mut visited = vec![false; mask.len()];
    let mut components = Vec::new();

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if visited[idx] || mask[idx] == 0 {
                continue;
            }

            let mut stack = vec![(x, y)];
            let mut min_x = x;
            let mut max_x = x;
            let mut min_y = y;
            let mut max_y = y;
            let mut area = 0usize;

            while let Some((cx, cy)) = stack.pop() {
                if cx >= width || cy >= height {
                    continue;
                }
                let cidx = cy * width + cx;
                if visited[cidx] || mask[cidx] == 0 {
                    continue;
                }
                visited[cidx] = true;
                area += 1;
                min_x = min_x.min(cx);
                max_x = max_x.max(cx);
                min_y = min_y.min(cy);
                max_y = max_y.max(cy);

                if cx > 0 {
                    stack.push((cx - 1, cy));
                }
                if cx + 1 < width {
                    stack.push((cx + 1, cy));
                }
                if cy > 0 {
                    stack.push((cx, cy - 1));
                }
                if cy + 1 < height {
                    stack.push((cx, cy + 1));
                }
            }

            components.push(Component {
                min_x,
                max_x,
                min_y,
                max_y,
                area,
            });
        }
    }

    components
}

fn evaluate_region(
    integral: &[u32],
    width: usize,
    x: usize,
    y: usize,
    region_width: usize,
    region_height: usize,
    vmr_k: usize,
) -> (f32, f32, f32) {
    let area = (region_width * region_height) as f32;
    let fill = sum_region(integral, width, x, y, region_width, region_height) as f32 / area;

    let mut vmr_sum = 0f32;
    let mut vmr_samples = 0usize;
    let step_y = cmp::max(1, region_height / vmr_k);
    for sample_y in (0..region_height).step_by(step_y) {
        let v = sum_region(integral, width, x, y + sample_y, region_width, 1) as f32;
        vmr_sum += v / region_width as f32;
        vmr_samples += 1;
    }
    let vmr = if vmr_samples > 0 {
        vmr_sum / vmr_samples as f32
    } else {
        0.0
    };

    let score = fill * vmr;
    (fill, vmr, score)
}

fn sum_region(
    integral: &[u32],
    width: usize,
    x: usize,
    y: usize,
    region_width: usize,
    region_height: usize,
) -> u32 {
    let mut sum = 0u32;
    for row in 0..region_height {
        let start = (y + row) * width + x;
        for col in 0..region_width {
            sum += integral[start + col];
        }
    }
    sum
}

fn merge_line_candidates(
    mut candidates: Vec<Candidate>,
    integral: &[u32],
    width: usize,
) -> Vec<Candidate> {
    candidates.sort_by(|a, b| a.y.cmp(&b.y));
    let mut merged: Vec<Candidate> = Vec::new();

    for cand in candidates {
        if let Some(last) = merged.last_mut() {
            let vertical_gap = cand.y.saturating_sub(last.y + last.height);
            if vertical_gap <= NEAR_GAP {
                let new_min_x = cmp::min(last.x, cand.x);
                let new_max_x = cmp::max(last.x + last.width, cand.x + cand.width);
                let new_width = new_max_x - new_min_x;
                let new_height = cmp::max(last.height, cand.height);

                let (fill, vmr, score) = evaluate_region(
                    integral,
                    width,
                    new_min_x,
                    cmp::min(last.y, cand.y),
                    new_width,
                    new_height,
                    VMR_K,
                );
                *last = Candidate {
                    x: new_min_x,
                    y: cmp::min(last.y, cand.y),
                    width: new_width,
                    height: new_height,
                    _fill: fill,
                    _vmr: vmr,
                    score,
                };
                continue;
            }
        }
        merged.push(cand);
    }

    merged
}

fn integral_image(mask: &[u8], width: usize, height: usize) -> Vec<u32> {
    let mut integral = vec![0u32; width * height];
    for y in 0..height {
        let mut row_sum = 0u32;
        for x in 0..width {
            row_sum += mask[y * width + x] as u32;
            let above = if y > 0 {
                integral[(y - 1) * width + x]
            } else {
                0
            };
            integral[y * width + x] = row_sum + above;
        }
    }
    integral
}

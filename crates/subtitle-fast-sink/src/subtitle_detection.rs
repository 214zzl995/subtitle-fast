use std::cmp::{max, min};

use thiserror::Error;

/// Configuration for [`SubtitlePresenceDetector`].
#[derive(Debug, Clone)]
pub struct SubtitleDetectionConfig {
    pub frame_width: usize,
    pub frame_height: usize,
    pub stride: usize,
    pub roi_height_ratio: f32,
    pub downsample_width: usize,
    pub run_length_min_fraction: f32,
    pub weights: LogisticWeights,
    pub decision_threshold: f32,
}

impl SubtitleDetectionConfig {
    pub fn for_frame(frame_width: usize, frame_height: usize, stride: usize) -> Self {
        Self {
            frame_width,
            frame_height,
            stride,
            roi_height_ratio: 0.20,
            downsample_width: 512,
            run_length_min_fraction: 0.03,
            weights: LogisticWeights::default(),
            decision_threshold: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LogisticWeights {
    pub bias: f32,
    pub edge_energy_weight: f32,
    pub run_ratio_weight: f32,
    pub dt_cv_weight: f32,
    pub cc_density_weight: f32,
    pub banner_weight: f32,
}

impl LogisticWeights {
    pub const fn new(
        bias: f32,
        edge_energy_weight: f32,
        run_ratio_weight: f32,
        dt_cv_weight: f32,
        cc_density_weight: f32,
        banner_weight: f32,
    ) -> Self {
        Self {
            bias,
            edge_energy_weight,
            run_ratio_weight,
            dt_cv_weight,
            cc_density_weight,
            banner_weight,
        }
    }
}

impl Default for LogisticWeights {
    fn default() -> Self {
        Self::new(-3.3, 2.2, 3.0, 1.4, 1.8, 2.0)
    }
}

#[derive(Debug, Error)]
pub enum SubtitleDetectionError {
    #[error("provided plane data length {data_len} is smaller than stride * height ({required})")]
    InsufficientData { data_len: usize, required: usize },
    #[error("downsample width must be greater than zero")]
    InvalidDownsampleWidth,
    #[error("region of interest height is zero")]
    EmptyRoi,
}

#[derive(Debug, Clone)]
pub struct SubtitleDetectionResult {
    pub edge_energy_ratio: f32,
    pub run_ratio: f32,
    pub dt_coefficient_of_variation: f32,
    pub cc_density: f32,
    pub banner_score: f32,
    pub score: f32,
    pub has_subtitle: bool,
}

#[derive(Debug, Clone)]
pub struct SubtitlePresenceDetector {
    config: SubtitleDetectionConfig,
}

impl SubtitlePresenceDetector {
    pub fn new(config: SubtitleDetectionConfig) -> Result<Self, SubtitleDetectionError> {
        if config.downsample_width == 0 {
            return Err(SubtitleDetectionError::InvalidDownsampleWidth);
        }
        Ok(Self { config })
    }

    pub fn detect(
        &self,
        y_plane: &[u8],
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        let config = &self.config;
        let required = config
            .stride
            .checked_mul(config.frame_height)
            .unwrap_or(usize::MAX);
        if y_plane.len() < required {
            return Err(SubtitleDetectionError::InsufficientData {
                data_len: y_plane.len(),
                required,
            });
        }

        let roi_start_row = config
            .frame_height
            .saturating_sub((config.frame_height as f32 * config.roi_height_ratio) as usize);
        let roi_height = config.frame_height - roi_start_row;
        if roi_height == 0 {
            return Err(SubtitleDetectionError::EmptyRoi);
        }

        let roi = copy_region(
            y_plane,
            config.stride,
            config.frame_width,
            roi_start_row,
            config.frame_height,
        );
        let roi_height = roi.len() / config.frame_width;
        let scale = config.downsample_width as f32 / config.frame_width.max(1) as f32;
        let downsampled_height = max(1, (roi_height as f32 * scale).round() as usize);
        let roi_resized = resize_bilinear(
            &roi,
            config.frame_width,
            roi_height,
            config.downsample_width,
            downsampled_height,
        );

        let mid_band_height = max(1, (config.frame_height as f32 * 0.20) as usize);
        let mid_band_start = config
            .frame_height
            .saturating_sub(roi_height + mid_band_height);
        let mid_band_start = min(mid_band_start, config.frame_height - mid_band_height);
        let mid_band = copy_region(
            y_plane,
            config.stride,
            config.frame_width,
            mid_band_start,
            mid_band_start + mid_band_height,
        );
        let mid_resized = resize_bilinear(
            &mid_band,
            config.frame_width,
            mid_band_height,
            config.downsample_width,
            max(1, (mid_band_height as f32 * scale).round() as usize),
        );

        let edge_energy_roi = sobel_mean(&roi_resized, config.downsample_width);
        let edge_energy_mid = sobel_mean(&mid_resized, config.downsample_width);
        let edge_energy_ratio = if edge_energy_mid > 0.0 {
            edge_energy_roi / (edge_energy_mid + 1e-6)
        } else {
            edge_energy_roi
        };

        let roi_u8 = roi_resized
            .iter()
            .map(|&v| v.clamp(0.0, 255.0) as u8)
            .collect::<Vec<_>>();
        let threshold = otsu_threshold(&roi_u8);
        let mut binary = roi_u8
            .iter()
            .map(|&v| if v >= threshold { 1u8 } else { 0u8 })
            .collect::<Vec<_>>();
        morphological_open_horizontal(&mut binary, config.downsample_width, downsampled_height);

        let run_threshold = max(
            3,
            (config.downsample_width as f32 * config.run_length_min_fraction).round() as usize,
        );
        let run_ratio = compute_run_ratio(
            &binary,
            config.downsample_width,
            downsampled_height,
            run_threshold,
        );

        let dt = distance_transform(&binary, config.downsample_width, downsampled_height);
        let (dt_mean, dt_std) = mean_std_for_masked(&dt, &binary);
        let dt_coefficient_of_variation = if dt_mean > 0.0 { dt_std / dt_mean } else { 0.0 };

        let cc_density =
            connected_component_density(&binary, config.downsample_width, downsampled_height);

        let banner_score =
            compute_banner_score(&roi_resized, config.downsample_width, downsampled_height);

        let weights = config.weights;
        let score = weights.bias
            + weights.edge_energy_weight * edge_energy_ratio.clamp(0.0, 3.0)
            + weights.run_ratio_weight * run_ratio.clamp(0.0, 0.6)
            - weights.dt_cv_weight * dt_coefficient_of_variation.clamp(0.0, 3.0)
            + weights.cc_density_weight * cc_density
            - weights.banner_weight * banner_score.clamp(0.0, 1.0);
        let has_subtitle = score >= config.decision_threshold;

        Ok(SubtitleDetectionResult {
            edge_energy_ratio,
            run_ratio,
            dt_coefficient_of_variation,
            cc_density,
            banner_score,
            score,
            has_subtitle,
        })
    }
}

fn copy_region(
    data: &[u8],
    stride: usize,
    width: usize,
    start_row: usize,
    end_row: usize,
) -> Vec<u8> {
    let height = end_row.saturating_sub(start_row);
    let mut out = Vec::with_capacity(width * height);
    for row in 0..height {
        let src_start = (start_row + row) * stride;
        let row_slice = &data[src_start..src_start + width];
        out.extend_from_slice(row_slice);
    }
    out
}

fn resize_bilinear(
    src: &[u8],
    src_width: usize,
    src_height: usize,
    dst_width: usize,
    dst_height: usize,
) -> Vec<f32> {
    if dst_width == 0 || dst_height == 0 {
        return Vec::new();
    }
    if src_width == 0 || src_height == 0 {
        return vec![0.0; dst_width * dst_height];
    }

    let mut out = vec![0.0f32; dst_width * dst_height];
    let scale_x = if dst_width > 1 {
        (src_width - 1) as f32 / (dst_width - 1) as f32
    } else {
        0.0
    };
    let scale_y = if dst_height > 1 {
        (src_height - 1) as f32 / (dst_height - 1) as f32
    } else {
        0.0
    };

    for dy in 0..dst_height {
        let fy = scale_y * dy as f32;
        let y0 = fy.floor() as usize;
        let y1 = min(y0 + 1, src_height - 1);
        let wy = fy - y0 as f32;
        for dx in 0..dst_width {
            let fx = scale_x * dx as f32;
            let x0 = fx.floor() as usize;
            let x1 = min(x0 + 1, src_width - 1);
            let wx = fx - x0 as f32;

            let top_left = src[y0 * src_width + x0] as f32;
            let top_right = src[y0 * src_width + x1] as f32;
            let bottom_left = src[y1 * src_width + x0] as f32;
            let bottom_right = src[y1 * src_width + x1] as f32;

            let top = top_left + (top_right - top_left) * wx;
            let bottom = bottom_left + (bottom_right - bottom_left) * wx;
            let value = top + (bottom - top) * wy;
            out[dy * dst_width + dx] = value;
        }
    }
    out
}

fn sobel_mean(image: &[f32], width: usize) -> f32 {
    if width == 0 {
        return 0.0;
    }
    let height = image.len() / width;
    if height < 2 || width < 2 {
        return 0.0;
    }

    let mut sum = 0.0f32;
    let mut count = 0usize;
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let idx = y * width + x;
            let gx = -image[idx - width - 1] - 2.0 * image[idx - 1] - image[idx + width - 1]
                + image[idx - width + 1]
                + 2.0 * image[idx + 1]
                + image[idx + width + 1];
            let gy = -image[idx - width - 1] - 2.0 * image[idx - width] - image[idx - width + 1]
                + image[idx + width - 1]
                + 2.0 * image[idx + width]
                + image[idx + width + 1];
            sum += gx.abs() + gy.abs();
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        sum / count as f32
    }
}

fn otsu_threshold(data: &[u8]) -> u8 {
    let mut hist = [0u32; 256];
    for &value in data {
        hist[value as usize] += 1;
    }
    let total: u32 = hist.iter().sum();
    if total == 0 {
        return 0;
    }

    let mut sum_total = 0u64;
    for (i, &count) in hist.iter().enumerate() {
        sum_total += (i as u64) * (count as u64);
    }

    let mut sum_b = 0u64;
    let mut w_b = 0u32;
    let mut max_between = 0f64;
    let mut threshold = 0u8;

    for (i, &count) in hist.iter().enumerate() {
        w_b += count;
        if w_b == 0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0 {
            break;
        }
        sum_b += (i as u64) * (count as u64);
        let m_b = sum_b as f64 / w_b as f64;
        let m_f = (sum_total - sum_b) as f64 / w_f as f64;
        let between = (w_b as f64) * (w_f as f64) * (m_b - m_f).powi(2);
        if between > max_between {
            max_between = between;
            threshold = i as u8;
        }
    }
    threshold
}

fn morphological_open_horizontal(mask: &mut [u8], width: usize, height: usize) {
    if width == 0 || height == 0 {
        return;
    }
    let mut eroded = vec![0u8; mask.len()];
    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            let left = mask[y * width + x.saturating_sub(1)];
            let center = mask[idx];
            let right = mask[y * width + min(x + 1, width - 1)];
            eroded[idx] = if left == 1 && center == 1 && right == 1 {
                1
            } else {
                0
            };
        }
    }
    let mut dilated = vec![0u8; mask.len()];
    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            let left = eroded[y * width + x.saturating_sub(1)];
            let center = eroded[idx];
            let right = eroded[y * width + min(x + 1, width - 1)];
            dilated[idx] = if left == 1 || center == 1 || right == 1 {
                1
            } else {
                0
            };
        }
    }
    mask.copy_from_slice(&dilated);
}

fn compute_run_ratio(mask: &[u8], width: usize, height: usize, min_run_length: usize) -> f32 {
    if width == 0 || height == 0 {
        return 0.0;
    }
    let mut run_pixels = 0usize;
    let mut total_pixels = 0usize;
    for y in 0..height {
        let mut x = 0;
        while x < width {
            if mask[y * width + x] == 0 {
                x += 1;
                continue;
            }
            let start = x;
            while x < width && mask[y * width + x] == 1 {
                x += 1;
            }
            let run = x - start;
            if run >= min_run_length {
                run_pixels += run;
            }
        }
        total_pixels += width;
    }
    run_pixels as f32 / total_pixels as f32
}

fn distance_transform(mask: &[u8], width: usize, height: usize) -> Vec<f32> {
    let mut dist = vec![f32::INFINITY; mask.len()];
    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if mask[idx] == 1 {
                dist[idx] = 0.0;
                continue;
            }
            let left = if x > 0 {
                dist[idx - 1] + 1.0
            } else {
                f32::INFINITY
            };
            let up = if y > 0 {
                dist[idx - width] + 1.0
            } else {
                f32::INFINITY
            };
            dist[idx] = dist[idx].min(left.min(up));
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let idx = y * width + x;
            let right = if x + 1 < width {
                dist[idx + 1] + 1.0
            } else {
                f32::INFINITY
            };
            let down = if y + 1 < height {
                dist[idx + width] + 1.0
            } else {
                f32::INFINITY
            };
            dist[idx] = dist[idx].min(right.min(down));
        }
    }
    dist
}

fn mean_std_for_masked(values: &[f32], mask: &[u8]) -> (f32, f32) {
    let mut sum = 0.0f32;
    let mut sum_sq = 0.0f32;
    let mut count = 0usize;
    for (value, &mask_value) in values.iter().zip(mask.iter()) {
        if mask_value == 1 {
            sum += *value;
            sum_sq += value * value;
            count += 1;
        }
    }
    if count == 0 {
        return (0.0, 0.0);
    }
    let mean = sum / count as f32;
    let variance = (sum_sq / count as f32) - mean * mean;
    let variance = variance.max(0.0);
    (mean, variance.sqrt())
}

fn connected_component_density(mask: &[u8], width: usize, height: usize) -> f32 {
    if width == 0 || height == 0 {
        return 0.0;
    }
    let mut visited = vec![false; mask.len()];
    let mut selected_area = 0usize;

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if mask[idx] == 0 || visited[idx] {
                continue;
            }
            let mut stack = vec![(x, y)];
            visited[idx] = true;
            let mut area = 0usize;
            let mut min_x = x;
            let mut max_x = x;
            let mut min_y = y;
            let mut max_y = y;

            while let Some((cx, cy)) = stack.pop() {
                area += 1;
                min_x = min(min_x, cx);
                max_x = max(max_x, cx);
                min_y = min(min_y, cy);
                max_y = max(max_y, cy);

                let neighbors = [
                    (cx.wrapping_sub(1), cy),
                    (cx + 1, cy),
                    (cx, cy.wrapping_sub(1)),
                    (cx, cy + 1),
                ];
                for (nx, ny) in neighbors {
                    if nx < width && ny < height {
                        let nidx = ny * width + nx;
                        if mask[nidx] != 0 && !visited[nidx] {
                            visited[nidx] = true;
                            stack.push((nx, ny));
                        }
                    }
                }
            }

            let comp_width = max_x - min_x + 1;
            let comp_height = max_y - min_y + 1;
            let aspect = comp_width as f32 / comp_height.max(1) as f32;
            if (6..=80).contains(&comp_width)
                && (6..=80).contains(&comp_height)
                && aspect >= 1.0
                && aspect <= 10.0
            {
                selected_area += area;
            }
        }
    }

    selected_area as f32 / (width * height) as f32
}

fn compute_banner_score(image: &[f32], width: usize, height: usize) -> f32 {
    if width == 0 || height == 0 {
        return 0.0;
    }
    let start_row = (height * 2) / 3;
    if start_row >= height {
        return 0.0;
    }
    let region = &image[start_row * width..];
    let grad = sobel_mean(region, width);
    let mean = region.iter().sum::<f32>() / region.len() as f32;
    let variance = region
        .iter()
        .map(|v| {
            let diff = *v - mean;
            diff * diff
        })
        .sum::<f32>()
        / region.len() as f32;
    let grad_norm = grad / (grad + 15.0);
    let var_norm = variance / (variance + 500.0);
    let banner = (1.0 - grad_norm).clamp(0.0, 1.0) * (1.0 - var_norm).clamp(0.0, 1.0);
    banner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_detector(width: usize, height: usize) -> SubtitlePresenceDetector {
        let stride = width;
        SubtitlePresenceDetector::new(SubtitleDetectionConfig::for_frame(width, height, stride))
            .unwrap()
    }

    #[test]
    fn uniform_frame_returns_low_score() {
        let detector = build_detector(640, 360);
        let frame = vec![32u8; 640 * 360];
        let result = detector.detect(&frame).unwrap();
        assert!(!result.has_subtitle);
        assert!(result.edge_energy_ratio < 1.0);
    }

    #[test]
    fn horizontal_lines_increase_run_ratio() {
        let width = 640;
        let height = 360;
        let stride = width;
        let mut frame = vec![0u8; stride * height];
        for y in (height - 40)..height {
            for x in 50..(width - 50) {
                if (y / 4) % 2 == 0 {
                    frame[y * stride + x] = 220;
                }
            }
        }
        let detector = SubtitlePresenceDetector::new(SubtitleDetectionConfig::for_frame(
            width, height, stride,
        ))
        .unwrap();
        let result = detector.detect(&frame).unwrap();
        assert!(result.run_ratio > 0.05);
    }
}

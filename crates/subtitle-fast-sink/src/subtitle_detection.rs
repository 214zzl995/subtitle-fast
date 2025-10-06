use std::cmp::{max, min};

use thiserror::Error;

/// Detection parameters describing the geometric layout of the incoming frames.
///
/// The detector only receives raw Y plane buffers at inference time.  All
/// geometry related data (width, height, stride) is provided when the detector
/// is constructed.  This keeps the runtime API lightweight while still
/// enabling the detector to reason about pixel positions.
#[derive(Debug, Clone)]
pub struct SubtitlePresenceDetector {
    width: usize,
    height: usize,
    stride: usize,
    roi_ratio: f32,
    downsample_width: usize,
    weights: DetectionWeights,
}

/// Configuration for the linear head described in the design document.
#[derive(Debug, Clone, Copy)]
pub struct DetectionWeights {
    pub b: f32,
    pub w_edge_ratio: f32,
    pub w_run_ratio: f32,
    pub w_dt_cv: f32,
    pub w_cc_density: f32,
    pub w_banner: f32,
    pub threshold: f32,
}

impl Default for DetectionWeights {
    fn default() -> Self {
        Self {
            // Baseline bias and weights derived from the recommendation in the
            // provided specification.
            b: -3.3,
            w_edge_ratio: 2.2,
            w_run_ratio: 3.0,
            w_dt_cv: 1.4,
            w_cc_density: 1.8,
            w_banner: 2.0,
            threshold: 0.0,
        }
    }
}

impl SubtitlePresenceDetector {
    /// Constructs a new detector instance.
    pub fn new(width: usize, height: usize, stride: usize) -> Self {
        Self {
            width,
            height,
            stride,
            roi_ratio: 0.20,
            downsample_width: 512,
            weights: DetectionWeights::default(),
        }
    }

    /// Allows overriding the ROI height ratio (portion of the frame counted as
    /// the subtitle band).
    pub fn with_roi_ratio(mut self, ratio: f32) -> Self {
        self.roi_ratio = ratio.clamp(0.05, 0.5);
        self
    }

    /// Allows overriding the width of the downsampled ROI.
    pub fn with_downsample_width(mut self, width: usize) -> Self {
        self.downsample_width = width.max(16);
        self
    }

    /// Allows overriding the weights of the linear head.
    pub fn with_weights(mut self, weights: DetectionWeights) -> Self {
        self.weights = weights;
        self
    }

    /// Evaluates whether the provided Y plane contains subtitle-like text.
    ///
    /// The detector requires that `data` length is at least `stride * height`.
    pub fn detect(&self, data: &[u8]) -> Result<SubtitleDetectionResult, DetectionError> {
        let expected = self
            .stride
            .checked_mul(self.height)
            .ok_or(DetectionError::DimensionOverflow)?;
        if data.len() < expected {
            return Err(DetectionError::InsufficientData {
                expected,
                actual: data.len(),
            });
        }

        let roi = extract_roi(
            data,
            self.width,
            self.height,
            self.stride,
            self.roi_ratio,
        );
        let roi_height = roi.height;
        let roi_width = roi.width;
        let downsample_width = self.downsample_width.min(roi_width).max(16);
        let downsampled_roi = resize_roi(&roi, downsample_width);

        let mid_band = extract_mid_band(data, self.width, self.height, self.stride);
        let downsampled_mid = resize_roi(&mid_band, downsample_width);

        let edge_roi = sobel_energy(&downsampled_roi);
        let edge_mid = sobel_energy(&downsampled_mid);
        let mean_edge_roi = edge_roi.mean;
        let mean_edge_mid = edge_mid.mean;
        let edge_ratio = if mean_edge_mid > 0.0 {
            mean_edge_roi / (mean_edge_mid + 1e-6)
        } else {
            mean_edge_roi
        };

        let threshold = percentile(&downsampled_roi.data, 0.85);
        let mut binary_mask = threshold_binary(&downsampled_roi, threshold);
        morphological_open_horizontal(&mut binary_mask, downsampled_roi.width);

        let run_ratio = compute_run_ratio(&binary_mask, downsampled_roi.width);

        let distance_map = distance_transform(&binary_mask, downsampled_roi.width);
        let dt_stats = stroke_width_stats(&binary_mask, &distance_map);
        let dt_cv = dt_stats.variation_coefficient;

        let cc_density = connected_component_density(&binary_mask, downsampled_roi.width);

        let banner_score = banner_filter_score(&downsampled_roi, &edge_roi);

        let weights = self.weights;
        let score = weights.b
            + weights.w_edge_ratio * edge_ratio.clamp(0.0, 3.0)
            + weights.w_run_ratio * run_ratio.clamp(0.0, 0.6)
            - weights.w_dt_cv * dt_cv.clamp(0.0, 3.0)
            + weights.w_cc_density * cc_density
            - weights.w_banner * banner_score.clamp(0.0, 1.5);
        let present = score >= weights.threshold;

        Ok(SubtitleDetectionResult {
            edge_ratio,
            run_ratio,
            dt_coefficient: dt_cv,
            cc_density,
            banner_score,
            score,
            present,
            roi_size: (roi_width, roi_height),
            downsampled_size: (downsampled_roi.width, downsampled_roi.height),
        })
    }
}

/// Result of the detection pipeline.
#[derive(Debug, Clone)]
pub struct SubtitleDetectionResult {
    pub edge_ratio: f32,
    pub run_ratio: f32,
    pub dt_coefficient: f32,
    pub cc_density: f32,
    pub banner_score: f32,
    pub score: f32,
    pub present: bool,
    pub roi_size: (usize, usize),
    pub downsampled_size: (usize, usize),
}

#[derive(Debug, Error)]
pub enum DetectionError {
    #[error("insufficient data: expected {expected} bytes, received {actual}")]
    InsufficientData { expected: usize, actual: usize },
    #[error("dimension overflow")]
    DimensionOverflow,
}

#[derive(Clone)]
struct ImageView<'a> {
    data: &'a [u8],
    width: usize,
    height: usize,
    stride: usize,
}

#[derive(Clone)]
struct OwnedImage {
    data: Vec<u8>,
    width: usize,
    height: usize,
}

#[derive(Clone)]
struct FloatImage {
    data: Vec<f32>,
    width: usize,
    height: usize,
    mean: f32,
}

fn extract_roi<'a>(
    data: &'a [u8],
    width: usize,
    height: usize,
    stride: usize,
    ratio: f32,
) -> ImageView<'a> {
    let ratio = ratio.clamp(0.05, 0.5);
    let roi_height = max(1, (height as f32 * ratio) as usize);
    let start_row = height - roi_height;
    let start = start_row * stride;
    let end = start + roi_height * stride;
    ImageView {
        data: &data[start..end],
        width,
        height: roi_height,
        stride,
    }
}

fn extract_mid_band<'a>(
    data: &'a [u8],
    width: usize,
    height: usize,
    stride: usize,
) -> ImageView<'a> {
    let band_height = max(1, (height as f32 * 0.20) as usize);
    let start_row = (height.saturating_sub(band_height)) / 2;
    let start = start_row * stride;
    let end = min(height, start_row + band_height) * stride;
    ImageView {
        data: &data[start..end],
        width,
        height: min(band_height, height),
        stride,
    }
}

fn resize_roi(view: &ImageView<'_>, target_width: usize) -> OwnedImage {
    let src_w = view.width;
    let src_h = view.height;
    if src_w == 0 || src_h == 0 {
        return OwnedImage {
            data: Vec::new(),
            width: src_w,
            height: src_h,
        };
    }
    if target_width >= src_w {
        // No downsampling required; create a contiguous copy.
        let mut data = vec![0u8; src_w * src_h];
        for (row_idx, dest_row) in data.chunks_mut(src_w).enumerate() {
            let start = row_idx * view.stride;
            let end = start + src_w;
            dest_row.copy_from_slice(&view.data[start..end]);
        }
        return OwnedImage {
            data,
            width: src_w,
            height: src_h,
        };
    }

    let scale = target_width as f32 / src_w as f32;
    let target_height = max(1, (src_h as f32 * scale).round() as usize);
    let mut data = vec![0u8; target_width * target_height];
    let inv_scale_x = src_w as f32 / target_width as f32;
    let inv_scale_y = src_h as f32 / target_height as f32;

    for dy in 0..target_height {
        let src_y = (dy as f32 + 0.5) * inv_scale_y - 0.5;
        let y0 = src_y.floor();
        let y1 = y0 + 1.0;
        let wy = src_y - y0;
        let y0_idx = clamp_index(y0 as isize, src_h);
        let y1_idx = clamp_index(y1 as isize, src_h);

        for dx in 0..target_width {
            let src_x = (dx as f32 + 0.5) * inv_scale_x - 0.5;
            let x0 = src_x.floor();
            let x1 = x0 + 1.0;
            let wx = src_x - x0;
            let x0_idx = clamp_index(x0 as isize, src_w);
            let x1_idx = clamp_index(x1 as isize, src_w);

            let top_left = sample(view, x0_idx, y0_idx);
            let top_right = sample(view, x1_idx, y0_idx);
            let bottom_left = sample(view, x0_idx, y1_idx);
            let bottom_right = sample(view, x1_idx, y1_idx);

            let top = lerp(top_left, top_right, wx);
            let bottom = lerp(bottom_left, bottom_right, wx);
            let value = lerp(top, bottom, wy).round() as u8;
            data[dy * target_width + dx] = value;
        }
    }

    OwnedImage {
        data,
        width: target_width,
        height: target_height,
    }
}

fn clamp_index(idx: isize, max_value: usize) -> usize {
    if max_value == 0 {
        return 0;
    }
    if idx <= 0 {
        0
    } else if (idx as usize) >= max_value {
        max_value - 1
    } else {
        idx as usize
    }
}

fn sample(view: &ImageView<'_>, x: usize, y: usize) -> f32 {
    let idx = y * view.stride + x;
    view.data[idx] as f32
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

fn sobel_energy(image: &OwnedImage) -> FloatImage {
    let width = image.width;
    let height = image.height;
    let mut data = vec![0f32; width * height];
    let mut sum = 0.0f32;

    for y in 0..height {
        for x in 0..width {
            let gx = sobel_at(&image.data, width, height, x, y, true);
            let gy = sobel_at(&image.data, width, height, x, y, false);
            let magnitude = gx.abs() + gy.abs();
            data[y * width + x] = magnitude;
            sum += magnitude;
        }
    }

    let mean = if data.is_empty() {
        0.0
    } else {
        sum / data.len() as f32
    };

    FloatImage {
        data,
        width,
        height,
        mean,
    }
}

fn sobel_at(data: &[u8], width: usize, height: usize, x: usize, y: usize, horizontal: bool) -> f32 {
    let kernel = if horizontal {
        [[-1.0, 0.0, 1.0], [-2.0, 0.0, 2.0], [-1.0, 0.0, 1.0]]
    } else {
        [[1.0, 2.0, 1.0], [0.0, 0.0, 0.0], [-1.0, -2.0, -1.0]]
    };

    let mut value = 0.0f32;
    for ky in 0..3 {
        let sy = clamp_index(y as isize + ky as isize - 1, height);
        for kx in 0..3 {
            let sx = clamp_index(x as isize + kx as isize - 1, width);
            value += data[sy * width + sx] as f32 * kernel[ky][kx];
        }
    }
    value
}

fn percentile(data: &[u8], percentile: f32) -> u8 {
    if data.is_empty() {
        return 0;
    }
    let mut histogram = [0u32; 256];
    for &value in data {
        histogram[value as usize] += 1;
    }
    let total = data.len() as f32;
    let target = (total * percentile).clamp(0.0, total - 1.0);
    let mut cumulative = 0f32;
    for (value, &count) in histogram.iter().enumerate() {
        cumulative += count as f32;
        if cumulative >= target {
            return value as u8;
        }
    }
    255
}

fn threshold_binary(image: &OwnedImage, threshold: u8) -> Vec<u8> {
    image
        .data
        .iter()
        .map(|&v| if v >= threshold { 1 } else { 0 })
        .collect()
}

fn morphological_open_horizontal(mask: &mut [u8], width: usize) {
    if width < 3 {
        return;
    }
    let height = mask.len() / width;
    let mut temp = vec![0u8; mask.len()];

    // Erosion with a 1x3 structuring element.
    for y in 0..height {
        for x in 0..width {
            let mut acc = 1u8;
            for dx in x.saturating_sub(1)..=min(x + 1, width - 1) {
                acc &= mask[y * width + dx];
            }
            temp[y * width + x] = acc;
        }
    }

    // Dilation with the same element.
    for y in 0..height {
        for x in 0..width {
            let mut acc = 0u8;
            for dx in x.saturating_sub(1)..=min(x + 1, width - 1) {
                acc |= temp[y * width + dx];
            }
            mask[y * width + x] = acc;
        }
    }
}

fn compute_run_ratio(mask: &[u8], width: usize) -> f32 {
    let total_pixels = mask.len();
    if width == 0 || total_pixels == 0 {
        return 0.0;
    }
    let threshold = max(3, (0.04 * width as f32).round() as usize);
    let mut long_run_pixels = 0usize;

    for row in mask.chunks(width) {
        let mut current = 0usize;
        for &value in row {
            if value == 1 {
                current += 1;
            } else {
                if current >= threshold {
                    long_run_pixels += current;
                }
                current = 0;
            }
        }
        if current >= threshold {
            long_run_pixels += current;
        }
    }

    long_run_pixels as f32 / (total_pixels as f32)
}

fn distance_transform(mask: &[u8], width: usize) -> Vec<f32> {
    let height = if width == 0 { 0 } else { mask.len() / width };
    let mut dist = vec![std::f32::INFINITY; mask.len()];

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if mask[idx] == 0 {
                dist[idx] = 0.0;
            } else {
                let mut best = dist[idx];
                if x > 0 {
                    best = best.min(dist[idx - 1] + 1.0);
                }
                if y > 0 {
                    best = best.min(dist[idx - width] + 1.0);
                }
                if x > 0 && y > 0 {
                    best = best.min(dist[idx - width - 1] + SQRT2);
                }
                if x + 1 < width && y > 0 {
                    best = best.min(dist[idx - width + 1] + SQRT2);
                }
                dist[idx] = best;
            }
        }
    }

    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let idx = y * width + x;
            if mask[idx] == 0 {
                continue;
            }
            let mut best = dist[idx];
            if x + 1 < width {
                best = best.min(dist[idx + 1] + 1.0);
            }
            if y + 1 < height {
                best = best.min(dist[idx + width] + 1.0);
            }
            if x + 1 < width && y + 1 < height {
                best = best.min(dist[idx + width + 1] + SQRT2);
            }
            if x > 0 && y + 1 < height {
                best = best.min(dist[idx + width - 1] + SQRT2);
            }
            dist[idx] = best;
        }
    }

    dist
}

const SQRT2: f32 = 1.4142135;

struct StrokeStats {
    variation_coefficient: f32,
}

fn stroke_width_stats(mask: &[u8], distances: &[f32]) -> StrokeStats {
    let mut sum = 0.0f32;
    let mut sum_sq = 0.0f32;
    let mut count = 0usize;

    for (&value, &distance) in mask.iter().zip(distances.iter()) {
        if value == 1 {
            sum += distance;
            sum_sq += distance * distance;
            count += 1;
        }
    }

    if count == 0 {
        return StrokeStats {
            variation_coefficient: 2.0,
        };
    }

    let mean = sum / count as f32;
    if mean <= 1e-6 {
        return StrokeStats {
            variation_coefficient: 2.0,
        };
    }
    let variance = (sum_sq / count as f32) - mean * mean;
    let std_dev = variance.max(0.0).sqrt();
    StrokeStats {
        variation_coefficient: (std_dev / mean).clamp(0.0, 5.0),
    }
}

fn connected_component_density(mask: &[u8], width: usize) -> f32 {
    if width == 0 || mask.is_empty() {
        return 0.0;
    }
    let height = mask.len() / width;
    let mut visited = vec![false; mask.len()];
    let mut queue = Vec::new();
    let mut area_sum = 0usize;

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if mask[idx] == 0 || visited[idx] {
                continue;
            }
            queue.clear();
            queue.push(idx);
            visited[idx] = true;

            let mut min_x = x;
            let mut max_x = x;
            let mut min_y = y;
            let mut max_y = y;
            let mut pixels = 0usize;

            while let Some(current) = queue.pop() {
                let cx = current % width;
                let cy = current / width;
                pixels += 1;
                min_x = min(min_x, cx);
                max_x = max(max_x, cx);
                min_y = min(min_y, cy);
                max_y = max(max_y, cy);

                for (nx, ny) in neighbors(cx, cy, width, height) {
                    let nidx = ny * width + nx;
                    if mask[nidx] == 1 && !visited[nidx] {
                        visited[nidx] = true;
                        queue.push(nidx);
                    }
                }
            }

            let comp_width = max_x - min_x + 1;
            let comp_height = max_y - min_y + 1;
            let aspect = if comp_width >= comp_height {
                comp_width as f32 / comp_height.max(1) as f32
            } else {
                comp_height as f32 / comp_width.max(1) as f32
            };

            if (6..=80).contains(&comp_width)
                && (6..=80).contains(&comp_height)
                && aspect >= 1.0
                && aspect <= 10.0
            {
                area_sum += pixels;
            }
        }
    }

    area_sum as f32 / mask.len() as f32
}

fn neighbors(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> impl Iterator<Item = (usize, usize)> {
    const OFFSETS: [(isize, isize); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    OFFSETS.into_iter().filter_map(move |(dx, dy)| {
        let nx = x as isize + dx;
        let ny = y as isize + dy;
        if nx >= 0 && nx < width as isize && ny >= 0 && ny < height as isize {
            Some((nx as usize, ny as usize))
        } else {
            None
        }
    })
}

fn banner_filter_score(image: &OwnedImage, edge_image: &FloatImage) -> f32 {
    let height = image.height;
    let edge_height = edge_image.height;
    if height == 0 || edge_height == 0 {
        return 0.0;
    }
    let start = (height as f32 * (2.0 / 3.0)).round() as usize;
    let start = min(start, min(height, edge_height));
    let section = &image.data[start * image.width..];
    if section.is_empty() {
        return 0.0;
    }
    let mut mean = 0.0f32;
    for &v in section {
        mean += v as f32;
    }
    mean /= section.len() as f32;

    let mut variance = 0.0f32;
    for &v in section {
        let diff = v as f32 - mean;
        variance += diff * diff;
    }
    variance /= section.len() as f32;

    let edge_section = &edge_image.data[start * edge_image.width..];
    if edge_section.is_empty() {
        return 0.0;
    }
    let mut edge_mean = 0.0f32;
    for &v in edge_section {
        edge_mean += v;
    }
    edge_mean /= edge_section.len() as f32;

    let variance_score = ((30.0 - variance.sqrt()).max(0.0) / 30.0).clamp(0.0, 1.0);
    let edge_score = ((1.5 - edge_mean / 255.0).max(0.0)).clamp(0.0, 1.0);
    ((variance_score + edge_score) * 0.5).clamp(0.0, 1.5)
}


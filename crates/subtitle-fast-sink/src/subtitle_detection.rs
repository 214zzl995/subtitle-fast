/// Features derived from a single grayscale frame that describe the likelihood
/// of subtitle text being present in the bottom region of the image.
#[derive(Debug, Clone, Copy)]
pub struct SubtitleDetectionFeatures {
    pub edge_ratio: f32,
    pub run_ratio: f32,
    pub stroke_width_cv: f32,
    pub cc_density: f32,
    pub banner_score: f32,
}

/// The result returned by [`SubtitleDetector::detect`] containing the
/// aggregated features and the score of the linear classifier.
#[derive(Debug, Clone, Copy)]
pub struct SubtitleDetectionResult {
    pub features: SubtitleDetectionFeatures,
    pub score: f32,
    pub has_subtitle: bool,
}

/// Errors produced when the detector receives an invalid frame.
#[derive(Debug, thiserror::Error)]
pub enum SubtitleDetectionError {
    #[error("frame is empty")]
    Empty,
    #[error("frame data length {len} is smaller than stride*height {required}")]
    PlaneTooSmall { len: usize, required: usize },
    #[error("detector configured with zero dimensions")]
    InvalidDimensions,
}

/// Subtitle detector that consumes Y-plane (luma) data and evaluates whether a
/// subtitle is likely present.
///
/// The detector assumes a fixed frame geometry (width, height, stride). The
/// same instance can therefore be reused for multiple frames coming from the
/// same source stream, while each detection call only requires passing the raw
/// luma buffer (`YPlaneFrame::data`).
#[derive(Debug, Clone)]
pub struct SubtitleDetector {
    width: usize,
    height: usize,
    stride: usize,
    roi_fraction: f32,
    downsample_width: usize,
    run_min_ratio: f32,
}

impl SubtitleDetector {
    /// Creates a detector for frames with the provided geometry.
    pub fn new(width: usize, height: usize, stride: usize) -> Result<Self, SubtitleDetectionError> {
        if width == 0 || height == 0 || stride == 0 {
            return Err(SubtitleDetectionError::InvalidDimensions);
        }
        Ok(Self {
            width,
            height,
            stride,
            roi_fraction: 0.20,
            downsample_width: 512,
            run_min_ratio: 0.04,
        })
    }

    /// Runs the detection pipeline.
    pub fn detect(&self, data: &[u8]) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        if self.width == 0 || self.height == 0 {
            return Err(SubtitleDetectionError::InvalidDimensions);
        }
        if data.is_empty() {
            return Err(SubtitleDetectionError::Empty);
        }
        let required = self.stride * self.height;
        if data.len() < required {
            return Err(SubtitleDetectionError::PlaneTooSmall {
                len: data.len(),
                required,
            });
        }

        let roi_start = self
            .height
            .saturating_sub((self.height as f32 * self.roi_fraction) as usize);
        let roi_height = self.height - roi_start;
        let roi_height = roi_height.max(1);
        let roi_data = extract_roi(
            data,
            self.width,
            self.height,
            self.stride,
            roi_start,
            roi_height,
        );

        let downsampled = downsample_box(&roi_data, self.width, roi_height, self.downsample_width);
        let down_width = if self.width <= self.downsample_width {
            self.width
        } else {
            self.downsample_width
        };
        let down_height = downsampled.len() / down_width;

        let edge_ratio = compute_edge_ratio(
            data,
            self.width,
            self.height,
            self.stride,
            roi_start,
            roi_height,
        );

        let (mask, otsu_threshold) = binarize_otsu(&downsampled, down_width, down_height);
        let mask = morph_open_horizontal(&mask, down_width, down_height);

        let run_ratio = compute_run_ratio(&mask, down_width, down_height, self.run_min_ratio);
        let stroke_width_cv = compute_stroke_width_cv(&mask, down_width, down_height);
        let cc_density = compute_cc_density(&mask, down_width, down_height);
        let banner_score =
            compute_banner_score(&downsampled, down_width, down_height, otsu_threshold);

        let features = SubtitleDetectionFeatures {
            edge_ratio,
            run_ratio,
            stroke_width_cv,
            cc_density,
            banner_score,
        };

        let score = evaluate_linear_head(&features);
        Ok(SubtitleDetectionResult {
            features,
            score,
            has_subtitle: score >= 0.0,
        })
    }
}

fn extract_roi(
    data: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    start_row: usize,
    roi_height: usize,
) -> Vec<u8> {
    let mut roi = vec![0u8; width * roi_height];
    for row in 0..roi_height {
        let src_idx = (start_row + row) * stride;
        let dst_idx = row * width;
        roi[dst_idx..dst_idx + width].copy_from_slice(&data[src_idx..src_idx + width]);
    }
    roi
}

fn downsample_box(input: &[u8], width: usize, height: usize, target_width: usize) -> Vec<u8> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    if width <= target_width {
        return input.to_vec();
    }
    let scale = target_width as f32 / width as f32;
    let target_height = ((height as f32) * scale).max(1.0).round() as usize;
    let mut output = vec![0u8; target_width * target_height];

    let integral = build_integral_image(input, width, height);
    for ty in 0..target_height {
        let src_y0 = (ty as f32 / target_height as f32 * height as f32).floor() as usize;
        let src_y1 =
            (((ty + 1) as f32 / target_height as f32 * height as f32).ceil() as usize).min(height);
        let src_y1 = src_y1.max(src_y0 + 1);
        for tx in 0..target_width {
            let src_x0 = (tx as f32 / target_width as f32 * width as f32).floor() as usize;
            let src_x1 =
                (((tx + 1) as f32 / target_width as f32 * width as f32).ceil() as usize).min(width);
            let src_x1 = src_x1.max(src_x0 + 1);
            let avg = sample_integral(&integral, width, src_x0, src_y0, src_x1, src_y1);
            output[ty * target_width + tx] = avg;
        }
    }
    output
}

fn build_integral_image(input: &[u8], width: usize, height: usize) -> Vec<u32> {
    let mut integral = vec![0u32; (width + 1) * (height + 1)];
    for y in 0..height {
        let mut row_sum = 0u32;
        for x in 0..width {
            row_sum += input[y * width + x] as u32;
            let idx = (y + 1) * (width + 1) + (x + 1);
            integral[idx] = integral[idx - (width + 1)] + row_sum;
        }
    }
    integral
}

fn sample_integral(
    integral: &[u32],
    width: usize,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
) -> u8 {
    let stride = width + 1;
    let a = integral[y0 * stride + x0];
    let b = integral[y0 * stride + x1];
    let c = integral[y1 * stride + x0];
    let d = integral[y1 * stride + x1];
    let area = ((x1 - x0) * (y1 - y0)) as u32;
    if area == 0 {
        return 0;
    }
    let sum = d + a - b - c;
    (sum / area) as u8
}

fn compute_edge_ratio(
    data: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    roi_start: usize,
    roi_height: usize,
) -> f32 {
    if width < 3 || height < 3 {
        return 0.0;
    }
    let sobel = |x: usize, y: usize| -> f32 {
        let idx = y * stride + x;
        let gx = data[idx + 1 + stride] as f32
            + 2.0 * data[idx + 1] as f32
            + data[idx + 1 - stride] as f32
            - data[idx - 1 + stride] as f32
            - 2.0 * data[idx - 1] as f32
            - data[idx - 1 - stride] as f32;
        let gy = data[idx - stride - 1] as f32
            + 2.0 * data[idx - stride] as f32
            + data[idx - stride + 1] as f32
            - data[idx + stride - 1] as f32
            - 2.0 * data[idx + stride] as f32
            - data[idx + stride + 1] as f32;
        gx.abs() + gy.abs()
    };

    let mut roi_sum = 0.0;
    let mut roi_count = 0;
    let mut mid_sum = 0.0;
    let mut mid_count = 0;

    let mid_band_height = (height as f32 * 0.2).max(1.0).round() as usize;
    let mid_center = height / 2;
    let mid_band_start = mid_center.saturating_sub(mid_band_height / 2);
    let mid_band_end = (mid_band_start + mid_band_height).min(height);

    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let g = sobel(x, y);
            if y >= roi_start && y < roi_start + roi_height {
                roi_sum += g;
                roi_count += 1;
            }
            if y >= mid_band_start && y < mid_band_end {
                mid_sum += g;
                mid_count += 1;
            }
        }
    }

    if roi_count == 0 || mid_count == 0 {
        return 0.0;
    }

    let roi_mean = roi_sum / roi_count as f32;
    let mid_mean = mid_sum / mid_count as f32;
    roi_mean / (mid_mean + 1e-6)
}

fn binarize_otsu(input: &[u8], width: usize, height: usize) -> (Vec<u8>, f32) {
    let mut histogram = [0u32; 256];
    for &value in input.iter() {
        histogram[value as usize] += 1;
    }

    let total = (width * height) as u32;
    let mut sum_total = 0u64;
    for (i, &count) in histogram.iter().enumerate() {
        sum_total += (i as u64) * (count as u64);
    }

    let mut sum_b = 0u64;
    let mut w_b = 0u32;
    let mut max_var = 0.0f64;
    let mut threshold = 0u8;

    for (i, &count) in histogram.iter().enumerate() {
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
        if between > max_var {
            max_var = between;
            threshold = i as u8;
        }
    }

    let mut mask = vec![0u8; input.len()];
    for (dst, &src) in mask.iter_mut().zip(input.iter()) {
        *dst = if src >= threshold { 1 } else { 0 };
    }
    (mask, threshold as f32)
}

fn morph_open_horizontal(mask: &[u8], width: usize, height: usize) -> Vec<u8> {
    if width < 3 {
        return mask.to_vec();
    }
    let mut eroded = vec![0u8; mask.len()];
    for y in 0..height {
        for x in 1..width - 1 {
            let idx = y * width + x;
            eroded[idx] = mask[idx - 1] & mask[idx] & mask[idx + 1];
        }
    }
    let mut dilated = vec![0u8; mask.len()];
    for y in 0..height {
        for x in 1..width - 1 {
            let idx = y * width + x;
            dilated[idx] = eroded[idx - 1] | eroded[idx] | eroded[idx + 1];
        }
    }
    dilated
}

fn compute_run_ratio(mask: &[u8], width: usize, height: usize, min_ratio: f32) -> f32 {
    if width == 0 || height == 0 {
        return 0.0;
    }
    let min_run = (width as f32 * min_ratio).max(3.0).round() as usize;
    let mut long_pixels = 0usize;
    for y in 0..height {
        let row = &mask[y * width..(y + 1) * width];
        let mut current = 0usize;
        for &value in row {
            if value != 0 {
                current += 1;
            } else {
                if current >= min_run {
                    long_pixels += current;
                }
                current = 0;
            }
        }
        if current >= min_run {
            long_pixels += current;
        }
    }
    long_pixels as f32 / (width * height) as f32
}

fn compute_stroke_width_cv(mask: &[u8], width: usize, height: usize) -> f32 {
    if width == 0 || height == 0 {
        return 0.0;
    }
    let mut horiz = vec![0usize; width * height];
    let mut vert = vec![0usize; width * height];

    for y in 0..height {
        let mut run = 0usize;
        for x in 0..width {
            let idx = y * width + x;
            if mask[idx] != 0 {
                run += 1;
            } else {
                run = 0;
            }
            horiz[idx] = run;
        }
        let mut run = 0usize;
        for x in (0..width).rev() {
            let idx = y * width + x;
            if mask[idx] != 0 {
                run = run.max(horiz[idx]);
            } else {
                run = 0;
            }
            horiz[idx] = horiz[idx].min(run);
        }
    }

    for x in 0..width {
        let mut run = 0usize;
        for y in 0..height {
            let idx = y * width + x;
            if mask[idx] != 0 {
                run += 1;
            } else {
                run = 0;
            }
            vert[idx] = run;
        }
        let mut run = 0usize;
        for y in (0..height).rev() {
            let idx = y * width + x;
            if mask[idx] != 0 {
                run = run.max(vert[idx]);
            } else {
                run = 0;
            }
            vert[idx] = vert[idx].min(run);
        }
    }

    let mut widths = Vec::new();
    widths.reserve(width * height / 8);
    for idx in 0..mask.len() {
        if mask[idx] != 0 {
            let stroke = horiz[idx].min(vert[idx]) as f32;
            if stroke > 0.0 {
                widths.push(stroke);
            }
        }
    }
    if widths.is_empty() {
        return 3.0;
    }
    let mean = widths.iter().copied().sum::<f32>() / widths.len() as f32;
    if mean == 0.0 {
        return 3.0;
    }
    let variance = widths.iter().map(|w| (w - mean).powi(2)).sum::<f32>() / widths.len() as f32;
    let std_dev = variance.sqrt();
    (std_dev / mean).clamp(0.0, 5.0)
}

fn compute_cc_density(mask: &[u8], width: usize, height: usize) -> f32 {
    if width == 0 || height == 0 {
        return 0.0;
    }
    let mut visited = vec![false; mask.len()];
    let mut queue = Vec::new();
    let mut total_pixels = 0usize;
    for &v in mask {
        if v != 0 {
            total_pixels += 1;
        }
    }
    if total_pixels == 0 {
        return 0.0;
    }

    let mut accepted_pixels = 0usize;

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
                pixels += 1;
                let cy = current / width;
                let cx = current % width;
                min_x = min_x.min(cx);
                max_x = max_x.max(cx);
                min_y = min_y.min(cy);
                max_y = max_y.max(cy);

                for (nx, ny) in neighbors4(cx, cy, width, height) {
                    let nidx = ny * width + nx;
                    if mask[nidx] != 0 && !visited[nidx] {
                        visited[nidx] = true;
                        queue.push(nidx);
                    }
                }
            }

            let w = max_x.saturating_sub(min_x) + 1;
            let h = max_y.saturating_sub(min_y) + 1;
            let aspect = w as f32 / h as f32;
            if w >= 6 && w <= 80 && h >= 4 && h <= 80 && aspect >= 1.0 && aspect <= 10.0 {
                accepted_pixels += pixels;
            }
        }
    }

    accepted_pixels as f32 / (width * height) as f32
}

fn neighbors4(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> impl Iterator<Item = (usize, usize)> {
    let mut neighbors = Vec::with_capacity(4);
    if x > 0 {
        neighbors.push((x - 1, y));
    }
    if x + 1 < width {
        neighbors.push((x + 1, y));
    }
    if y > 0 {
        neighbors.push((x, y - 1));
    }
    if y + 1 < height {
        neighbors.push((x, y + 1));
    }
    neighbors.into_iter()
}

fn compute_banner_score(input: &[u8], width: usize, height: usize, threshold: f32) -> f32 {
    if height == 0 || width == 0 {
        return 0.0;
    }
    let banner_start = (height * 2) / 3;
    let mut variance_sum = 0.0f32;
    let mut gradient_sum = 0.0f32;
    let mut count = 0usize;

    for y in banner_start.max(1)..height - 1 {
        for x in 1..width - 1 {
            let idx = y * width + x;
            let value = input[idx] as f32;
            variance_sum += (value - threshold).abs();
            let gx = input[idx + 1] as f32 - input[idx - 1] as f32;
            let gy = input[idx + width] as f32 - input[idx - width] as f32;
            gradient_sum += ((gx * gx + gy * gy).sqrt()) as f32;
            count += 1;
        }
    }

    if count == 0 {
        return 0.0;
    }

    let mean_variance = variance_sum / count as f32;
    let mean_gradient = gradient_sum / count as f32;

    let variance_norm = (1.0 - (mean_variance / 32.0)).clamp(0.0, 1.0);
    let gradient_norm = (1.0 - (mean_gradient / 16.0)).clamp(0.0, 1.0);
    variance_norm * gradient_norm
}

fn evaluate_linear_head(features: &SubtitleDetectionFeatures) -> f32 {
    let edge = features.edge_ratio.clamp(0.0, 3.0);
    let run = features.run_ratio.clamp(0.0, 0.6);
    let stroke = features.stroke_width_cv.clamp(0.0, 3.0);
    let cc = features.cc_density;
    let banner = features.banner_score;

    -3.3 + 2.2 * edge + 3.0 * run - 1.4 * stroke + 1.8 * cc - 2.0 * banner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_handles_empty_input() {
        let detector = SubtitleDetector::new(1920, 1080, 1920).unwrap();
        let err = detector.detect(&[]).unwrap_err();
        assert!(matches!(err, SubtitleDetectionError::PlaneTooSmall { .. }));
    }

    #[test]
    fn detector_rejects_invalid_dimensions() {
        let err = SubtitleDetector::new(0, 1080, 1920).unwrap_err();
        assert!(matches!(err, SubtitleDetectionError::InvalidDimensions));
    }
}

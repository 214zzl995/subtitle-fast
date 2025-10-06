use subtitle_fast_decoder::YPlaneFrame;

const ROI_HEIGHT_FRACTION: f32 = 0.20;
const MID_BAND_FRACTION: f32 = 0.20;
const TARGET_DOWNSCALE_WIDTH: usize = 512;
const EPSILON: f32 = 1e-6;

const LINEAR_WEIGHTS: [f32; 5] = [2.2, 3.0, 1.4, 1.8, 2.0];
const LINEAR_BIAS: f32 = -3.3;
const LINEAR_THRESHOLD: f32 = 0.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubtitlePresenceFeatures {
    pub edge_ratio: f32,
    pub run_ratio: f32,
    pub distance_transform_cv: f32,
    pub cc_density: f32,
    pub banner_score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubtitlePresenceResult {
    pub features: SubtitlePresenceFeatures,
    pub score: f32,
    pub threshold: f32,
}

impl SubtitlePresenceResult {
    pub fn has_subtitle(&self) -> bool {
        self.score >= self.threshold
    }
}

#[derive(Debug, Clone)]
struct Plane<'a> {
    data: &'a [u8],
    width: usize,
    height: usize,
    stride: usize,
}

impl<'a> Plane<'a> {
    fn from_frame(frame: &'a YPlaneFrame) -> Self {
        Self {
            data: frame.data(),
            width: frame.width() as usize,
            height: frame.height() as usize,
            stride: frame.stride(),
        }
    }
}

pub fn detect_subtitle_presence(frame: &YPlaneFrame) -> SubtitlePresenceResult {
    let plane = Plane::from_frame(frame);
    let default_features = SubtitlePresenceFeatures {
        edge_ratio: 0.0,
        run_ratio: 0.0,
        distance_transform_cv: 0.0,
        cc_density: 0.0,
        banner_score: 0.0,
    };

    if plane.width == 0 || plane.height == 0 {
        return SubtitlePresenceResult {
            features: default_features,
            score: LINEAR_BIAS,
            threshold: LINEAR_THRESHOLD,
        };
    }

    let roi = extract_roi(&plane);
    let mid_band = extract_mid_band(&plane);
    let roi_downsampled = downsample_plane(&roi, TARGET_DOWNSCALE_WIDTH);
    let mid_downsampled = downsample_plane(&mid_band, TARGET_DOWNSCALE_WIDTH);

    let edge_ratio = compute_edge_ratio(&roi_downsampled, &mid_downsampled);
    let binarized = adaptive_threshold(&roi_downsampled);
    let opened = morphological_open(&binarized, roi_downsampled.width, roi_downsampled.height);
    let run_ratio = compute_run_ratio(&opened, roi_downsampled.width, roi_downsampled.height);
    let distance_transform_cv = compute_distance_transform_cv(
        &opened,
        roi_downsampled.width,
        roi_downsampled.height,
    );
    let cc_density = compute_cc_density(&opened, roi_downsampled.width, roi_downsampled.height);
    let banner_score = compute_banner_score(
        &roi_downsampled,
        &opened,
        edge_ratio,
    );

    let features = SubtitlePresenceFeatures {
        edge_ratio,
        run_ratio,
        distance_transform_cv,
        cc_density,
        banner_score,
    };

    let score = linear_score(&features);

    SubtitlePresenceResult {
        features,
        score,
        threshold: LINEAR_THRESHOLD,
    }
}

#[derive(Clone)]
struct OwnedPlane {
    data: Vec<u8>,
    width: usize,
    height: usize,
}

impl OwnedPlane {
    fn to_plane(&self) -> Plane<'_> {
        Plane {
            data: &self.data,
            width: self.width,
            height: self.height,
            stride: self.width,
        }
    }
}

fn extract_roi(plane: &Plane<'_>) -> OwnedPlane {
    let roi_height = ((plane.height as f32) * ROI_HEIGHT_FRACTION)
        .round()
        .clamp(1.0, plane.height as f32) as usize;
    let start_row = plane.height.saturating_sub(roi_height);
    copy_region(plane, start_row, roi_height)
}

fn extract_mid_band(plane: &Plane<'_>) -> OwnedPlane {
    let band_height = ((plane.height as f32) * MID_BAND_FRACTION)
        .round()
        .clamp(1.0, plane.height as f32) as usize;
    if band_height >= plane.height {
        return copy_region(plane, 0, plane.height);
    }
    let middle = plane.height / 2;
    let half = band_height / 2;
    let start_row = middle.saturating_sub(half);
    copy_region(plane, start_row, band_height)
}

fn copy_region(plane: &Plane<'_>, start_row: usize, height: usize) -> OwnedPlane {
    let end_row = (start_row + height).min(plane.height);
    let actual_height = end_row.saturating_sub(start_row);
    let mut data = Vec::with_capacity(actual_height * plane.width);
    for row in start_row..end_row {
        let start = row * plane.stride;
        let end = start + plane.width;
        data.extend_from_slice(&plane.data[start..end]);
    }
    OwnedPlane {
        data,
        width: plane.width,
        height: actual_height,
    }
}

fn downsample_plane(plane: &OwnedPlane, target_width: usize) -> OwnedPlane {
    if plane.width == 0 || plane.height == 0 {
        return plane.clone();
    }
    let desired_width = plane.width.min(target_width.max(1));
    if desired_width == plane.width {
        return plane.clone();
    }
    let scale_x = plane.width as f32 / desired_width as f32;
    let desired_height = ((plane.height as f32) / scale_x)
        .round()
        .max(1.0) as usize;

    let mut data = vec![0u8; desired_width * desired_height];
    for dy in 0..desired_height {
        let src_y_start = ((dy as f32) * scale_x).floor() as usize;
        let mut src_y_end = (((dy + 1) as f32) * scale_x).ceil() as usize;
        if src_y_end <= src_y_start {
            src_y_end = src_y_start + 1;
        }
        src_y_end = src_y_end.min(plane.height);
        for dx in 0..desired_width {
            let src_x_start = ((dx as f32) * scale_x).floor() as usize;
            let mut src_x_end = (((dx + 1) as f32) * scale_x).ceil() as usize;
            if src_x_end <= src_x_start {
                src_x_end = src_x_start + 1;
            }
            src_x_end = src_x_end.min(plane.width);
            let mut sum = 0u32;
            let mut count = 0u32;
            for sy in src_y_start..src_y_end {
                let base = sy.min(plane.height - 1) * plane.width;
                for sx in src_x_start..src_x_end {
                    let col = sx.min(plane.width - 1);
                    sum += plane.data[base + col] as u32;
                    count += 1;
                }
            }
            let count = count.max(1);
            data[dy * desired_width + dx] = (sum / count) as u8;
        }
    }

    OwnedPlane {
        data,
        width: desired_width,
        height: desired_height,
    }
}

fn compute_edge_ratio(roi: &OwnedPlane, mid: &OwnedPlane) -> f32 {
    let roi_plane = roi.to_plane();
    let mid_plane = mid.to_plane();
    let roi_edge = sobel_mean(&roi_plane);
    let mid_edge = sobel_mean(&mid_plane);
    roi_edge / (mid_edge + EPSILON)
}

fn sobel_mean(plane: &Plane<'_>) -> f32 {
    if plane.width < 3 || plane.height < 3 {
        return 0.0;
    }
    let mut total = 0u64;
    let mut count = 0u64;
    for y in 1..plane.height - 1 {
        for x in 1..plane.width - 1 {
            let p00 = plane.data[(y - 1) * plane.stride + (x - 1)] as i32;
            let p01 = plane.data[(y - 1) * plane.stride + x] as i32;
            let p02 = plane.data[(y - 1) * plane.stride + (x + 1)] as i32;
            let p10 = plane.data[y * plane.stride + (x - 1)] as i32;
            let p12 = plane.data[y * plane.stride + (x + 1)] as i32;
            let p20 = plane.data[(y + 1) * plane.stride + (x - 1)] as i32;
            let p21 = plane.data[(y + 1) * plane.stride + x] as i32;
            let p22 = plane.data[(y + 1) * plane.stride + (x + 1)] as i32;

            let gx = (p02 + 2 * p12 + p22) - (p00 + 2 * p10 + p20);
            let gy = (p20 + 2 * p21 + p22) - (p00 + 2 * p01 + p02);
            total += (gx.abs() + gy.abs()) as u64;
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        total as f32 / count as f32
    }
}

fn adaptive_threshold(plane: &OwnedPlane) -> Vec<u8> {
    let mut histogram = [0u32; 256];
    for &value in &plane.data {
        histogram[value as usize] += 1;
    }
    let total_pixels = plane.data.len() as u32;
    if total_pixels == 0 {
        return vec![];
    }
    let threshold_quantile = (total_pixels as f32 * 0.85).round() as u32;
    let mut cumulative = 0u32;
    let mut threshold = 0u8;
    for (value, &count) in histogram.iter().enumerate() {
        cumulative += count;
        if cumulative >= threshold_quantile {
            threshold = value as u8;
            break;
        }
    }
    plane
        .data
        .iter()
        .map(|&v| if v >= threshold { 1 } else { 0 })
        .collect()
}

fn morphological_open(data: &[u8], width: usize, height: usize) -> Vec<u8> {
    if width == 0 || height == 0 {
        return vec![];
    }
    let mut eroded = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..width {
            let mut min_val = 1u8;
            for dx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                let idx = y * width + dx;
                min_val = min_val.min(data[idx]);
                if min_val == 0 {
                    break;
                }
            }
            eroded[y * width + x] = min_val;
        }
    }

    let mut dilated = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..width {
            let mut max_val = 0u8;
            for dx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                let idx = y * width + dx;
                max_val = max_val.max(eroded[idx]);
                if max_val == 1 {
                    break;
                }
            }
            dilated[y * width + x] = max_val;
        }
    }
    dilated
}

fn compute_run_ratio(binary: &[u8], width: usize, height: usize) -> f32 {
    if width == 0 || height == 0 {
        return 0.0;
    }
    let threshold = (width as f32 * 0.04).round() as usize;
    let min_run = threshold.max(3);
    let mut covered = 0usize;
    for row in binary.chunks(width) {
        let mut current = 0usize;
        for &pixel in row {
            if pixel > 0 {
                current += 1;
            } else {
                if current >= min_run {
                    covered += current;
                }
                current = 0;
            }
        }
        if current >= min_run {
            covered += current;
        }
    }
    covered as f32 / (width * height) as f32
}

fn compute_distance_transform_cv(binary: &[u8], width: usize, height: usize) -> f32 {
    if width == 0 || height == 0 {
        return 0.0;
    }
    let mut dist = vec![f32::INFINITY; width * height];
    for (idx, &value) in binary.iter().enumerate() {
        if value == 0 {
            dist[idx] = 0.0;
        }
    }

    // forward pass
    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            let mut best = dist[idx];
            if x > 0 {
                best = best.min(dist[idx - 1] + 1.0);
            }
            if y > 0 {
                best = best.min(dist[idx - width] + 1.0);
            }
            if x > 0 && y > 0 {
                best = best.min(dist[idx - width - 1] + 1.4142135);
            }
            if x + 1 < width && y > 0 {
                best = best.min(dist[idx - width + 1] + 1.4142135);
            }
            dist[idx] = best;
        }
    }

    // backward pass
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let idx = y * width + x;
            let mut best = dist[idx];
            if x + 1 < width {
                best = best.min(dist[idx + 1] + 1.0);
            }
            if y + 1 < height {
                best = best.min(dist[idx + width] + 1.0);
            }
            if x + 1 < width && y + 1 < height {
                best = best.min(dist[idx + width + 1] + 1.4142135);
            }
            if x > 0 && y + 1 < height {
                best = best.min(dist[idx + width - 1] + 1.4142135);
            }
            dist[idx] = best;
        }
    }

    let mut values = Vec::new();
    for (idx, &value) in binary.iter().enumerate() {
        if value > 0 {
            let distance = dist[idx];
            if distance.is_finite() && distance > 0.0 {
                values.push(distance);
            }
        }
    }
    if values.is_empty() {
        return 0.0;
    }
    let mean = values.iter().copied().sum::<f32>() / values.len() as f32;
    if mean <= EPSILON {
        return 0.0;
    }
    let variance = values
        .iter()
        .map(|&v| {
            let diff = v - mean;
            diff * diff
        })
        .sum::<f32>()
        / values.len() as f32;
    let std_dev = variance.sqrt();
    std_dev / (mean + EPSILON)
}

fn compute_cc_density(binary: &[u8], width: usize, height: usize) -> f32 {
    if width == 0 || height == 0 {
        return 0.0;
    }
    let mut visited = vec![false; width * height];
    let mut selected_area = 0usize;
    let mut stack = Vec::new();

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if binary[idx] == 0 || visited[idx] {
                continue;
            }
            stack.push((x, y));
            visited[idx] = true;
            let mut pixels = 0usize;
            let mut min_x = x;
            let mut max_x = x;
            let mut min_y = y;
            let mut max_y = y;

            while let Some((cx, cy)) = stack.pop() {
                pixels += 1;
                min_x = min_x.min(cx);
                max_x = max_x.max(cx);
                min_y = min_y.min(cy);
                max_y = max_y.max(cy);

                let neighbors = [
                    (cx.wrapping_sub(1), cy),
                    (cx.saturating_add(1), cy),
                    (cx, cy.wrapping_sub(1)),
                    (cx, cy.saturating_add(1)),
                ];
                for (nx, ny) in neighbors {
                    if nx >= width || ny >= height {
                        continue;
                    }
                    let nidx = ny * width + nx;
                    if binary[nidx] == 0 || visited[nidx] {
                        continue;
                    }
                    visited[nidx] = true;
                    stack.push((nx, ny));
                }
            }

            let comp_width = max_x - min_x + 1;
            let comp_height = max_y - min_y + 1;
            if comp_width < 6
                || comp_width > 80
                || comp_height < 6
                || comp_height > 80
            {
                continue;
            }
            let aspect_ratio = comp_width as f32 / comp_height as f32;
            if !(1.0..=10.0).contains(&aspect_ratio) {
                continue;
            }
            selected_area += pixels;
        }
    }
    selected_area as f32 / (width * height) as f32
}

fn compute_banner_score(roi: &OwnedPlane, binary: &[u8], edge_ratio: f32) -> f32 {
    if roi.width == 0 || roi.height == 0 {
        return 0.0;
    }
    let len = roi.data.len() as f32;
    if len <= 0.0 {
        return 0.0;
    }
    let mean = roi.data.iter().map(|&v| v as f32).sum::<f32>() / len;
    let variance = roi
        .data
        .iter()
        .map(|&v| {
            let diff = v as f32 - mean;
            diff * diff
        })
        .sum::<f32>()
        / len;
    let low_var = (1.0 - (variance / 500.0)).clamp(0.0, 1.0);
    let low_edge = (1.0 - (edge_ratio / 2.5)).clamp(0.0, 1.0);
    let coverage = binary.iter().filter(|&&v| v > 0).count() as f32 / len;
    (low_var * low_edge * coverage).clamp(0.0, 1.0)
}

fn linear_score(features: &SubtitlePresenceFeatures) -> f32 {
    let clamped_edge = features.edge_ratio.clamp(0.0, 3.0);
    let clamped_run = features.run_ratio.clamp(0.0, 0.6);
    let clamped_dt = features.distance_transform_cv.clamp(0.0, 3.0);
    let cc = features.cc_density;
    let banner = features.banner_score;
    LINEAR_BIAS
        + LINEAR_WEIGHTS[0] * clamped_edge
        + LINEAR_WEIGHTS[1] * clamped_run
        - LINEAR_WEIGHTS[2] * clamped_dt
        + LINEAR_WEIGHTS[3] * cc
        - LINEAR_WEIGHTS[4] * banner
}

#[cfg(test)]
mod tests {
    use super::*;
    use subtitle_fast_decoder::YPlaneFrame;

    fn build_frame(width: usize, height: usize, stride: usize, value: u8) -> YPlaneFrame {
        let mut data = vec![value; stride * height];
        for row in 0..height {
            for col in width..stride {
                data[row * stride + col] = 0;
            }
        }
        YPlaneFrame::from_owned(
            width as u32,
            height as u32,
            stride,
            None,
            data,
        )
        .expect("valid frame")
    }

    #[test]
    fn empty_roi_returns_default_bias() {
        let frame = build_frame(0, 0, 0, 0);
        let result = detect_subtitle_presence(&frame);
        assert_eq!(result.score, LINEAR_BIAS);
        assert!(!result.has_subtitle());
    }

    #[test]
    fn uniform_frame_has_low_score() {
        let frame = build_frame(640, 360, 640, 32);
        let result = detect_subtitle_presence(&frame);
        assert!(!result.has_subtitle());
    }
}

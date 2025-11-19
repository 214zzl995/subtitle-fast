use std::cmp::Ordering;

pub fn resize_average(
    pixels: &[f32],
    width: usize,
    height: usize,
    new_width: usize,
    new_height: usize,
) -> Vec<f32> {
    assert_eq!(pixels.len(), width * height);
    if width == 0 || height == 0 || new_width == 0 || new_height == 0 {
        return vec![0.0; new_width * new_height];
    }
    let scale_x = width as f32 / new_width as f32;
    let scale_y = height as f32 / new_height as f32;
    let mut output = vec![0.0f32; new_width * new_height];
    for ny in 0..new_height {
        let src_y0 = (ny as f32 * scale_y).floor() as isize;
        let src_y1 = (((ny + 1) as f32 * scale_y).ceil() as isize).min(height as isize);
        for nx in 0..new_width {
            let src_x0 = (nx as f32 * scale_x).floor() as isize;
            let src_x1 = (((nx + 1) as f32 * scale_x).ceil() as isize).min(width as isize);
            let mut sum = 0.0f32;
            let mut count = 0;
            for sy in src_y0.max(0)..src_y1.max(src_y0 + 1) {
                for sx in src_x0.max(0)..src_x1.max(src_x0 + 1) {
                    let idx = sy as usize * width + sx as usize;
                    sum += pixels[idx];
                    count += 1;
                }
            }
            let value = if count == 0 { 0.0 } else { sum / count as f32 };
            output[ny * new_width + nx] = value;
        }
    }
    output
}

pub fn gaussian_blur_3x3(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
    assert_eq!(pixels.len(), width * height);
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let kernel = [[1.0f32, 2.0, 1.0], [2.0, 4.0, 2.0], [1.0, 2.0, 1.0]];
    let mut output = vec![0.0f32; pixels.len()];
    for y in 0..height {
        for x in 0..width {
            let mut sum = 0.0;
            let mut weight = 0.0;
            for ky in 0..3 {
                for kx in 0..3 {
                    let oy = y as isize + ky as isize - 1;
                    let ox = x as isize + kx as isize - 1;
                    if oy < 0 || ox < 0 || oy >= height as isize || ox >= width as isize {
                        continue;
                    }
                    let idx = oy as usize * width + ox as usize;
                    let w = kernel[ky][kx];
                    sum += pixels[idx] * w;
                    weight += w;
                }
            }
            output[y * width + x] = if weight == 0.0 { 0.0 } else { sum / weight };
        }
    }
    output
}

pub fn sobel_magnitude_into(pixels: &[f32], width: usize, height: usize, output: &mut Vec<f32>) {
    assert_eq!(pixels.len(), width * height);
    output.clear();
    if width == 0 || height == 0 {
        return;
    }
    output.resize(pixels.len(), 0.0);
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let idx = y * width + x;
            let gx = pixels[(y - 1) * width + (x + 1)]
                + 2.0 * pixels[y * width + (x + 1)]
                + pixels[(y + 1) * width + (x + 1)]
                - pixels[(y - 1) * width + (x - 1)]
                - 2.0 * pixels[y * width + (x - 1)]
                - pixels[(y + 1) * width + (x - 1)];
            let gy = pixels[(y + 1) * width + (x - 1)]
                + 2.0 * pixels[(y + 1) * width + x]
                + pixels[(y + 1) * width + (x + 1)]
                - pixels[(y - 1) * width + (x - 1)]
                - 2.0 * pixels[(y - 1) * width + x]
                - pixels[(y - 1) * width + (x + 1)];
            output[idx] = gx.abs() + gy.abs();
        }
    }
}

pub fn sobel_magnitude(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut output = Vec::new();
    sobel_magnitude_into(pixels, width, height, &mut output);
    output
}

pub fn normalize(values: &mut [f32]) {
    if values.is_empty() {
        return;
    }
    let mut max_value = values[0];
    for &v in values.iter().skip(1) {
        if v > max_value {
            max_value = v;
        }
    }
    if max_value <= f32::EPSILON {
        return;
    }
    for value in values.iter_mut() {
        *value /= max_value;
    }
}

pub fn percentile_in_place(values: &mut [f32], pct: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let len = values.len();
    let target = ((len - 1) as f32 * pct.clamp(0.0, 1.0)).round() as usize;
    let (_, value, _) =
        values.select_nth_unstable_by(target, |a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    *value
}

pub fn percentile(values: &[f32], pct: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut buf: Vec<f32> = values.to_vec();
    percentile_in_place(&mut buf, pct)
}

pub fn distance_transform(edge_map: &[u8], width: usize, height: usize) -> Vec<f32> {
    assert_eq!(edge_map.len(), width * height);
    let mut dist = vec![f32::MAX; edge_map.len()];
    for (idx, &value) in edge_map.iter().enumerate() {
        if value > 0 {
            dist[idx] = 0.0;
        }
    }

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if dist[idx] == 0.0 {
                continue;
            }
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
    dist
}

pub fn dilate_binary(mask: &[u8], width: usize, height: usize, iterations: usize) -> Vec<u8> {
    assert_eq!(mask.len(), width * height);
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

pub fn erode_binary(mask: &[u8], width: usize, height: usize, iterations: usize) -> Vec<u8> {
    assert_eq!(mask.len(), width * height);
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

pub fn dct2(input: &[f32], width: usize, height: usize) -> Vec<f32> {
    assert_eq!(input.len(), width * height);
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let mut rows = vec![0.0f32; width * height];
    for y in 0..height {
        for u in 0..width {
            let mut sum = 0.0f32;
            for x in 0..width {
                let angle = std::f32::consts::PI / width as f32 * (x as f32 + 0.5) * u as f32;
                sum += input[y * width + x] * angle.cos();
            }
            rows[y * width + u] = sum;
        }
    }
    let mut output = vec![0.0f32; width * height];
    for x in 0..width {
        for v in 0..height {
            let mut sum = 0.0f32;
            for y in 0..height {
                let angle = std::f32::consts::PI / height as f32 * (y as f32 + 0.5) * v as f32;
                sum += rows[y * width + x] * angle.cos();
            }
            output[v * width + x] = sum;
        }
    }
    output
}

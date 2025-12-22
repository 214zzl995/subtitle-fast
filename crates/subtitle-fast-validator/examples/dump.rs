use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::thread;

use image::{Rgb, RgbImage};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use serde_json::json;
use subtitle_fast_types::VideoFrame;
#[cfg(all(feature = "detector-vision", target_os = "macos"))]
use subtitle_fast_validator::subtitle_detection::VisionTextDetector;
use subtitle_fast_validator::subtitle_detection::projection_band::ProjectionBandDetector;
use subtitle_fast_validator::subtitle_detection::{
    DetectionRegion, IntegralBandDetector, LumaBandConfig, RoiConfig, SubtitleDetectionConfig,
    SubtitleDetectionError, SubtitleDetector,
};

const TARGET: u8 = 235;
const DELTA: u8 = 12;
const PRESETS: &[(usize, usize)] = &[(1920, 1080), (1920, 824)];
const YUV_DIR: &str = "./demo/decoder/yuv";
const OUT_DIR: &str = "./demo/validator";
#[cfg(all(feature = "detector-vision", target_os = "macos"))]
const DETECTORS: &[&str] = &["integral", "projection", "vision"];
#[cfg(not(all(feature = "detector-vision", target_os = "macos")))]
const DETECTORS: &[&str] = &["integral", "projection"];

const DIGIT_WIDTH: i32 = 3;
const DIGIT_HEIGHT: i32 = 5;
const LABEL_SCALE: i32 = 5;
const LABEL_SPACING: i32 = LABEL_SCALE;

fn main() -> Result<(), Box<dyn Error>> {
    let yuv_dir = PathBuf::from(YUV_DIR);
    if !yuv_dir.exists() {
        return Err(format!("missing {:?}", yuv_dir).into());
    }

    let mut frames = Vec::new();
    for entry in fs::read_dir(&yuv_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("yuv") {
            frames.push(path);
        }
    }

    if frames.is_empty() {
        return Err("no demo frames processed".into());
    }

    let total_frames = frames.len();
    let progress = MultiProgress::new();
    let style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] {prefix:>10.cyan.bold} \
{bar:40.cyan/blue} {pos:>4}/{len:4} frames",
    )
    .unwrap()
    .progress_chars("█▉▊▋▌▍▎▏  ");
    let mut handles = Vec::new();

    for detector_name in DETECTORS {
        let frames = frames.clone();
        let detector_name = detector_name.to_string();

        let bar = progress.add(ProgressBar::new(total_frames as u64));
        bar.set_style(style.clone());
        bar.set_prefix(detector_name.clone());

        let handle = thread::spawn(move || -> Result<usize, Box<dyn Error + Send + Sync>> {
            let out_dir = PathBuf::from(OUT_DIR).join(&detector_name);
            fs::create_dir_all(&out_dir)?;

            let mut processed = 0usize;
            for path in frames {
                let data = fs::read(&path)?;
                let (width, height) = match resolution_from_len(data.len()) {
                    Some(dim) => dim,
                    None => {
                        eprintln!(
                            "skipping {:?}: unknown resolution ({} bytes)",
                            path,
                            data.len()
                        );
                        continue;
                    }
                };
                let y_len = width * height;
                let uv_rows = (height + 1) / 2;
                let uv_len = width * uv_rows;
                let y_plane = data[..y_len].to_vec();
                let uv_plane = if data.len() >= y_len + uv_len {
                    data[y_len..y_len + uv_len].to_vec()
                } else {
                    vec![128u8; uv_len]
                };
                let frame = VideoFrame::from_nv12_owned(
                    width as u32,
                    height as u32,
                    width,
                    width,
                    None,
                    y_plane,
                    uv_plane,
                )?;
                let mut config = SubtitleDetectionConfig::for_frame(width, height, width);
                config.roi = RoiConfig {
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: 1.0,
                };
                config.luma_band = LumaBandConfig {
                    target: TARGET,
                    delta: DELTA,
                };
                let roi = config.roi;
                let detector = build_detector(&detector_name, config)?;
                let result = detector.detect(&frame)?;

                let mut image = frame_to_image(&frame);
                overlay_regions(&mut image, &result.regions);

                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("frame");
                let out_path = out_dir.join(format!("{stem}.png"));
                image.save(out_path)?;

                let regions_with_index: Vec<_> = result
                    .regions
                    .iter()
                    .enumerate()
                    .map(|(index, region)| {
                        json!({
                            "index": index,
                            "x": region.x,
                            "y": region.y,
                            "width": region.width,
                            "height": region.height,
                            "score": region.score,
                        })
                    })
                    .collect();

                let report = json!({
                    "detector": detector_name,
                    "source": path.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
                    "frame": { "width": width, "height": height },
                    "roi": { "x": roi.x, "y": roi.y, "width": roi.width, "height": roi.height },
                    "luma_band": { "target": TARGET, "delta": DELTA },
                    "has_subtitle": result.has_subtitle,
                    "max_score": result.max_score,
                    "regions": regions_with_index,
                });
                let json_path = out_dir.join(format!("{stem}.json"));
                fs::write(json_path, serde_json::to_vec_pretty(&report)?)?;

                processed += 1;
                bar.inc(1);
            }

            bar.finish_with_message("done");

            Ok(processed)
        });

        handles.push(handle);
    }

    let mut any_processed = false;
    for handle in handles {
        match handle.join() {
            Ok(Ok(count)) => {
                if count > 0 {
                    any_processed = true;
                }
            }
            Ok(Err(err)) => {
                return Err(err);
            }
            Err(_) => {
                return Err("detector worker panicked".into());
            }
        }
    }

    if !any_processed {
        return Err("no demo frames processed".into());
    }

    Ok(())
}

fn resolution_from_len(len: usize) -> Option<(usize, usize)> {
    PRESETS.iter().copied().find(|(w, h)| {
        let y_len = w * h;
        let uv_rows = (h + 1) / 2;
        let uv_len = w * uv_rows;
        len == y_len || len == y_len + uv_len
    })
}

fn frame_to_image(frame: &VideoFrame) -> RgbImage {
    let width = frame.width();
    let height = frame.height();
    let stride = frame.stride();
    let data = frame.data();
    RgbImage::from_fn(width, height, |x, y| {
        let idx = y as usize * stride + x as usize;
        let v = data[idx];
        Rgb([v, v, v])
    })
}

fn overlay_regions(image: &mut RgbImage, regions: &[DetectionRegion]) {
    for (index, region) in regions.iter().enumerate() {
        draw_box(image, region);
        draw_label(image, region, index);
    }
}

fn draw_box(image: &mut RgbImage, region: &DetectionRegion) {
    let width = image.width() as f32;
    let height = image.height() as f32;
    let x0 = region.x.max(0.0).min(width - 1.0) as i32;
    let y0 = region.y.max(0.0).min(height - 1.0) as i32;
    let x1 = (region.x + region.width).max(0.0).min(width - 1.0) as i32;
    let y1 = (region.y + region.height).max(0.0).min(height - 1.0) as i32;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    for x in x0..=x1 {
        set_pixel(image, x, y0);
        set_pixel(image, x, y1);
    }
    for y in y0..=y1 {
        set_pixel(image, x0, y);
        set_pixel(image, x1, y);
    }
}

fn draw_label(image: &mut RgbImage, region: &DetectionRegion, index: usize) {
    let label = index.to_string();
    let width = image.width() as i32;
    let height = image.height() as i32;
    let digit_w = DIGIT_WIDTH * LABEL_SCALE;
    let digit_h = DIGIT_HEIGHT * LABEL_SCALE;
    let total_width = label.len() as i32 * (digit_w + LABEL_SPACING) - LABEL_SPACING;
    let mut x = region.x.max(0.0) as i32 + 2;
    let mut y = region.y.max(0.0) as i32 + 2;

    if y + digit_h >= height {
        y = (height - (digit_h + 1)).max(0);
    }
    if x + total_width >= width {
        x = (width - (total_width + 1)).max(0);
    }

    fill_rect(
        image,
        x - 1,
        y - 1,
        total_width + 2,
        digit_h + 2,
        Rgb([0, 0, 0]),
    );

    for ch in label.chars() {
        draw_digit(image, x, y, ch);
        x += digit_w + LABEL_SPACING;
    }
}

fn draw_digit(image: &mut RgbImage, x: i32, y: i32, ch: char) {
    let bitmap: [[u8; 3]; 5] = match ch {
        '0' => [[1, 1, 1], [1, 0, 1], [1, 0, 1], [1, 0, 1], [1, 1, 1]],
        '1' => [[0, 1, 0], [1, 1, 0], [0, 1, 0], [0, 1, 0], [1, 1, 1]],
        '2' => [[1, 1, 1], [0, 0, 1], [1, 1, 1], [1, 0, 0], [1, 1, 1]],
        '3' => [[1, 1, 1], [0, 0, 1], [0, 1, 1], [0, 0, 1], [1, 1, 1]],
        '4' => [[1, 0, 1], [1, 0, 1], [1, 1, 1], [0, 0, 1], [0, 0, 1]],
        '5' => [[1, 1, 1], [1, 0, 0], [1, 1, 1], [0, 0, 1], [1, 1, 1]],
        '6' => [[1, 1, 1], [1, 0, 0], [1, 1, 1], [1, 0, 1], [1, 1, 1]],
        '7' => [[1, 1, 1], [0, 0, 1], [0, 1, 0], [0, 1, 0], [0, 1, 0]],
        '8' => [[1, 1, 1], [1, 0, 1], [1, 1, 1], [1, 0, 1], [1, 1, 1]],
        '9' => [[1, 1, 1], [1, 0, 1], [1, 1, 1], [0, 0, 1], [1, 1, 1]],
        _ => [[0, 0, 0]; 5],
    };
    let color = Rgb([255, 255, 0]);
    for (dy, row) in bitmap.iter().enumerate() {
        for (dx, &on) in row.iter().enumerate() {
            if on == 1 {
                for sy in 0..LABEL_SCALE {
                    for sx in 0..LABEL_SCALE {
                        set_pixel_color(
                            image,
                            x + dx as i32 * LABEL_SCALE + sx,
                            y + dy as i32 * LABEL_SCALE + sy,
                            color,
                        );
                    }
                }
            }
        }
    }
}

fn set_pixel_color(image: &mut RgbImage, x: i32, y: i32, color: Rgb<u8>) {
    if x < 0 || y < 0 {
        return;
    }
    let (x, y) = (x as u32, y as u32);
    if x < image.width() && y < image.height() {
        image.put_pixel(x, y, color);
    }
}

fn fill_rect(image: &mut RgbImage, x: i32, y: i32, width: i32, height: i32, color: Rgb<u8>) {
    for dy in 0..height {
        for dx in 0..width {
            set_pixel_color(image, x + dx, y + dy, color);
        }
    }
}

fn set_pixel(image: &mut RgbImage, x: i32, y: i32) {
    if x < 0 || y < 0 {
        return;
    }
    let (x, y) = (x as u32, y as u32);
    if x < image.width() && y < image.height() {
        image.put_pixel(x, y, Rgb([255, 0, 0]));
    }
}

fn build_detector(
    name: &str,
    config: SubtitleDetectionConfig,
) -> Result<Box<dyn SubtitleDetector>, SubtitleDetectionError> {
    match name {
        "integral" => Ok(Box::new(IntegralBandDetector::new(config)?)),
        "projection" => Ok(Box::new(ProjectionBandDetector::new(config)?)),
        #[cfg(all(feature = "detector-vision", target_os = "macos"))]
        "vision" => Ok(Box::new(VisionTextDetector::new(config)?)),
        #[cfg(not(all(feature = "detector-vision", target_os = "macos")))]
        "vision" => Err(SubtitleDetectionError::Unsupported {
            backend: "vision-detector",
        }),
        other => {
            eprintln!("unknown detector '{other}', defaulting to unsupported error");
            Err(SubtitleDetectionError::Unsupported {
                backend: "unknown-detector",
            })
        }
    }
}

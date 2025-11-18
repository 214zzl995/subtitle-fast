use std::error::Error;
use std::fs;
use std::path::PathBuf;

use image::{Rgb, RgbImage};
use serde_json::json;
use subtitle_fast_decoder::YPlaneFrame;
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
const DETECTORS: &[&str] = &["integral", "projection"];

fn main() -> Result<(), Box<dyn Error>> {
    let yuv_dir = PathBuf::from(YUV_DIR);
    if !yuv_dir.exists() {
        return Err(format!("missing {:?}", yuv_dir).into());
    }

    let mut processed_total = 0usize;
    for detector_name in DETECTORS {
        let out_dir = PathBuf::from(OUT_DIR).join(detector_name);
        fs::create_dir_all(&out_dir)?;

        let mut processed = 0usize;
        for entry in fs::read_dir(&yuv_dir)? {
            let path = entry?.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("yuv") {
                continue;
            }
            println!("processing {:?} with {detector_name}", path);
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

            let frame = YPlaneFrame::from_owned(width as u32, height as u32, width, None, data)?;
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
            let detector = build_detector(detector_name, config)?;
            let result = detector.detect(&frame)?;

            let mut image = frame_to_image(&frame);
            overlay_regions(&mut image, &result.regions);

            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("frame");
            let out_path = out_dir.join(format!("{stem}.png"));
            image.save(out_path)?;

            let report = json!({
                "detector": detector_name,
                "source": path.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
                "frame": { "width": width, "height": height },
                "roi": { "x": roi.x, "y": roi.y, "width": roi.width, "height": roi.height },
                "luma_band": { "target": TARGET, "delta": DELTA },
                "has_subtitle": result.has_subtitle,
                "max_score": result.max_score,
                "regions": result.regions,
            });
            let json_path = out_dir.join(format!("{stem}.json"));
            fs::write(json_path, serde_json::to_vec_pretty(&report)?)?;

            processed += 1;
            processed_total += 1;
        }

        if processed == 0 {
            return Err("no demo frames processed".into());
        }

        println!(
            "Generated {processed} PNG files in {:?} using {}",
            out_dir, detector_name
        );
    }

    if processed_total == 0 {
        return Err("no demo frames processed".into());
    }

    Ok(())
}

fn resolution_from_len(len: usize) -> Option<(usize, usize)> {
    PRESETS.iter().copied().find(|(w, h)| w * h == len)
}

fn frame_to_image(frame: &YPlaneFrame) -> RgbImage {
    let width = frame.width() as u32;
    let height = frame.height() as u32;
    let stride = frame.stride();
    let data = frame.data();
    RgbImage::from_fn(width, height, |x, y| {
        let idx = y as usize * stride + x as usize;
        let v = data[idx];
        Rgb([v, v, v])
    })
}

fn overlay_regions(image: &mut RgbImage, regions: &[DetectionRegion]) {
    for region in regions {
        draw_box(image, region);
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
        other => {
            eprintln!("unknown detector '{other}', defaulting to unsupported error");
            Err(SubtitleDetectionError::Unsupported {
                backend: "unknown-detector",
            })
        }
    }
}

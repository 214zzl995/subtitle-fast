use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use image::{Rgb, RgbImage};
use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_validator::subtitle_detection::projection_band::ProjectionBandDetector;
use subtitle_fast_validator::subtitle_detection::{
    DetectionRegion, IntegralBandDetector, LumaBandConfig, RoiConfig, SubtitleDetectionConfig,
    SubtitleDetectionError, SubtitleDetector,
};

const TARGET: u8 = 235;
const DELTA: u8 = 12;
const PRESETS: &[(usize, usize)] = &[(1920, 1080), (1920, 824)];

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let detector_kind = parse_detector_arg(&args)?;
    let repo_root = repo_root();
    let yuv_dir = repo_root.join("demo/yuv");
    let out_dir = repo_root.join("demo/output").join(detector_kind.name());
    if !yuv_dir.exists() {
        return Err(format!("missing {:?}", yuv_dir).into());
    }
    fs::create_dir_all(&out_dir)?;

    let mut processed = 0usize;
    for entry in fs::read_dir(&yuv_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("yuv") {
            continue;
        }
        println!("processing {:?}", path);
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
        let detector = detector_kind.build(config)?;
        let result = detector.detect(&frame)?;

        let mut image = frame_to_image(&frame);
        overlay_regions(&mut image, &result.regions);

        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("frame");
        let out_path = out_dir.join(format!("{stem}.png"));
        image.save(out_path)?;
        processed += 1;
    }

    if processed == 0 {
        return Err("no demo frames processed".into());
    }

    println!(
        "Generated {processed} PNG files in {:?} using {}",
        out_dir,
        detector_kind.name()
    );
    Ok(())
}

fn parse_detector_arg(args: &[String]) -> Result<DemoDetectorKind, Box<dyn Error>> {
    let mut detector = DemoDetectorKind::Projection;
    let mut idx = 1usize;
    while idx < args.len() {
        let arg = &args[idx];
        if let Some(value) = arg.strip_prefix("--detector=") {
            detector = DemoDetectorKind::from_str(value)?;
        } else if arg == "--detector" {
            idx += 1;
            if idx >= args.len() {
                return Err("--detector requires a value".into());
            }
            detector = DemoDetectorKind::from_str(&args[idx])?;
        } else {
            return Err(format!("unknown argument: {arg}").into());
        }
        idx += 1;
    }
    Ok(detector)
}

fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
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

#[derive(Clone, Copy)]
enum DemoDetectorKind {
    Integral,
    Projection,
}

impl DemoDetectorKind {
    fn from_str(value: &str) -> Result<Self, Box<dyn Error>> {
        match value {
            "integral" => Ok(Self::Integral),
            "projection" => Ok(Self::Projection),
            other => Err(format!("unknown detector '{other}'").into()),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Integral => "integral",
            Self::Projection => "projection",
        }
    }

    fn build(
        &self,
        config: SubtitleDetectionConfig,
    ) -> Result<Box<dyn SubtitleDetector>, SubtitleDetectionError> {
        match self {
            Self::Integral => Ok(Box::new(IntegralBandDetector::new(config)?)),
            Self::Projection => Ok(Box::new(ProjectionBandDetector::new(config)?)),
        }
    }
}

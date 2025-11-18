use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_validator::subtitle_detection::projection_band::ProjectionBandDetector;
use subtitle_fast_validator::subtitle_detection::{
    IntegralBandDetector, LumaBandConfig, RoiConfig, SubtitleDetectionConfig,
    SubtitleDetectionError, SubtitleDetectionResult, SubtitleDetector,
};

const TARGET: u8 = 235;
const DELTA: u8 = 12;
const PRESETS: &[(usize, usize)] = &[(1920, 1080), (1920, 824)];
const YUV_DIR: &str = "./demo/decoder/yuv";
const DETECTORS: &[&str] = &["integral", "projection"];

fn main() -> Result<(), Box<dyn Error>> {
    let yuv_dir = PathBuf::from(YUV_DIR);
    if !yuv_dir.exists() {
        return Err(format!("missing {:?}", yuv_dir).into());
    }
    let detectors: Vec<DetectorKind> = DETECTORS
        .iter()
        .map(|name| {
            DetectorKind::from_str(name).unwrap_or_else(|| {
                panic!("unknown detector '{name}' in DETECTORS constant");
            })
        })
        .collect();

    let mut stats: HashMap<&'static str, DetectorStats> = HashMap::new();
    let mut projection_perf = ProjectionPerf::default();
    let mut processed = 0usize;
    for entry in fs::read_dir(&yuv_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("yuv") {
            continue;
        }
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

        println!("processing {:?}", path);
        let mut baseline: Option<SubtitleDetectionResult> = None;
        for detector_kind in &detectors {
            let detector = detector_kind.build(&config)?;
            let start = Instant::now();
            let result = detector.detect(&frame)?;
            let duration = start.elapsed();
            if let Err(err) = validate_regions(&result, width, height) {
                eprintln!(
                    "{} produced invalid regions on {:?}: {}",
                    detector_kind.name(),
                    path,
                    err
                );
            }
            if matches!(detector_kind, DetectorKind::Projection) {
                baseline = Some(result.clone());
            } else if let Some(base) = &baseline {
                if base.has_subtitle && !result.has_subtitle {
                    eprintln!("{:?} missed subtitles compared to projection", path);
                }
            }
            let entry = stats.entry(detector_kind.name()).or_default();
            entry.record(duration);
            if matches!(detector_kind, DetectorKind::Projection) {
                projection_perf.record(duration);
            }
        }
        processed += 1;
    }

    if processed == 0 {
        return Err("no demo frames processed".into());
    }

    println!("\nBenchmark summary over {processed} frames:");
    for detector_kind in &detectors {
        if let Some(stat) = stats.get(detector_kind.name()) {
            println!(
                "{:>12}: avg={:.3}ms frames={}",
                detector_kind.name(),
                stat.avg_ms(),
                stat.frames,
            );
        }
    }
    if projection_perf.frames > 0 {
        projection_perf.print_report();
    }

    Ok(())
}

fn resolution_from_len(len: usize) -> Option<(usize, usize)> {
    PRESETS.iter().copied().find(|(w, h)| w * h == len)
}

fn validate_regions(
    result: &SubtitleDetectionResult,
    width: usize,
    height: usize,
) -> Result<(), String> {
    for region in &result.regions {
        if region.width <= 0.0 || region.height <= 0.0 {
            return Err("region has non-positive dimensions".into());
        }
        if region.x < 0.0
            || region.y < 0.0
            || region.x + region.width > width as f32
            || region.y + region.height > height as f32
        {
            return Err("region exceeds frame bounds".into());
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum DetectorKind {
    Integral,
    Projection,
}

impl DetectorKind {
    fn from_str(name: &str) -> Option<Self> {
        match name {
            "integral" => Some(Self::Integral),
            "projection" => Some(Self::Projection),
            _ => None,
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
        config: &SubtitleDetectionConfig,
    ) -> Result<Box<dyn SubtitleDetector>, SubtitleDetectionError> {
        match self {
            Self::Integral => Ok(Box::new(IntegralBandDetector::new(config.clone())?)),
            Self::Projection => Ok(Box::new(ProjectionBandDetector::new(config.clone())?)),
        }
    }
}

#[derive(Default)]
struct DetectorStats {
    total: Duration,
    frames: usize,
}

impl DetectorStats {
    fn record(&mut self, duration: Duration) {
        self.total += duration;
        self.frames += 1;
    }

    fn avg_ms(&self) -> f64 {
        if self.frames == 0 {
            0.0
        } else {
            self.total.as_secs_f64() * 1000.0 / self.frames as f64
        }
    }
}

#[derive(Default)]
struct ProjectionPerf {
    total: Duration,
    frames: u64,
}

impl ProjectionPerf {
    fn record(&mut self, duration: Duration) {
        self.frames += 1;
        self.total += duration;
    }

    fn print_report(&self) {
        if self.frames == 0 {
            return;
        }
        let avg_ms = (self.total.as_secs_f64() * 1000.0) / self.frames as f64;
        eprintln!(
            "[projection][bench-perf] frames={} avg={:.3}ms",
            self.frames, avg_ms
        );
    }
}

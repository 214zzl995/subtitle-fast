use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_validator::subtitle_detection::projection_band::ProjectionBandDetector;
use subtitle_fast_validator::subtitle_detection::{
    IntegralBandDetector, LumaBandConfig, RoiConfig, SubtitleDetectionConfig,
    SubtitleDetectionError, SubtitleDetector, VisionTextDetector,
};

const TARGET: u8 = 235;
const DELTA: u8 = 12;
const PRESETS: &[(usize, usize)] = &[(1920, 1080), (1920, 824)];
const YUV_DIR: &str = "./demo/decoder/yuv";
const DETECTORS: &[&str] = &["integral", "projection", "vision"];

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

    let detectors: Vec<DetectorKind> = DETECTORS
        .iter()
        .map(|name| {
            DetectorKind::from_str(name).unwrap_or_else(|| {
                panic!("unknown detector '{name}' in DETECTORS constant");
            })
        })
        .collect();

    let total_frames = frames.len() as u64;
    let multi = MultiProgress::new();
    let style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] {prefix:>10.magenta.bold} \
{bar:40.magenta/blue} {pos:>4}/{len:4} frames avg={msg}ms",
    )
    .unwrap()
    .progress_chars("█▉▊▋▌▍▎▏  ");

    let mut handles = Vec::new();
    for detector_kind in &detectors {
        let frames = frames.clone();
        let kind = *detector_kind;

        let bar = multi.add(ProgressBar::new(total_frames));
        bar.set_style(style.clone());
        bar.set_prefix(kind.name().to_string());
        bar.set_message("0.000");

        let handle = thread::spawn(move || -> Result<(DetectorKind, DetectorStats, ProjectionPerf), Box<dyn Error + Send + Sync>> {
            let mut stats = DetectorStats::default();
            let mut projection_perf = ProjectionPerf::default();

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
                let frame =
                    YPlaneFrame::from_owned(width as u32, height as u32, width, None, data)?;
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

                let detector = kind.build(&config)?;
                let start = Instant::now();
                let _result = detector.detect(&frame)?;
                let duration = start.elapsed();

                stats.record(duration);
                if matches!(kind, DetectorKind::Projection) {
                    projection_perf.record(duration);
                }

                bar.inc(1);
                bar.set_message(format!("{:.3}", stats.avg_ms()));
            }

            bar.finish_with_message("done");

            Ok((kind, stats, projection_perf))
        });

        handles.push(handle);
    }

    let mut any_processed = false;
    let mut stats: HashMap<&'static str, DetectorStats> = HashMap::new();
    let mut projection_perf = ProjectionPerf::default();

    for handle in handles {
        match handle.join() {
            Ok(Ok((kind, detector_stats, worker_projection))) => {
                if detector_stats.frames > 0 {
                    any_processed = true;
                }
                stats.insert(kind.name(), detector_stats);

                if matches!(kind, DetectorKind::Projection) {
                    projection_perf.total += worker_projection.total;
                    projection_perf.frames += worker_projection.frames;
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

    println!("\nBenchmark summary over {total_frames} frames:");
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

#[derive(Clone, Copy)]
enum DetectorKind {
    Integral,
    Projection,
    Vision,
}

impl DetectorKind {
    fn from_str(name: &str) -> Option<Self> {
        match name {
            "integral" => Some(Self::Integral),
            "projection" => Some(Self::Projection),
            "vision" => Some(Self::Vision),
            _ => None,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Integral => "integral",
            Self::Projection => "projection",
            Self::Vision => "vision",
        }
    }

    fn build(
        &self,
        config: &SubtitleDetectionConfig,
    ) -> Result<Box<dyn SubtitleDetector>, SubtitleDetectionError> {
        match self {
            Self::Integral => Ok(Box::new(IntegralBandDetector::new(config.clone())?)),
            Self::Projection => Ok(Box::new(ProjectionBandDetector::new(config.clone())?)),
            Self::Vision => Ok(Box::new(VisionTextDetector::new(config.clone())?)),
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

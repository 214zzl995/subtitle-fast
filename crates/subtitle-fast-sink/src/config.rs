use std::path::PathBuf;
use std::time::Duration;

use crate::subtitle_detection::{
    RoiConfig, SubtitleDetectorKind, DEFAULT_LUMA_DELTA, DEFAULT_LUMA_TARGET,
};

const DEFAULT_CHANNEL_CAPACITY: usize = 64;
const DEFAULT_MAX_CONCURRENCY: usize = 16;

#[derive(Clone, Debug)]
pub struct FrameSinkConfig {
    pub channel_capacity: usize,
    pub max_concurrency: usize,
    pub dump: Option<FrameDumpConfig>,
    pub detection: SubtitleDetectionOptions,
}

impl Default for FrameSinkConfig {
    fn default() -> Self {
        Self {
            channel_capacity: DEFAULT_CHANNEL_CAPACITY,
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
            dump: None,
            detection: SubtitleDetectionOptions::default(),
        }
    }
}

impl FrameSinkConfig {
    pub fn from_outputs(
        dump_dir: Option<PathBuf>,
        format: ImageOutputFormat,
        samples_per_second: u32,
    ) -> Self {
        let mut config = Self::default();
        if let Some(dir) = dump_dir {
            config.dump = Some(FrameDumpConfig::new(dir, format, samples_per_second));
        }
        config.detection.samples_per_second = samples_per_second.max(1);
        config
    }
}

#[derive(Clone, Debug)]
pub struct FrameDumpConfig {
    pub directory: PathBuf,
    pub format: ImageOutputFormat,
    pub samples_per_second: u32,
}

impl FrameDumpConfig {
    pub fn new(directory: PathBuf, format: ImageOutputFormat, samples_per_second: u32) -> Self {
        Self {
            directory,
            format,
            samples_per_second: samples_per_second.max(1),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub enum ImageOutputFormat {
    Jpeg { quality: u8 },
    Png,
    Webp,
    Yuv,
}

#[derive(Clone, Debug)]
pub struct SubtitleDetectionOptions {
    pub enabled: bool,
    pub samples_per_second: u32,
    pub detector: SubtitleDetectorKind,
    pub onnx_model_path: Option<PathBuf>,
    pub roi_override: Option<RoiConfig>,
    pub dump_json: bool,
    pub luma_band: LumaBandOptions,
}

impl Default for SubtitleDetectionOptions {
    fn default() -> Self {
        let detector = if cfg!(target_os = "macos") {
            SubtitleDetectorKind::MacVision
        } else {
            SubtitleDetectorKind::OnnxPpocr
        };
        Self {
            enabled: true,
            samples_per_second: 7,
            detector,
            onnx_model_path: None,
            roi_override: None,
            dump_json: true,
            luma_band: LumaBandOptions::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LumaBandOptions {
    pub target_luma: u8,
    pub delta: u8,
}

impl Default for LumaBandOptions {
    fn default() -> Self {
        Self {
            target_luma: DEFAULT_LUMA_TARGET,
            delta: DEFAULT_LUMA_DELTA,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FrameMetadata {
    pub frame_index: u64,
    pub decoder_frame_index: Option<u64>,
    pub processed_index: u64,
    pub timestamp: Option<Duration>,
}

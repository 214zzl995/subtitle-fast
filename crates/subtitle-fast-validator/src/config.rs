use std::path::PathBuf;
use std::time::Duration;

use crate::subtitle_detection::{
    RoiConfig, SubtitleDetectorKind, DEFAULT_LUMA_DELTA, DEFAULT_LUMA_TARGET,
};

#[derive(Clone, Debug)]
pub struct FrameValidatorConfig {
    pub detection: SubtitleDetectionOptions,
}

impl Default for FrameValidatorConfig {
    fn default() -> Self {
        Self {
            detection: SubtitleDetectionOptions::default(),
        }
    }
}

impl FrameValidatorConfig {
    pub fn from_outputs(
        dump_dir: Option<PathBuf>,
        format: ImageOutputFormat,
        samples_per_second: u32,
    ) -> Self {
        let mut config = Self::default();
        config.detection.samples_per_second = samples_per_second.max(1);
        if let Some(dir) = dump_dir {
            config.detection.frame_dump = Some(FrameDumpConfig::new(dir, format));
        }
        config
    }
}

#[derive(Clone, Debug)]
pub struct FrameDumpConfig {
    pub directory: PathBuf,
    pub format: ImageOutputFormat,
}

impl FrameDumpConfig {
    pub fn new(directory: PathBuf, format: ImageOutputFormat) -> Self {
        Self { directory, format }
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
    pub frame_dump: Option<FrameDumpConfig>,
}

impl Default for SubtitleDetectionOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            samples_per_second: 7,
            detector: SubtitleDetectorKind::LumaBand,
            onnx_model_path: None,
            roi_override: None,
            dump_json: true,
            luma_band: LumaBandOptions::default(),
            frame_dump: None,
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

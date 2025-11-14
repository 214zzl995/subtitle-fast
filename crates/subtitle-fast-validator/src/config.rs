use crate::subtitle_detection::{RoiConfig, SubtitleDetectorKind, DEFAULT_DELTA, DEFAULT_TARGET};

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

#[derive(Clone, Debug)]
pub struct SubtitleDetectionOptions {
    pub enabled: bool,
    pub roi: Option<RoiConfig>,
    pub detector: SubtitleDetectorKind,
    pub luma_band: LumaBandOptions,
}

impl Default for SubtitleDetectionOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            roi: None,
            detector: SubtitleDetectorKind::IntegralBand,
            luma_band: LumaBandOptions::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LumaBandOptions {
    pub target: u8,
    pub delta: u8,
}

impl Default for LumaBandOptions {
    fn default() -> Self {
        Self {
            target: DEFAULT_TARGET,
            delta: DEFAULT_DELTA,
        }
    }
}

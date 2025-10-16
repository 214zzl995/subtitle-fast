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

#[derive(Clone, Debug)]
pub struct SubtitleDetectionOptions {
    pub enabled: bool,
    pub detector: SubtitleDetectorKind,
    pub roi: Option<RoiConfig>,
    pub luma_band: LumaBandOptions,
}

impl Default for SubtitleDetectionOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            detector: SubtitleDetectorKind::LumaBand,
            roi: None,
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

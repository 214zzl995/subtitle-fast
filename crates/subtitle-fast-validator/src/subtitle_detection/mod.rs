use std::path::{Path, PathBuf};

use crate::config::FrameMetadata;
use serde::Serialize;
use thiserror::Error;

pub mod luma_band;
pub use luma_band::LumaBandDetector;

#[cfg(feature = "detector-onnx")]
pub mod onnx;
#[cfg(feature = "detector-onnx")]
pub use onnx::OnnxPpocrDetector;

#[cfg(all(feature = "detector-vision", target_os = "macos"))]
pub mod vision;
#[cfg(all(feature = "detector-vision", target_os = "macos"))]
pub use vision::VisionTextDetector;

pub const DEFAULT_LUMA_TARGET: u8 = 230;
pub const DEFAULT_LUMA_DELTA: u8 = 12;

#[cfg(target_os = "macos")]
const AUTO_DETECTOR_PRIORITY: &[SubtitleDetectorKind] = &[SubtitleDetectorKind::LumaBand];

#[cfg(not(target_os = "macos"))]
const AUTO_DETECTOR_PRIORITY: &[SubtitleDetectorKind] = &[SubtitleDetectorKind::LumaBand];

fn backend_for_kind(kind: SubtitleDetectorKind) -> Option<&'static dyn DetectorBackend> {
    match kind {
        SubtitleDetectorKind::Auto => None,
        SubtitleDetectorKind::OnnxPpocr => {
            #[cfg(feature = "detector-onnx")]
            {
                return Some(&ONNX_BACKEND);
            }
            #[cfg(not(feature = "detector-onnx"))]
            {
                return None;
            }
        }
        SubtitleDetectorKind::MacVision => {
            #[cfg(all(feature = "detector-vision", target_os = "macos"))]
            {
                return Some(&VISION_BACKEND);
            }
            #[cfg(not(all(feature = "detector-vision", target_os = "macos")))]
            {
                return None;
            }
        }
        SubtitleDetectorKind::LumaBand => Some(&LUMA_BAND_BACKEND),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RoiConfig {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct DetectionRegion {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubtitleDetectionResult {
    pub has_subtitle: bool,
    pub max_score: f32,
    pub regions: Vec<DetectionRegion>,
}

#[derive(Debug, Clone, Copy)]
pub struct LumaBandConfig {
    pub target_luma: u8,
    pub delta: u8,
}

trait DetectorBackend {
    fn kind(&self) -> SubtitleDetectorKind;
    fn ensure_available(
        &self,
        config: &SubtitleDetectionConfig,
    ) -> Result<(), SubtitleDetectionError>;
    fn build(
        &self,
        config: SubtitleDetectionConfig,
    ) -> Result<Box<dyn SubtitleDetector>, SubtitleDetectionError>;
}

#[cfg(feature = "detector-onnx")]
struct OnnxBackend;

#[cfg(feature = "detector-onnx")]
impl DetectorBackend for OnnxBackend {
    fn kind(&self) -> SubtitleDetectorKind {
        SubtitleDetectorKind::OnnxPpocr
    }

    fn ensure_available(
        &self,
        config: &SubtitleDetectionConfig,
    ) -> Result<(), SubtitleDetectionError> {
        OnnxPpocrDetector::ensure_available(config)
    }

    fn build(
        &self,
        config: SubtitleDetectionConfig,
    ) -> Result<Box<dyn SubtitleDetector>, SubtitleDetectionError> {
        Ok(Box::new(OnnxPpocrDetector::new(config)?))
    }
}

#[cfg(feature = "detector-onnx")]
static ONNX_BACKEND: OnnxBackend = OnnxBackend;

#[cfg(all(feature = "detector-vision", target_os = "macos"))]
struct VisionBackend;

#[cfg(all(feature = "detector-vision", target_os = "macos"))]
impl DetectorBackend for VisionBackend {
    fn kind(&self) -> SubtitleDetectorKind {
        SubtitleDetectorKind::MacVision
    }

    fn ensure_available(
        &self,
        config: &SubtitleDetectionConfig,
    ) -> Result<(), SubtitleDetectionError> {
        VisionTextDetector::ensure_available(config)
    }

    fn build(
        &self,
        config: SubtitleDetectionConfig,
    ) -> Result<Box<dyn SubtitleDetector>, SubtitleDetectionError> {
        Ok(Box::new(VisionTextDetector::new(config)?))
    }
}

#[cfg(all(feature = "detector-vision", target_os = "macos"))]
static VISION_BACKEND: VisionBackend = VisionBackend;

struct LumaBandBackend;

impl DetectorBackend for LumaBandBackend {
    fn kind(&self) -> SubtitleDetectorKind {
        SubtitleDetectorKind::LumaBand
    }

    fn ensure_available(
        &self,
        config: &SubtitleDetectionConfig,
    ) -> Result<(), SubtitleDetectionError> {
        LumaBandDetector::ensure_available(config)
    }

    fn build(
        &self,
        config: SubtitleDetectionConfig,
    ) -> Result<Box<dyn SubtitleDetector>, SubtitleDetectionError> {
        Ok(Box::new(LumaBandDetector::new(config)?))
    }
}

static LUMA_BAND_BACKEND: LumaBandBackend = LumaBandBackend;

#[derive(Debug, Error)]
pub enum SubtitleDetectionError {
    #[error("provided plane data length {data_len} is smaller than stride * height ({required})")]
    InsufficientData { data_len: usize, required: usize },
    #[error("region of interest height is zero")]
    EmptyRoi,
    #[error("failed to initialize onnx runtime environment: {0}")]
    Environment(String),
    #[error("failed to create inference session: {0}")]
    Session(String),
    #[error("model file not found: {path}")]
    ModelNotFound { path: std::path::PathBuf },
    #[error("no ONNX model path configured; provide --onnx-model or set detection.onnx_model_path in the configuration file")]
    MissingOnnxModelPath,
    #[error("failed to prepare model input: {0}")]
    Input(String),
    #[error("model inference failed: {0}")]
    Inference(String),
    #[error("unexpected model output shape")]
    InvalidOutputShape,
    #[error(
        "onnxruntime schema registration conflict detected. Ensure only one ONNX Runtime version is present and that it matches the crate (suggest reinstalling onnxruntime 1.16.x). Original error: {message}"
    )]
    RuntimeSchemaConflict { message: String },
    #[error("vision framework error: {0}")]
    Vision(String),
    #[error("{backend} detector is not supported on this platform")]
    Unsupported { backend: &'static str },
}

#[derive(Debug, Clone)]
pub struct SubtitleDetectionConfig {
    pub frame_width: usize,
    pub frame_height: usize,
    pub stride: usize,
    pub model_path: Option<PathBuf>,
    pub roi: RoiConfig,
    pub luma_band: LumaBandConfig,
}

impl SubtitleDetectionConfig {
    pub fn for_frame(frame_width: usize, frame_height: usize, stride: usize) -> Self {
        Self {
            frame_width,
            frame_height,
            stride,
            model_path: None,
            roi: RoiConfig {
                x: 0.05,
                y: 0.65,
                width: 0.90,
                height: 0.30,
            },
            luma_band: LumaBandConfig {
                target_luma: DEFAULT_LUMA_TARGET,
                delta: DEFAULT_LUMA_DELTA,
            },
        }
    }
}

#[cfg(feature = "detector-onnx")]
pub fn ensure_onnx_detector_ready(model_path: Option<&Path>) -> Result<(), SubtitleDetectionError> {
    onnx::ensure_model_ready(model_path)
}

pub fn preflight_detection(
    kind: SubtitleDetectorKind,
    model_path: Option<&Path>,
) -> Result<(), SubtitleDetectionError> {
    let probe_config = build_probe_config(model_path);
    match kind {
        SubtitleDetectorKind::Auto => preflight_auto(&probe_config, model_path),
        SubtitleDetectorKind::OnnxPpocr => {
            #[cfg(feature = "detector-onnx")]
            log_onnx_preflight(model_path);
            ensure_backend_available(SubtitleDetectorKind::OnnxPpocr, &probe_config)
        }
        SubtitleDetectorKind::MacVision => {
            ensure_backend_available(SubtitleDetectorKind::MacVision, &probe_config)
        }
        SubtitleDetectorKind::LumaBand => {
            ensure_backend_available(SubtitleDetectorKind::LumaBand, &probe_config)
        }
    }
}

#[cfg(feature = "detector-onnx")]
fn log_onnx_preflight(path: Option<&Path>) {
    let target = path
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "bundled default model".to_string());
    eprintln!("preflighting ONNX subtitle detector using {target}");
}

fn build_probe_config(model_path: Option<&Path>) -> SubtitleDetectionConfig {
    let mut config = SubtitleDetectionConfig::for_frame(640, 360, 640);
    if let Some(path) = model_path {
        config.model_path = Some(path.to_path_buf());
    }
    config
}

fn preflight_auto(
    probe_config: &SubtitleDetectionConfig,
    model_path: Option<&Path>,
) -> Result<(), SubtitleDetectionError> {
    let mut last_err: Option<SubtitleDetectionError> = None;
    for &candidate in auto_backend_priority() {
        if candidate == SubtitleDetectorKind::OnnxPpocr {
            #[cfg(feature = "detector-onnx")]
            log_onnx_preflight(model_path);
        }
        match ensure_backend_available(candidate, probe_config) {
            Ok(()) => return Ok(()),
            Err(err) => {
                eprintln!(
                    "auto subtitle detector candidate '{}' unavailable during preflight: {err}",
                    candidate.as_str()
                );
                last_err = Some(err);
            }
        }
    }
    Err(last_err.unwrap_or(SubtitleDetectionError::Unsupported {
        backend: SubtitleDetectorKind::Auto.as_str(),
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleDetectorKind {
    Auto,
    OnnxPpocr,
    MacVision,
    LumaBand,
}

impl SubtitleDetectorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SubtitleDetectorKind::Auto => "auto",
            SubtitleDetectorKind::OnnxPpocr => "onnx-ppocr",
            SubtitleDetectorKind::MacVision => "macos-vision",
            SubtitleDetectorKind::LumaBand => "luma",
        }
    }
}

pub trait SubtitleDetector: Send + Sync {
    fn detect(
        &self,
        y_plane: &[u8],
        metadata: &FrameMetadata,
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError>;

    fn ensure_available(config: &SubtitleDetectionConfig) -> Result<(), SubtitleDetectionError>
    where
        Self: Sized;
}

pub fn build_detector(
    kind: SubtitleDetectorKind,
    config: SubtitleDetectionConfig,
) -> Result<Box<dyn SubtitleDetector>, SubtitleDetectionError> {
    match kind {
        SubtitleDetectorKind::Auto => build_auto(config),
        _ => {
            let backend =
                backend_for_kind(kind).ok_or_else(|| SubtitleDetectionError::Unsupported {
                    backend: kind.as_str(),
                })?;
            ensure_backend_or_panic(backend, &config);
            backend.build(config)
        }
    }
}

fn auto_backend_priority() -> &'static [SubtitleDetectorKind] {
    AUTO_DETECTOR_PRIORITY
}

fn build_auto(
    config: SubtitleDetectionConfig,
) -> Result<Box<dyn SubtitleDetector>, SubtitleDetectionError> {
    let mut last_err: Option<SubtitleDetectionError> = None;
    for &candidate in auto_backend_priority() {
        let Some(backend) = backend_for_kind(candidate) else {
            let err = SubtitleDetectionError::Unsupported {
                backend: candidate.as_str(),
            };
            eprintln!(
                "auto subtitle detector candidate '{}' unavailable: {err}",
                candidate.as_str()
            );
            last_err = Some(err);
            continue;
        };
        let candidate_config = config.clone();
        match backend.ensure_available(&candidate_config) {
            Ok(()) => match backend.build(candidate_config) {
                Ok(detector) => return Ok(detector),
                Err(err) => {
                    eprintln!(
                        "auto subtitle detector candidate '{}' failed to initialize: {err}",
                        candidate.as_str()
                    );
                    last_err = Some(err);
                }
            },
            Err(err) => {
                eprintln!(
                    "auto subtitle detector candidate '{}' unavailable: {err}",
                    candidate.as_str()
                );
                last_err = Some(err);
            }
        }
    }
    Err(last_err.unwrap_or(SubtitleDetectionError::Unsupported {
        backend: SubtitleDetectorKind::Auto.as_str(),
    }))
}

fn ensure_backend_or_panic(backend: &dyn DetectorBackend, config: &SubtitleDetectionConfig) {
    if let Err(err) = backend.ensure_available(config) {
        panic!(
            "subtitle detection backend '{}' is not available: {err}",
            backend.kind().as_str()
        );
    }
}

fn ensure_backend_available(
    kind: SubtitleDetectorKind,
    config: &SubtitleDetectionConfig,
) -> Result<(), SubtitleDetectionError> {
    match kind {
        SubtitleDetectorKind::Auto => Ok(()),
        _ => backend_for_kind(kind)
            .ok_or_else(|| SubtitleDetectionError::Unsupported {
                backend: kind.as_str(),
            })?
            .ensure_available(config),
    }
}

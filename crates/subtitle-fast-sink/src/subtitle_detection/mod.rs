use std::path::{Path, PathBuf};

use crate::config::FrameMetadata;
use serde::Serialize;
use thiserror::Error;

#[cfg(feature = "detector-onnx")]
pub mod onnx;
#[cfg(feature = "detector-onnx")]
pub use onnx::OnnxPpocrDetector;

#[cfg(all(feature = "detector-vision", target_os = "macos"))]
pub mod vision;
#[cfg(all(feature = "detector-vision", target_os = "macos"))]
pub use vision::VisionTextDetector;

const OUTPUT_JSON_PATH: &str = "subtitle_detection_output.jsonl";

#[cfg(target_os = "macos")]
const AUTO_DETECTOR_PRIORITY: &[SubtitleDetectorKind] = &[
    SubtitleDetectorKind::MacVision,
    SubtitleDetectorKind::OnnxPpocr,
];

#[cfg(not(target_os = "macos"))]
const AUTO_DETECTOR_PRIORITY: &[SubtitleDetectorKind] = &[SubtitleDetectorKind::OnnxPpocr];

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
    pub dump_json: bool,
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
            dump_json: true,
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
    config.dump_json = false;
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
}

impl SubtitleDetectorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SubtitleDetectorKind::Auto => "auto",
            SubtitleDetectorKind::OnnxPpocr => "onnx-ppocr",
            SubtitleDetectorKind::MacVision => "macos-vision",
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
        SubtitleDetectorKind::OnnxPpocr | SubtitleDetectorKind::MacVision => {
            ensure_backend_or_panic(kind, &config);
            instantiate_backend(kind, config)
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
        let candidate_config = config.clone();
        match ensure_backend_available(candidate, &candidate_config) {
            Ok(()) => match instantiate_backend(candidate, candidate_config) {
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

fn build_onnx(
    config: SubtitleDetectionConfig,
) -> Result<Box<dyn SubtitleDetector>, SubtitleDetectionError> {
    #[cfg(feature = "detector-onnx")]
    {
        return Ok(Box::new(OnnxPpocrDetector::new(config)?));
    }
    #[cfg(not(feature = "detector-onnx"))]
    {
        let _ = config;
        Err(SubtitleDetectionError::Unsupported {
            backend: SubtitleDetectorKind::OnnxPpocr.as_str(),
        })
    }
}

fn build_vision(
    config: SubtitleDetectionConfig,
) -> Result<Box<dyn SubtitleDetector>, SubtitleDetectionError> {
    #[cfg(all(feature = "detector-vision", target_os = "macos"))]
    {
        return Ok(Box::new(VisionTextDetector::new(config)?));
    }
    #[cfg(not(all(feature = "detector-vision", target_os = "macos")))]
    {
        let _ = config;
        Err(SubtitleDetectionError::Unsupported {
            backend: SubtitleDetectorKind::MacVision.as_str(),
        })
    }
}

fn ensure_backend_or_panic(kind: SubtitleDetectorKind, config: &SubtitleDetectionConfig) {
    if let Err(err) = ensure_backend_available(kind, config) {
        panic!(
            "subtitle detection backend '{}' is not available: {err}",
            kind.as_str()
        );
    }
}

fn ensure_backend_available(
    kind: SubtitleDetectorKind,
    config: &SubtitleDetectionConfig,
) -> Result<(), SubtitleDetectionError> {
    match kind {
        SubtitleDetectorKind::Auto => Ok(()),
        SubtitleDetectorKind::OnnxPpocr => {
            #[cfg(feature = "detector-onnx")]
            {
                OnnxPpocrDetector::ensure_available(config)
            }
            #[cfg(not(feature = "detector-onnx"))]
            {
                let _ = config;
                Err(SubtitleDetectionError::Unsupported {
                    backend: SubtitleDetectorKind::OnnxPpocr.as_str(),
                })
            }
        }
        SubtitleDetectorKind::MacVision => {
            #[cfg(all(feature = "detector-vision", target_os = "macos"))]
            {
                VisionTextDetector::ensure_available(config)
            }
            #[cfg(not(all(feature = "detector-vision", target_os = "macos")))]
            {
                let _ = config;
                Err(SubtitleDetectionError::Unsupported {
                    backend: SubtitleDetectorKind::MacVision.as_str(),
                })
            }
        }
    }
}

fn instantiate_backend(
    kind: SubtitleDetectorKind,
    config: SubtitleDetectionConfig,
) -> Result<Box<dyn SubtitleDetector>, SubtitleDetectionError> {
    match kind {
        SubtitleDetectorKind::OnnxPpocr => build_onnx(config),
        SubtitleDetectorKind::MacVision => build_vision(config),
        SubtitleDetectorKind::Auto => unreachable!("auto backend cannot be instantiated directly"),
    }
}

pub fn append_json_result(
    metadata: &FrameMetadata,
    result: &SubtitleDetectionResult,
) -> std::io::Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(OUTPUT_JSON_PATH)?;
    let line = serde_json::to_string(&serde_json::json!({
        "frame_index": metadata.frame_index,
        "has_subtitle": result.has_subtitle,
        "max_score": result.max_score,
        "regions": result.regions,
    }))?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

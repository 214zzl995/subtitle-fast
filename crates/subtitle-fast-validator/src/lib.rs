pub mod subtitle_detection;

mod config;
mod detection;
mod validator;

pub use config::{FrameValidatorConfig, SubtitleDetectionOptions};
#[cfg(feature = "detector-onnx")]
pub use subtitle_detection::ensure_onnx_detector_ready;
pub use subtitle_detection::{RoiConfig, SubtitleDetectorKind};
pub use validator::FrameValidator;

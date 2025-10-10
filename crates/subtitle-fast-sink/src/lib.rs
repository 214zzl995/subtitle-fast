pub mod subtitle_detection;

mod config;
mod detection;
mod dump;
mod sampler;
mod sink;

pub use config::{
    FrameDumpConfig, FrameMetadata, FrameSinkConfig, ImageOutputFormat, SubtitleDetectionOptions,
};
pub use sink::{FrameSink, FrameSinkError, FrameSinkProgress};
#[cfg(feature = "detector-onnx")]
pub use subtitle_detection::ensure_onnx_detector_ready;
pub use subtitle_detection::{RoiConfig, SubtitleDetectorKind};

pub mod subtitle_detection;

mod config;
mod detection;
mod validator;

pub use config::{FrameValidatorConfig, SubtitleDetectionOptions};
pub use subtitle_detection::RoiConfig;
pub use validator::FrameValidator;

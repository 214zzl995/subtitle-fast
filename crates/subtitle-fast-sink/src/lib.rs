pub mod subtitle_detection;

mod config;
mod detection;
mod dump;
mod sink;

pub use config::{
    FrameDumpConfig, FrameMetadata, FrameSinkConfig, ImageOutputFormat, SubtitleDetectionOptions,
};
pub use sink::{FrameSink, FrameSinkError, FrameSinkProgress};

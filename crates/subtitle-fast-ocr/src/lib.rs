mod engine;
mod error;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(feature = "engine-onnx")]
mod onnx;
mod plane;
mod region;
mod request;
mod response;

pub use engine::{NoopOcrEngine, OcrEngine};
pub use error::OcrError;
#[cfg(target_os = "macos")]
pub use macos::VisionOcrEngine;
#[cfg(feature = "engine-onnx")]
pub use onnx::OnnxOcrEngine;
pub use plane::LumaPlane;
pub use region::OcrRegion;
pub use request::OcrRequest;
pub use response::{OcrResponse, OcrText};

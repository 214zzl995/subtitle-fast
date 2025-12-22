//! Shared domain models for the subtitle-fast workspace.
//!
//! This crate centralizes lightweight data structures used across decoder,
//! validator, comparator, OCR, and CLI crates. Keep it backend-agnostic and
//! avoid platform-specific dependencies so all crates can depend on it without
//! pulling native SDKs or heavy features.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use thiserror::Error;

pub type YPlaneResult<T> = Result<T, YPlaneError>;

#[derive(Clone)]
pub struct YPlaneFrame {
    width: u32,
    height: u32,
    stride: usize,
    frame_index: Option<u64>,
    timestamp: Option<Duration>,
    data: Arc<[u8]>,
}

impl fmt::Debug for YPlaneFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("YPlaneFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("stride", &self.stride)
            .field("timestamp", &self.timestamp)
            .field("bytes", &self.data.len())
            .field("frame_index", &self.frame_index)
            .finish()
    }
}

impl YPlaneFrame {
    pub fn from_owned(
        width: u32,
        height: u32,
        stride: usize,
        timestamp: Option<Duration>,
        data: Vec<u8>,
    ) -> YPlaneResult<Self> {
        let required =
            stride
                .checked_mul(height as usize)
                .ok_or_else(|| YPlaneError::InvalidFrame {
                    reason: "calculated Y plane length overflowed".into(),
                })?;
        if data.len() < required {
            return Err(YPlaneError::InvalidFrame {
                reason: format!(
                    "insufficient Y plane bytes: got {} expected at least {}",
                    data.len(),
                    required
                ),
            });
        }
        Ok(Self {
            width,
            height,
            stride,
            timestamp,
            data: Arc::from(data.into_boxed_slice()),
            frame_index: None,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn stride(&self) -> usize {
        self.stride
    }

    pub fn timestamp(&self) -> Option<Duration> {
        self.timestamp
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn frame_index(&self) -> Option<u64> {
        self.frame_index
    }

    pub fn with_frame_index(mut self, index: Option<u64>) -> Self {
        self.frame_index = index;
        self
    }

    pub fn set_frame_index(&mut self, index: Option<u64>) {
        self.frame_index = index;
    }
}

#[derive(Debug, Error)]
pub enum YPlaneError {
    #[error("backend {backend} is not supported in this build")]
    Unsupported { backend: &'static str },

    #[error("{backend} backend failed: {message}")]
    BackendFailure {
        backend: &'static str,
        message: String,
    },

    #[error("configuration error: {message}")]
    Configuration { message: String },

    #[error("invalid frame: {reason}")]
    InvalidFrame { reason: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl YPlaneError {
    pub fn unsupported(backend: &'static str) -> Self {
        Self::Unsupported { backend }
    }

    pub fn backend_failure(backend: &'static str, message: impl Into<String>) -> Self {
        Self::BackendFailure {
            backend,
            message: message.into(),
        }
    }

    pub fn configuration(message: impl Into<String>) -> Self {
        Self::Configuration {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
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

impl SubtitleDetectionResult {
    pub fn empty() -> Self {
        Self {
            has_subtitle: false,
            max_score: 0.0,
            regions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OcrRegion {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl OcrRegion {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OcrText {
    pub region: OcrRegion,
    pub text: String,
    pub confidence: Option<f32>,
}

impl OcrText {
    pub fn new(region: OcrRegion, text: String) -> Self {
        Self {
            region,
            text,
            confidence: None,
        }
    }

    pub fn with_confidence(mut self, value: f32) -> Self {
        self.confidence = Some(value);
        self
    }
}

#[derive(Debug, Clone)]
pub struct OcrResponse {
    pub texts: Vec<OcrText>,
}

impl OcrResponse {
    pub fn new(texts: Vec<OcrText>) -> Self {
        Self { texts }
    }

    pub fn empty() -> Self {
        Self { texts: Vec::new() }
    }
}

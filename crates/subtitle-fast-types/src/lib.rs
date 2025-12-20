//! Shared domain models for the subtitle-fast workspace.
//!
//! This crate centralizes lightweight data structures used across decoder,
//! validator, comparator, OCR, and CLI crates. Keep it backend-agnostic and
//! avoid platform-specific dependencies so all crates can depend on it without
//! pulling native SDKs or heavy features.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use thiserror::Error;

pub type YPlaneResult<T> = Result<T, YPlaneError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawFrameFormat {
    Y,
    NV12,
    NV21,
    I420,
    YUYV,
    UYVY,
}

impl RawFrameFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            RawFrameFormat::Y => "y",
            RawFrameFormat::NV12 => "nv12",
            RawFrameFormat::NV21 => "nv21",
            RawFrameFormat::I420 => "i420",
            RawFrameFormat::YUYV => "yuyv",
            RawFrameFormat::UYVY => "uyvy",
        }
    }
}

impl fmt::Display for RawFrameFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RawFrameFormat {
    type Err = YPlaneError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "y" | "y-plane" | "yplane" | "luma" => Ok(RawFrameFormat::Y),
            "nv12" => Ok(RawFrameFormat::NV12),
            "nv21" => Ok(RawFrameFormat::NV21),
            "i420" | "yuv420p" => Ok(RawFrameFormat::I420),
            "yuyv" | "yuyv422" => Ok(RawFrameFormat::YUYV),
            "uyvy" | "uyvy422" => Ok(RawFrameFormat::UYVY),
            other => Err(YPlaneError::configuration(format!(
                "unknown raw frame format '{other}'"
            ))),
        }
    }
}

#[derive(Clone)]
pub struct PlaneFrame {
    width: u32,
    height: u32,
    frame_index: Option<u64>,
    timestamp: Option<Duration>,
    raw: RawFrame,
}

#[derive(Clone)]
pub enum RawFrame {
    Y {
        stride: usize,
        data: Arc<[u8]>,
    },
    NV12 {
        y_stride: usize,
        uv_stride: usize,
        y: Arc<[u8]>,
        uv: Arc<[u8]>,
    },
    NV21 {
        y_stride: usize,
        vu_stride: usize,
        y: Arc<[u8]>,
        vu: Arc<[u8]>,
    },
    I420 {
        y_stride: usize,
        u_stride: usize,
        v_stride: usize,
        y: Arc<[u8]>,
        u: Arc<[u8]>,
        v: Arc<[u8]>,
    },
    YUYV {
        stride: usize,
        data: Arc<[u8]>,
    },
    UYVY {
        stride: usize,
        data: Arc<[u8]>,
    },
}

#[derive(Clone, Copy)]
pub struct PlaneView<'a> {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub data: &'a [u8],
}

impl fmt::Debug for PlaneFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlaneFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("timestamp", &self.timestamp)
            .field("frame_index", &self.frame_index)
            .field("format", &self.format())
            .finish()
    }
}

impl PlaneFrame {
    pub fn from_owned(
        width: u32,
        height: u32,
        stride: usize,
        timestamp: Option<Duration>,
        data: Vec<u8>,
    ) -> YPlaneResult<Self> {
        let raw = RawFrame::Y {
            stride,
            data: Arc::from(data.into_boxed_slice()),
        };
        Self::from_raw(width, height, timestamp, raw)
    }

    pub fn from_raw(
        width: u32,
        height: u32,
        timestamp: Option<Duration>,
        raw: RawFrame,
    ) -> YPlaneResult<Self> {
        raw.validate(width, height)?;
        Ok(Self {
            width,
            height,
            frame_index: None,
            timestamp,
            raw,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn timestamp(&self) -> Option<Duration> {
        self.timestamp
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

    pub fn raw(&self) -> &RawFrame {
        &self.raw
    }

    pub fn format(&self) -> RawFrameFormat {
        self.raw.format()
    }

    pub fn stride(&self) -> usize {
        self.y_plane().map(|plane| plane.stride).unwrap_or(0)
    }

    pub fn data(&self) -> &[u8] {
        self.y_plane().map(|plane| plane.data).unwrap_or(&[])
    }

    pub fn y_plane(&self) -> Option<PlaneView<'_>> {
        match &self.raw {
            RawFrame::Y { stride, data } => Some(PlaneView {
                width: self.width,
                height: self.height,
                stride: *stride,
                data,
            }),
            RawFrame::NV12 { y_stride, y, .. }
            | RawFrame::NV21 { y_stride, y, .. }
            | RawFrame::I420 { y_stride, y, .. } => Some(PlaneView {
                width: self.width,
                height: self.height,
                stride: *y_stride,
                data: y,
            }),
            RawFrame::YUYV { .. } | RawFrame::UYVY { .. } => None,
        }
    }
}

impl RawFrame {
    pub fn format(&self) -> RawFrameFormat {
        match self {
            RawFrame::Y { .. } => RawFrameFormat::Y,
            RawFrame::NV12 { .. } => RawFrameFormat::NV12,
            RawFrame::NV21 { .. } => RawFrameFormat::NV21,
            RawFrame::I420 { .. } => RawFrameFormat::I420,
            RawFrame::YUYV { .. } => RawFrameFormat::YUYV,
            RawFrame::UYVY { .. } => RawFrameFormat::UYVY,
        }
    }

    fn validate(&self, width: u32, height: u32) -> YPlaneResult<()> {
        let width = width as usize;
        let height = height as usize;
        let (chroma_width, chroma_height) = chroma_dims(width, height);
        match self {
            RawFrame::Y { stride, data } => {
                if *stride < width {
                    return Err(YPlaneError::InvalidFrame {
                        reason: format!("Y stride {stride} is smaller than width {width}"),
                    });
                }
                ensure_len(*stride, height, data.len(), "Y plane")?;
            }
            RawFrame::NV12 {
                y_stride,
                uv_stride,
                y,
                uv,
            } => {
                if *y_stride < width {
                    return Err(YPlaneError::InvalidFrame {
                        reason: format!("Y stride {y_stride} is smaller than width {width}"),
                    });
                }
                if *uv_stride < width {
                    return Err(YPlaneError::InvalidFrame {
                        reason: format!("UV stride {uv_stride} is smaller than width {width}"),
                    });
                }
                ensure_len(*y_stride, height, y.len(), "NV12 Y plane")?;
                ensure_len(*uv_stride, chroma_height, uv.len(), "NV12 UV plane")?;
            }
            RawFrame::NV21 {
                y_stride,
                vu_stride,
                y,
                vu,
            } => {
                if *y_stride < width {
                    return Err(YPlaneError::InvalidFrame {
                        reason: format!("Y stride {y_stride} is smaller than width {width}"),
                    });
                }
                if *vu_stride < width {
                    return Err(YPlaneError::InvalidFrame {
                        reason: format!("VU stride {vu_stride} is smaller than width {width}"),
                    });
                }
                ensure_len(*y_stride, height, y.len(), "NV21 Y plane")?;
                ensure_len(*vu_stride, chroma_height, vu.len(), "NV21 VU plane")?;
            }
            RawFrame::I420 {
                y_stride,
                u_stride,
                v_stride,
                y,
                u,
                v,
            } => {
                if *y_stride < width {
                    return Err(YPlaneError::InvalidFrame {
                        reason: format!("Y stride {y_stride} is smaller than width {width}"),
                    });
                }
                if *u_stride < chroma_width {
                    return Err(YPlaneError::InvalidFrame {
                        reason: format!(
                            "U stride {u_stride} is smaller than chroma width {chroma_width}"
                        ),
                    });
                }
                if *v_stride < chroma_width {
                    return Err(YPlaneError::InvalidFrame {
                        reason: format!(
                            "V stride {v_stride} is smaller than chroma width {chroma_width}"
                        ),
                    });
                }
                ensure_len(*y_stride, height, y.len(), "I420 Y plane")?;
                ensure_len(*u_stride, chroma_height, u.len(), "I420 U plane")?;
                ensure_len(*v_stride, chroma_height, v.len(), "I420 V plane")?;
            }
            RawFrame::YUYV { stride, data } => {
                let min_stride = width.saturating_mul(2);
                if *stride < min_stride {
                    return Err(YPlaneError::InvalidFrame {
                        reason: format!("YUYV stride {stride} is smaller than {min_stride}"),
                    });
                }
                ensure_len(*stride, height, data.len(), "YUYV frame")?;
            }
            RawFrame::UYVY { stride, data } => {
                let min_stride = width.saturating_mul(2);
                if *stride < min_stride {
                    return Err(YPlaneError::InvalidFrame {
                        reason: format!("UYVY stride {stride} is smaller than {min_stride}"),
                    });
                }
                ensure_len(*stride, height, data.len(), "UYVY frame")?;
            }
        }
        Ok(())
    }
}

fn chroma_dims(width: usize, height: usize) -> (usize, usize) {
    ((width + 1) / 2, (height + 1) / 2)
}

fn ensure_len(stride: usize, height: usize, actual: usize, label: &str) -> YPlaneResult<()> {
    let required = stride
        .checked_mul(height)
        .ok_or_else(|| YPlaneError::InvalidFrame {
            reason: format!("{label} length overflow"),
        })?;
    if actual < required {
        return Err(YPlaneError::InvalidFrame {
            reason: format!(
                "insufficient {label} bytes: got {actual} expected at least {required}"
            ),
        });
    }
    Ok(())
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

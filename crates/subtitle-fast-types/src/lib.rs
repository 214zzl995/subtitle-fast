//! Shared domain models for the subtitle-fast workspace.
//!
//! This crate centralizes lightweight data structures used across decoder,
//! validator, comparator, OCR, and CLI crates. Keep it backend-agnostic and
//! avoid platform-specific dependencies so all crates can depend on it without
//! pulling native SDKs or heavy features.

use std::ffi::c_void;
use std::fmt;
use std::ptr::NonNull;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use thiserror::Error;

pub type DecoderResult<T> = Result<T, DecoderError>;

#[derive(Clone)]
pub struct VideoFrame {
    width: u32,
    height: u32,
    frame_index: Option<u64>,
    timestamp: Option<Duration>,
    buffer: FrameBuffer,
}

#[derive(Clone)]
pub enum FrameBuffer {
    Nv12(Nv12Buffer),
    Native(NativeBuffer),
}

#[derive(Clone)]
pub struct Nv12Buffer {
    y_stride: usize,
    uv_stride: usize,
    y_plane: Arc<[u8]>,
    uv_plane: Arc<[u8]>,
}

#[derive(Clone)]
pub struct NativeBuffer {
    backend: &'static str,
    pixel_format: u32,
    handle: Arc<NativeHandle>,
}

struct NativeHandle {
    handle: NonNull<c_void>,
    release: unsafe extern "C" fn(*mut c_void),
}

// Native handles are ref-counted by the backend, and release callbacks are thread-safe.
unsafe impl Send for NativeHandle {}
unsafe impl Sync for NativeHandle {}

impl Drop for NativeHandle {
    fn drop(&mut self) {
        unsafe { (self.release)(self.handle.as_ptr()) };
    }
}

impl NativeBuffer {
    pub fn backend(&self) -> &'static str {
        self.backend
    }

    pub fn pixel_format(&self) -> u32 {
        self.pixel_format
    }

    pub fn handle(&self) -> *mut c_void {
        self.handle.handle.as_ptr()
    }
}

impl Nv12Buffer {
    pub fn y_stride(&self) -> usize {
        self.y_stride
    }

    pub fn uv_stride(&self) -> usize {
        self.uv_stride
    }

    pub fn y_plane(&self) -> &[u8] {
        &self.y_plane
    }

    pub fn uv_plane(&self) -> &[u8] {
        &self.uv_plane
    }
}

impl fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.buffer {
            FrameBuffer::Nv12(buffer) => f
                .debug_struct("VideoFrame")
                .field("width", &self.width)
                .field("height", &self.height)
                .field("format", &"nv12")
                .field("y_stride", &buffer.y_stride)
                .field("uv_stride", &buffer.uv_stride)
                .field("y_bytes", &buffer.y_plane.len())
                .field("uv_bytes", &buffer.uv_plane.len())
                .field("timestamp", &self.timestamp)
                .field("frame_index", &self.frame_index)
                .finish(),
            FrameBuffer::Native(buffer) => f
                .debug_struct("VideoFrame")
                .field("width", &self.width)
                .field("height", &self.height)
                .field("format", &"native-handle")
                .field("backend", &buffer.backend)
                .field("pixel_format", &buffer.pixel_format)
                .field("handle", &buffer.handle())
                .field("timestamp", &self.timestamp)
                .field("frame_index", &self.frame_index)
                .finish(),
        }
    }
}

impl VideoFrame {
    pub fn from_nv12_owned(
        width: u32,
        height: u32,
        y_stride: usize,
        uv_stride: usize,
        timestamp: Option<Duration>,
        mut y_plane: Vec<u8>,
        mut uv_plane: Vec<u8>,
    ) -> DecoderResult<Self> {
        let y_required =
            y_stride
                .checked_mul(height as usize)
                .ok_or_else(|| DecoderError::InvalidFrame {
                    reason: "calculated NV12 Y plane length overflowed".into(),
                })?;
        let uv_rows = nv12_uv_rows(height);
        let uv_required =
            uv_stride
                .checked_mul(uv_rows)
                .ok_or_else(|| DecoderError::InvalidFrame {
                    reason: "calculated NV12 UV plane length overflowed".into(),
                })?;

        if y_plane.len() < y_required {
            return Err(DecoderError::InvalidFrame {
                reason: format!(
                    "insufficient NV12 Y plane bytes: got {} expected at least {}",
                    y_plane.len(),
                    y_required
                ),
            });
        }
        if uv_plane.len() < uv_required {
            return Err(DecoderError::InvalidFrame {
                reason: format!(
                    "insufficient NV12 UV plane bytes: got {} expected at least {}",
                    uv_plane.len(),
                    uv_required
                ),
            });
        }

        y_plane.truncate(y_required);
        uv_plane.truncate(uv_required);

        Ok(Self {
            width,
            height,
            timestamp,
            frame_index: None,
            buffer: FrameBuffer::Nv12(Nv12Buffer {
                y_stride,
                uv_stride,
                y_plane: Arc::from(y_plane.into_boxed_slice()),
                uv_plane: Arc::from(uv_plane.into_boxed_slice()),
            }),
        })
    }

    pub fn from_native_handle(
        width: u32,
        height: u32,
        timestamp: Option<Duration>,
        frame_index: Option<u64>,
        backend: &'static str,
        pixel_format: u32,
        handle: *mut c_void,
        release: unsafe extern "C" fn(*mut c_void),
    ) -> DecoderResult<Self> {
        let handle = NonNull::new(handle).ok_or_else(|| DecoderError::InvalidFrame {
            reason: "native handle is null".into(),
        })?;

        Ok(Self {
            width,
            height,
            timestamp,
            frame_index,
            buffer: FrameBuffer::Native(NativeBuffer {
                backend,
                pixel_format,
                handle: Arc::new(NativeHandle { handle, release }),
            }),
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

    pub fn buffer(&self) -> &FrameBuffer {
        &self.buffer
    }

    pub fn nv12(&self) -> &Nv12Buffer {
        self.expect_nv12()
    }

    pub fn native(&self) -> Option<&NativeBuffer> {
        match &self.buffer {
            FrameBuffer::Native(buffer) => Some(buffer),
            _ => None,
        }
    }

    pub fn stride(&self) -> usize {
        self.expect_nv12().y_stride
    }

    pub fn y_stride(&self) -> usize {
        self.expect_nv12().y_stride
    }

    pub fn uv_stride(&self) -> usize {
        self.expect_nv12().uv_stride
    }

    pub fn data(&self) -> &[u8] {
        &self.expect_nv12().y_plane
    }

    pub fn y_plane(&self) -> &[u8] {
        &self.expect_nv12().y_plane
    }

    pub fn uv_plane(&self) -> &[u8] {
        &self.expect_nv12().uv_plane
    }

    pub fn with_frame_index(mut self, index: Option<u64>) -> Self {
        self.frame_index = index;
        self
    }

    pub fn set_frame_index(&mut self, index: Option<u64>) {
        self.frame_index = index;
    }

    fn expect_nv12(&self) -> &Nv12Buffer {
        match &self.buffer {
            FrameBuffer::Nv12(buffer) => buffer,
            FrameBuffer::Native(_) => {
                panic!("VideoFrame does not contain NV12 data (native handle output requested)")
            }
        }
    }
}

fn nv12_uv_rows(height: u32) -> usize {
    (height as usize + 1) / 2
}

#[derive(Debug, Error)]
pub enum DecoderError {
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

impl DecoderError {
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

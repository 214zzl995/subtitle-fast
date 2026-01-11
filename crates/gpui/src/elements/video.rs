use crate::{
    App, Bounds, DefiniteLength, DevicePixels, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Length, ObjectFit, Pixels, Style, StyleRefinement,
    Styled, Window, px, size,
};
#[cfg(target_os = "macos")]
use core_video::pixel_buffer::CVPixelBuffer;
use parking_lot::Mutex;
use refineable::Refineable;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

use crate::scene::{Nv12Frame, SurfaceId, SurfaceSource};

static NEXT_SURFACE_ID: AtomicU64 = AtomicU64::new(1);

/// Result type returned by NV12 frame constructors.
pub type DecoderResult<T> = std::result::Result<T, DecoderError>;

/// Errors returned when validating frame inputs.
#[derive(Debug, Error, Clone)]
pub enum DecoderError {
    /// Frame input failed validation.
    #[error("invalid frame: {reason}")]
    InvalidFrame {
        /// Details describing the validation failure.
        reason: String,
    },
}

/// NV12 frame data (Y plane + interleaved UV plane).
#[derive(Clone, Debug)]
pub struct Frame {
    width: u32,
    height: u32,
    y_stride: usize,
    uv_stride: usize,
    y_plane: Arc<[u8]>,
    uv_plane: Arc<[u8]>,
}

impl Frame {
    /// Build an NV12 frame from shared plane buffers.
    pub fn from_nv12(
        width: u32,
        height: u32,
        y_stride: usize,
        uv_stride: usize,
        y_plane: Arc<[u8]>,
        uv_plane: Arc<[u8]>,
    ) -> DecoderResult<Self> {
        if y_stride == 0 || uv_stride == 0 {
            return Err(DecoderError::InvalidFrame {
                reason: "NV12 plane stride is zero".into(),
            });
        }
        if y_stride < width as usize {
            return Err(DecoderError::InvalidFrame {
                reason: "NV12 Y plane stride is smaller than width".into(),
            });
        }
        if uv_stride < width as usize {
            return Err(DecoderError::InvalidFrame {
                reason: "NV12 UV plane stride is smaller than width".into(),
            });
        }
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

        Ok(Self {
            width,
            height,
            y_stride,
            uv_stride,
            y_plane,
            uv_plane,
        })
    }

    /// Build an NV12 frame from owned plane buffers.
    pub fn from_nv12_owned(
        width: u32,
        height: u32,
        y_stride: usize,
        uv_stride: usize,
        mut y_plane: Vec<u8>,
        mut uv_plane: Vec<u8>,
    ) -> DecoderResult<Self> {
        if y_stride == 0 || uv_stride == 0 {
            return Err(DecoderError::InvalidFrame {
                reason: "NV12 plane stride is zero".into(),
            });
        }
        if y_stride < width as usize {
            return Err(DecoderError::InvalidFrame {
                reason: "NV12 Y plane stride is smaller than width".into(),
            });
        }
        if uv_stride < width as usize {
            return Err(DecoderError::InvalidFrame {
                reason: "NV12 UV plane stride is smaller than width".into(),
            });
        }
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

        Self::from_nv12(
            width,
            height,
            y_stride,
            uv_stride,
            Arc::from(y_plane.into_boxed_slice()),
            Arc::from(uv_plane.into_boxed_slice()),
        )
    }

    /// Frame width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Frame height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Byte stride for the Y plane.
    pub fn y_stride(&self) -> usize {
        self.y_stride
    }

    /// Byte stride for the interleaved UV plane.
    pub fn uv_stride(&self) -> usize {
        self.uv_stride
    }

    /// Y plane bytes.
    pub fn y_plane(&self) -> &[u8] {
        &self.y_plane
    }

    /// Interleaved UV plane bytes.
    pub fn uv_plane(&self) -> &[u8] {
        &self.uv_plane
    }
}

/// Supported frame sources for the video element.
#[derive(Clone, Debug)]
pub enum VideoFrame {
    /// NV12 planes (Y + interleaved UV).
    Nv12(Frame),
    #[cfg(target_os = "macos")]
    /// macOS CoreVideo pixel buffer.
    CvPixelBuffer(CVPixelBuffer),
}

impl VideoFrame {
    /// Frame width in pixels.
    pub fn width(&self) -> u32 {
        match self {
            VideoFrame::Nv12(frame) => frame.width(),
            #[cfg(target_os = "macos")]
            VideoFrame::CvPixelBuffer(buffer) => buffer.get_width() as u32,
        }
    }

    /// Frame height in pixels.
    pub fn height(&self) -> u32 {
        match self {
            VideoFrame::Nv12(frame) => frame.height(),
            #[cfg(target_os = "macos")]
            VideoFrame::CvPixelBuffer(buffer) => buffer.get_height() as u32,
        }
    }
}

impl From<Frame> for VideoFrame {
    fn from(value: Frame) -> Self {
        VideoFrame::Nv12(value)
    }
}

#[cfg(target_os = "macos")]
impl From<CVPixelBuffer> for VideoFrame {
    fn from(value: CVPixelBuffer) -> Self {
        VideoFrame::CvPixelBuffer(value)
    }
}

/// Handle used to submit frames to a [`Video`] element.
#[derive(Clone)]
pub struct VideoHandle {
    state: Arc<VideoHandleState>,
}

struct VideoHandleState {
    id: SurfaceId,
    inner: Mutex<VideoHandleInner>,
}

struct VideoHandleInner {
    frame: Option<VideoFrame>,
    generation: u64,
}

impl Default for VideoHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoHandle {
    /// Create a new handle with a unique surface id.
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn new() -> Self {
        let id = SurfaceId(NEXT_SURFACE_ID.fetch_add(1, Ordering::Relaxed));
        Self {
            state: Arc::new(VideoHandleState {
                id,
                inner: Mutex::new(VideoHandleInner {
                    frame: None,
                    generation: 0,
                }),
            }),
        }
    }

    /// Submit a new frame, replacing any existing frame.
    pub fn submit(&self, frame: impl Into<VideoFrame>) {
        let mut inner = self.state.inner.lock();
        inner.frame = Some(frame.into());
        inner.generation = inner.generation.saturating_add(1);
    }

    /// Clear any queued frame.
    pub fn clear(&self) {
        let mut inner = self.state.inner.lock();
        inner.frame = None;
        inner.generation = inner.generation.saturating_add(1);
    }

    fn latest(&self) -> Option<(VideoFrame, u64)> {
        let inner = self.state.inner.lock();
        inner.frame.clone().map(|frame| (frame, inner.generation))
    }

    fn surface_id(&self) -> SurfaceId {
        self.state.id
    }
}

impl From<&VideoHandle> for VideoHandle {
    fn from(value: &VideoHandle) -> Self {
        value.clone()
    }
}

/// An element that renders the latest submitted video frame.
pub struct Video {
    handle: VideoHandle,
    object_fit: ObjectFit,
    style: StyleRefinement,
}

/// Create a new video element bound to a handle.
pub fn video(handle: impl Into<VideoHandle>) -> Video {
    Video {
        handle: handle.into(),
        object_fit: ObjectFit::Contain,
        style: Default::default(),
    }
}

impl Video {
    /// Set how the frame is fit inside the element bounds.
    pub fn object_fit(mut self, object_fit: ObjectFit) -> Self {
        self.object_fit = object_fit;
        self
    }
}

impl Element for Video {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.refine(&self.style);

        if let Some((frame, _)) = self.handle.latest() {
            let frame_size = size(px(frame.width() as f32), px(frame.height() as f32));
            style.aspect_ratio = Some(frame_size.width / frame_size.height);

            if let Length::Auto = style.size.width {
                style.size.width = match style.size.height {
                    Length::Definite(DefiniteLength::Absolute(abs_length)) => {
                        let height_px = abs_length.to_pixels(window.rem_size());
                        Length::Definite(
                            px(frame_size.width.0 * height_px.0 / frame_size.height.0).into(),
                        )
                    }
                    _ => Length::Definite(frame_size.width.into()),
                };
            }

            if let Length::Auto = style.size.height {
                style.size.height = match style.size.width {
                    Length::Definite(DefiniteLength::Absolute(abs_length)) => {
                        let width_px = abs_length.to_pixels(window.rem_size());
                        Length::Definite(
                            px(frame_size.height.0 * width_px.0 / frame_size.width.0).into(),
                        )
                    }
                    _ => Length::Definite(frame_size.height.into()),
                };
            }
        }

        let layout_id = window.request_layout(style, [], cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        let Some((frame, generation)) = self.handle.latest() else {
            return;
        };

        let frame_device_size = size(
            DevicePixels::from(frame.width() as i32),
            DevicePixels::from(frame.height() as i32),
        );
        let new_bounds = self.object_fit.get_bounds(bounds, frame_device_size);
        let surface_id = self.handle.surface_id();

        match frame {
            VideoFrame::Nv12(frame) => {
                window.paint_surface(
                    new_bounds,
                    SurfaceSource::Nv12(Nv12Frame::from(&frame)),
                    Some(surface_id),
                    generation,
                );
            }
            #[cfg(target_os = "macos")]
            VideoFrame::CvPixelBuffer(buffer) => {
                window.paint_surface(
                    new_bounds,
                    SurfaceSource::CvPixelBuffer(buffer),
                    Some(surface_id),
                    generation,
                );
            }
        }
    }
}

impl IntoElement for Video {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Styled for Video {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

fn nv12_uv_rows(height: u32) -> usize {
    (height as usize).div_ceil(2)
}

impl From<&Frame> for Nv12Frame {
    fn from(value: &Frame) -> Self {
        Nv12Frame {
            width: value.width,
            height: value.height,
            y_stride: value.y_stride,
            uv_stride: value.uv_stride,
            y_plane: Arc::clone(&value.y_plane),
            uv_plane: Arc::clone(&value.uv_plane),
        }
    }
}

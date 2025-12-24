use crate::{DevicePixels, Pixels, Result, SharedString, Size, size};
use anyhow::anyhow;
use smallvec::SmallVec;

use image::{Delay, Frame};
use std::{
    borrow::Cow,
    fmt,
    hash::Hash,
    sync::Arc,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

/// A source of assets for this app to use.
pub trait AssetSource: 'static + Send + Sync {
    /// Load the given asset from the source path.
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>>;

    /// List the assets at the given path.
    fn list(&self, path: &str) -> Result<Vec<SharedString>>;
}

impl AssetSource for () {
    fn load(&self, _path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(None)
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(vec![])
    }
}

/// A unique identifier for the image cache
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ImageId(pub usize);

static NEXT_IMAGE_ID: AtomicUsize = AtomicUsize::new(0);

#[derive(PartialEq, Eq, Hash, Clone)]
pub(crate) struct RenderImageParams {
    pub(crate) image_id: ImageId,
    pub(crate) frame_index: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct Nv12Frame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) y_stride: usize,
    pub(crate) uv_stride: usize,
    pub(crate) y_plane: Arc<[u8]>,
    pub(crate) uv_plane: Arc<[u8]>,
}

impl Nv12Frame {
    pub(crate) fn uv_width(&self) -> u32 {
        (self.width + 1) / 2
    }

    pub(crate) fn uv_height(&self) -> u32 {
        (self.height + 1) / 2
    }
}

enum RenderImageData {
    Rgba(SmallVec<[Frame; 1]>),
    Nv12(Nv12Frame),
}

/// A cached and processed image, in BGRA or NV12 format
pub struct RenderImage {
    /// The ID associated with this image
    pub id: ImageId,
    /// The scale factor of this image on render.
    pub(crate) scale_factor: f32,
    data: RenderImageData,
}

impl PartialEq for RenderImage {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for RenderImage {}

impl RenderImage {
    /// Create a new image from the given data.
    pub fn new(data: impl Into<SmallVec<[Frame; 1]>>) -> Self {
        Self {
            id: ImageId(NEXT_IMAGE_ID.fetch_add(1, SeqCst)),
            scale_factor: 1.0,
            data: RenderImageData::Rgba(data.into()),
        }
    }

    /// Create a new NV12 image from the given planes and strides.
    pub fn from_nv12(
        width: u32,
        height: u32,
        y_stride: usize,
        uv_stride: usize,
        y_plane: Vec<u8>,
        uv_plane: Vec<u8>,
    ) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(anyhow!("NV12 images must have non-zero dimensions"));
        }

        if y_stride < width as usize {
            return Err(anyhow!(
                "NV12 Y stride {} is smaller than width {}",
                y_stride,
                width
            ));
        }

        let uv_width = (width + 1) / 2;
        let uv_height = (height + 1) / 2;
        let min_uv_stride = uv_width as usize * 2;
        if uv_stride < min_uv_stride {
            return Err(anyhow!(
                "NV12 UV stride {} is smaller than minimum {}",
                uv_stride,
                min_uv_stride
            ));
        }

        let min_y_len = y_stride
            .checked_mul(height as usize)
            .ok_or_else(|| anyhow!("NV12 Y plane size overflow"))?;
        if y_plane.len() < min_y_len {
            return Err(anyhow!(
                "NV12 Y plane length {} is smaller than expected {}",
                y_plane.len(),
                min_y_len
            ));
        }

        let min_uv_len = uv_stride
            .checked_mul(uv_height as usize)
            .ok_or_else(|| anyhow!("NV12 UV plane size overflow"))?;
        if uv_plane.len() < min_uv_len {
            return Err(anyhow!(
                "NV12 UV plane length {} is smaller than expected {}",
                uv_plane.len(),
                min_uv_len
            ));
        }

        Ok(Self {
            id: ImageId(NEXT_IMAGE_ID.fetch_add(1, SeqCst)),
            scale_factor: 1.0,
            data: RenderImageData::Nv12(Nv12Frame {
                width,
                height,
                y_stride,
                uv_stride,
                y_plane: Arc::from(y_plane),
                uv_plane: Arc::from(uv_plane),
            }),
        })
    }

    pub(crate) fn nv12_frame(&self) -> Option<&Nv12Frame> {
        match &self.data {
            RenderImageData::Nv12(frame) => Some(frame),
            RenderImageData::Rgba(_) => None,
        }
    }

    /// Convert this image into a byte slice.
    pub fn as_bytes(&self, frame_index: usize) -> Option<&[u8]> {
        match &self.data {
            RenderImageData::Rgba(frames) => frames
                .get(frame_index)
                .map(|frame| frame.buffer().as_raw().as_slice()),
            RenderImageData::Nv12(_) => None,
        }
    }

    /// Get the size of this image, in pixels.
    pub fn size(&self, frame_index: usize) -> Size<DevicePixels> {
        match &self.data {
            RenderImageData::Rgba(frames) => {
                let (width, height) = frames[frame_index].buffer().dimensions();
                size(width.into(), height.into())
            }
            RenderImageData::Nv12(frame) => {
                debug_assert_eq!(frame_index, 0, "NV12 images only have one frame");
                size(frame.width.into(), frame.height.into())
            }
        }
    }

    /// Get the size of this image, in pixels for display, adjusted for the scale factor.
    pub(crate) fn render_size(&self, frame_index: usize) -> Size<Pixels> {
        self.size(frame_index)
            .map(|v| (v.0 as f32 / self.scale_factor).into())
    }

    /// Get the delay of this frame from the previous
    pub fn delay(&self, frame_index: usize) -> Delay {
        match &self.data {
            RenderImageData::Rgba(frames) => frames[frame_index].delay(),
            RenderImageData::Nv12(_) => Delay::from_numer_denom_ms(0, 1),
        }
    }

    /// Get the number of frames for this image.
    pub fn frame_count(&self) -> usize {
        match &self.data {
            RenderImageData::Rgba(frames) => frames.len(),
            RenderImageData::Nv12(_) => 1,
        }
    }
}

impl fmt::Debug for RenderImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let format = match &self.data {
            RenderImageData::Rgba(_) => "rgba",
            RenderImageData::Nv12(_) => "nv12",
        };

        f.debug_struct("ImageData")
            .field("id", &self.id)
            .field("format", &format)
            .field("size", &self.size(0))
            .finish()
    }
}

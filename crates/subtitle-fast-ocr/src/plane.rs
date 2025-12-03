use std::fmt;
use std::ops::Deref;

use subtitle_fast_types::YPlaneFrame;

use crate::error::OcrError;

/// Immutable view over a Y (luminance) plane.
#[derive(Clone)]
pub struct LumaPlane<'a> {
    width: u32,
    height: u32,
    stride: usize,
    data: &'a [u8],
}

impl<'a> LumaPlane<'a> {
    pub fn from_parts(
        width: u32,
        height: u32,
        stride: usize,
        data: &'a [u8],
    ) -> Result<Self, OcrError> {
        let required = stride
            .checked_mul(height as usize)
            .ok_or(OcrError::PlaneOverflow { stride, height })?;
        if data.len() < required {
            return Err(OcrError::InsufficientPlaneData {
                provided: data.len(),
                required,
            });
        }
        Ok(Self {
            width,
            height,
            stride,
            data: &data[..required],
        })
    }

    pub fn from_frame(frame: &'a YPlaneFrame) -> Self {
        // SAFETY: YPlaneFrame guarantees the buffer is at least stride * height bytes long.
        Self {
            width: frame.width(),
            height: frame.height(),
            stride: frame.stride(),
            data: frame.data(),
        }
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

    pub fn data(&self) -> &'a [u8] {
        self.data
    }
}

impl fmt::Debug for LumaPlane<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LumaPlane")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("stride", &self.stride)
            .field("bytes", &self.data.len())
            .finish()
    }
}

impl Deref for LumaPlane<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.data
    }
}

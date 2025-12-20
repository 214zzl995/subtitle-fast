use gpui::{Image, ImageFormat};
use png::{BitDepth, ColorType, Encoder};
use std::borrow::Cow;
use std::sync::Arc;
use subtitle_fast_types::{PlaneFrame, RawFrame};
use yuv::{
    YuvBiPlanarImage, YuvConversionMode, YuvGrayImage, YuvPackedImage, YuvPlanarImage, YuvRange,
    YuvStandardMatrix, uyvy422_to_rgb, yuv_nv12_to_rgb, yuv_nv21_to_rgb, yuv400_to_rgb,
    yuv420_to_rgb, yuyv422_to_rgb,
};

#[derive(Debug)]
pub struct FrameImageError(String);

impl FrameImageError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for FrameImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FrameImageError {}

pub fn frame_to_image(frame: &PlaneFrame) -> Result<Arc<Image>, FrameImageError> {
    let width = frame.width();
    let height = frame.height();

    let width_usize =
        usize::try_from(width).map_err(|_| FrameImageError::new("frame width overflows usize"))?;
    let height_usize = usize::try_from(height)
        .map_err(|_| FrameImageError::new("frame height overflows usize"))?;

    let rgb_stride = width_usize
        .checked_mul(3)
        .ok_or_else(|| FrameImageError::new("rgb stride overflows usize"))?;
    let rgb_len = rgb_stride
        .checked_mul(height_usize)
        .ok_or_else(|| FrameImageError::new("rgb buffer size overflows usize"))?;
    let rgb_stride_u32 =
        u32::try_from(rgb_stride).map_err(|_| FrameImageError::new("rgb stride overflows u32"))?;

    let mut rgb = vec![0u8; rgb_len];

    let range = YuvRange::Full;
    let matrix = YuvStandardMatrix::Bt601;
    let mode = YuvConversionMode::Balanced;

    match frame.raw() {
        RawFrame::Y { stride, data } => {
            let y_stride = u32::try_from(*stride)
                .map_err(|_| FrameImageError::new("Y stride overflows u32"))?;
            let gray_image = YuvGrayImage {
                y_plane: data,
                y_stride,
                width,
                height,
            };
            yuv400_to_rgb(&gray_image, &mut rgb, rgb_stride_u32, range, matrix)
                .map_err(map_yuv_error)?;
        }
        RawFrame::NV12 {
            y_stride,
            uv_stride,
            y,
            uv,
        } => {
            let y_stride = u32::try_from(*y_stride)
                .map_err(|_| FrameImageError::new("NV12 Y stride overflows u32"))?;
            let uv_stride = u32::try_from(*uv_stride)
                .map_err(|_| FrameImageError::new("NV12 UV stride overflows u32"))?;
            let bi_planar = YuvBiPlanarImage {
                y_plane: y,
                y_stride,
                uv_plane: uv,
                uv_stride,
                width,
                height,
            };
            yuv_nv12_to_rgb(&bi_planar, &mut rgb, rgb_stride_u32, range, matrix, mode)
                .map_err(map_yuv_error)?;
        }
        RawFrame::NV21 {
            y_stride,
            vu_stride,
            y,
            vu,
        } => {
            let y_stride = u32::try_from(*y_stride)
                .map_err(|_| FrameImageError::new("NV21 Y stride overflows u32"))?;
            let vu_stride = u32::try_from(*vu_stride)
                .map_err(|_| FrameImageError::new("NV21 VU stride overflows u32"))?;
            let bi_planar = YuvBiPlanarImage {
                y_plane: y,
                y_stride,
                uv_plane: vu,
                uv_stride: vu_stride,
                width,
                height,
            };
            yuv_nv21_to_rgb(&bi_planar, &mut rgb, rgb_stride_u32, range, matrix, mode)
                .map_err(map_yuv_error)?;
        }
        RawFrame::I420 {
            y_stride,
            u_stride,
            v_stride,
            y,
            u,
            v,
        } => {
            let y_stride = u32::try_from(*y_stride)
                .map_err(|_| FrameImageError::new("I420 Y stride overflows u32"))?;
            let u_stride = u32::try_from(*u_stride)
                .map_err(|_| FrameImageError::new("I420 U stride overflows u32"))?;
            let v_stride = u32::try_from(*v_stride)
                .map_err(|_| FrameImageError::new("I420 V stride overflows u32"))?;
            let planar = YuvPlanarImage {
                y_plane: y,
                y_stride,
                u_plane: u,
                u_stride,
                v_plane: v,
                v_stride,
                width,
                height,
            };
            yuv420_to_rgb(&planar, &mut rgb, rgb_stride_u32, range, matrix)
                .map_err(map_yuv_error)?;
        }
        RawFrame::YUYV { stride, data } => {
            let (packed, packed_stride) =
                repack_packed_422(width_usize, height_usize, *stride, data, "YUYV")?;
            let packed_image = YuvPackedImage {
                yuy: packed.as_ref(),
                yuy_stride: packed_stride,
                width,
                height,
            };
            yuyv422_to_rgb(&packed_image, &mut rgb, rgb_stride_u32, range, matrix)
                .map_err(map_yuv_error)?;
        }
        RawFrame::UYVY { stride, data } => {
            let (packed, packed_stride) =
                repack_packed_422(width_usize, height_usize, *stride, data, "UYVY")?;
            let packed_image = YuvPackedImage {
                yuy: packed.as_ref(),
                yuy_stride: packed_stride,
                width,
                height,
            };
            uyvy422_to_rgb(&packed_image, &mut rgb, rgb_stride_u32, range, matrix)
                .map_err(map_yuv_error)?;
        }
    }

    let mut png_bytes = Vec::new();
    let mut encoder = Encoder::new(&mut png_bytes, width, height);
    encoder.set_color(ColorType::Rgb);
    encoder.set_depth(BitDepth::Eight);
    {
        let mut writer = encoder
            .write_header()
            .map_err(|err| FrameImageError::new(format!("png header error: {err}")))?;
        writer
            .write_image_data(&rgb)
            .map_err(|err| FrameImageError::new(format!("png encode error: {err}")))?;
    }

    Ok(Arc::new(Image::from_bytes(ImageFormat::Png, png_bytes)))
}

fn repack_packed_422<'a>(
    width: usize,
    height: usize,
    stride: usize,
    data: &'a [u8],
    label: &str,
) -> Result<(Cow<'a, [u8]>, u32), FrameImageError> {
    let packed_width = packed_422_width(width, label)?;
    if stride < packed_width {
        return Err(FrameImageError::new(format!(
            "{label} stride {stride} is smaller than packed width {packed_width}"
        )));
    }

    let packed_len = packed_width
        .checked_mul(height)
        .ok_or_else(|| FrameImageError::new(format!("{label} packed size overflow")))?;

    if stride == packed_width {
        let slice = data
            .get(0..packed_len)
            .ok_or_else(|| FrameImageError::new(format!("{label} buffer too small")))?;
        let packed_stride = u32::try_from(packed_width)
            .map_err(|_| FrameImageError::new(format!("{label} stride overflows u32")))?;
        return Ok((Cow::Borrowed(slice), packed_stride));
    }

    let mut packed = Vec::with_capacity(packed_len);
    for row in 0..height {
        let row_start = row
            .checked_mul(stride)
            .ok_or_else(|| FrameImageError::new(format!("{label} stride overflow")))?;
        let row_end = row_start
            .checked_add(packed_width)
            .ok_or_else(|| FrameImageError::new(format!("{label} stride overflow")))?;
        let row_slice = data
            .get(row_start..row_end)
            .ok_or_else(|| FrameImageError::new(format!("{label} buffer too small")))?;
        packed.extend_from_slice(row_slice);
    }

    let packed_stride = u32::try_from(packed_width)
        .map_err(|_| FrameImageError::new(format!("{label} stride overflows u32")))?;
    Ok((Cow::Owned(packed), packed_stride))
}

fn packed_422_width(width: usize, label: &str) -> Result<usize, FrameImageError> {
    let padded = if width % 2 == 0 {
        width
    } else {
        width
            .checked_add(1)
            .ok_or_else(|| FrameImageError::new(format!("{label} width overflow")))?
    };
    padded
        .checked_mul(2)
        .ok_or_else(|| FrameImageError::new(format!("{label} packed width overflow")))
}

fn map_yuv_error(err: yuv::YuvError) -> FrameImageError {
    FrameImageError::new(format!("yuv conversion error: {err}"))
}

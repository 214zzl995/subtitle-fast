use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::config::{FrameDumpConfig, ImageOutputFormat};
use crate::subtitle_detection::{DetectionRegion, SubtitleDetectionResult};
use subtitle_fast_decoder::YPlaneFrame;
use thiserror::Error;
use tokio::task;

pub(crate) struct FrameDumpOperation {
    directory: Arc<PathBuf>,
    format: ImageOutputFormat,
}

impl FrameDumpOperation {
    pub fn new(config: FrameDumpConfig) -> Self {
        Self {
            directory: Arc::from(config.directory),
            format: config.format,
        }
    }

    pub async fn process(
        &self,
        frame: &YPlaneFrame,
        detection: &SubtitleDetectionResult,
    ) -> Result<(), WriteFrameError> {
        let frame_index = frame_identifier(frame);
        write_frame(
            frame,
            frame_index,
            self.directory.as_ref(),
            self.format,
            detection,
        )
        .await
    }

    pub async fn finalize(&self) -> Result<(), WriteFrameError> {
        Ok(())
    }
}

async fn write_frame(
    frame: &YPlaneFrame,
    index: u64,
    directory: &Path,
    format: ImageOutputFormat,
    detection: &SubtitleDetectionResult,
) -> Result<(), WriteFrameError> {
    use image::codecs::jpeg::JpegEncoder;
    use image::codecs::png::PngEncoder;
    use image::codecs::webp::WebPEncoder;
    use image::{ColorType, ImageEncoder};

    let width = frame.width() as usize;
    let height = frame.height() as usize;
    if width == 0 || height == 0 {
        return Ok(());
    }
    let stride = frame.stride();
    let required = stride
        .checked_mul(height)
        .ok_or(WriteFrameError::PlaneBounds {
            stride,
            width,
            height,
        })?;
    let data = frame.data();
    if data.len() < required {
        return Err(WriteFrameError::PlaneBounds {
            stride,
            width,
            height,
        });
    }

    let mut buffer = vec![0u8; width * height];
    for (row_idx, dest_row) in buffer.chunks_mut(width).enumerate() {
        let start = row_idx * stride;
        let end = start + width;
        dest_row.copy_from_slice(&data[start..end]);
    }

    let rects = regions_to_rects(&detection.regions, width, height);

    if matches!(&format, ImageOutputFormat::Yuv) {
        if !rects.is_empty() {
            draw_rectangles_luma(&mut buffer, width, height, &rects);
        }
        let filename = format!("frame_{index}.yuv");
        let path = directory.join(filename);
        task::spawn_blocking(move || std::fs::write(path, buffer))
            .await
            .map_err(|err| {
                WriteFrameError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("join error: {err}"),
                ))
            })??;
        return Ok(());
    }

    let mut rgb_buffer = luma_to_rgb(&buffer);
    if !rects.is_empty() {
        draw_rectangles_rgb(&mut rgb_buffer, width, height, &rects);
    }

    let (encoded, extension): (Vec<u8>, &'static str) = match format {
        ImageOutputFormat::Jpeg { quality } => {
            let mut encoded = Vec::new();
            let mut encoder = JpegEncoder::new_with_quality(&mut encoded, quality);
            encoder.encode(&rgb_buffer, frame.width(), frame.height(), ColorType::Rgb8)?;
            (encoded, "jpg")
        }
        ImageOutputFormat::Png => {
            let mut encoded = Vec::new();
            let encoder = PngEncoder::new(&mut encoded);
            encoder.write_image(&rgb_buffer, frame.width(), frame.height(), ColorType::Rgb8)?;
            (encoded, "png")
        }
        ImageOutputFormat::Webp => {
            let mut encoded = Vec::new();
            let encoder = WebPEncoder::new_lossless(&mut encoded);
            encoder.encode(&rgb_buffer, frame.width(), frame.height(), ColorType::Rgb8)?;
            (encoded, "webp")
        }
        ImageOutputFormat::Yuv => unreachable!(),
    };

    let filename = format!("frame_{index}.{extension}");
    let path = directory.join(filename);
    task::spawn_blocking(move || std::fs::write(path, encoded))
        .await
        .map_err(|err| {
            WriteFrameError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("join error: {err}"),
            ))
        })??;
    Ok(())
}

#[derive(Debug, Error)]
pub(crate) enum WriteFrameError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("encoding error: {0}")]
    Encode(#[from] image::ImageError),
    #[error("invalid plane dimensions stride={stride} width={width} height={height}")]
    PlaneBounds {
        stride: usize,
        width: usize,
        height: usize,
    },
}

#[derive(Clone, Copy)]
struct Rect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

fn regions_to_rects(
    regions: &[DetectionRegion],
    frame_width: usize,
    frame_height: usize,
) -> Vec<Rect> {
    let mut rects = Vec::new();
    if frame_width == 0 || frame_height == 0 {
        return rects;
    }

    for region in regions {
        let mut start_x = region.x.floor() as i32;
        let mut start_y = region.y.floor() as i32;
        let mut end_x = (region.x + region.width).ceil() as i32;
        let mut end_y = (region.y + region.height).ceil() as i32;

        if end_x <= 0 || end_y <= 0 {
            continue;
        }

        start_x = start_x.max(0);
        start_y = start_y.max(0);
        end_x = end_x.min(frame_width as i32);
        end_y = end_y.min(frame_height as i32);

        if start_x >= frame_width as i32 || start_y >= frame_height as i32 {
            continue;
        }

        if end_x <= start_x || end_y <= start_y {
            continue;
        }

        rects.push(Rect {
            x0: start_x as usize,
            y0: start_y as usize,
            x1: (end_x - 1) as usize,
            y1: (end_y - 1) as usize,
        });
    }

    rects
}

fn frame_identifier(frame: &YPlaneFrame) -> u64 {
    frame
        .frame_index()
        .or_else(|| frame.timestamp().map(duration_millis))
        .unwrap_or_default()
}

fn duration_millis(duration: Duration) -> u64 {
    let millis = duration.as_millis();
    if millis > u64::MAX as u128 {
        u64::MAX
    } else {
        millis as u64
    }
}

fn draw_rectangles_luma(buffer: &mut [u8], width: usize, height: usize, rects: &[Rect]) {
    let stride = width;
    for rect in rects {
        let thickness = rect_thickness(rect, width, height);
        for offset in 0..thickness {
            let top = rect.y0.saturating_add(offset);
            if top > rect.y1 {
                break;
            }
            let bottom = rect.y1.saturating_sub(offset);
            if bottom >= height {
                continue;
            }
            for x in rect.x0..=rect.x1 {
                if top < height {
                    buffer[top * stride + x] = 255;
                }
                if bottom < height {
                    buffer[bottom * stride + x] = 255;
                }
            }

            let left = rect.x0.saturating_add(offset);
            if left > rect.x1 {
                break;
            }
            let right = rect.x1.saturating_sub(offset);
            for y in rect.y0..=rect.y1 {
                if y < height {
                    buffer[y * stride + left] = 255;
                }
                if right < width && y < height {
                    buffer[y * stride + right] = 255;
                }
            }
        }
    }
}

fn draw_rectangles_rgb(buffer: &mut [u8], width: usize, height: usize, rects: &[Rect]) {
    let stride = width * 3;
    for rect in rects {
        let thickness = rect_thickness(rect, width, height);
        for offset in 0..thickness {
            let top = rect.y0.saturating_add(offset);
            if top > rect.y1 {
                break;
            }
            let bottom = rect.y1.saturating_sub(offset);
            for x in rect.x0..=rect.x1 {
                if top < height {
                    tint_pixel(buffer, stride, top, x, [255, 64, 64]);
                }
                if bottom < height {
                    tint_pixel(buffer, stride, bottom, x, [255, 64, 64]);
                }
            }

            let left = rect.x0.saturating_add(offset);
            if left > rect.x1 {
                break;
            }
            let right = rect.x1.saturating_sub(offset);
            for y in rect.y0..=rect.y1 {
                if y < height {
                    tint_pixel(buffer, stride, y, left, [255, 64, 64]);
                    tint_pixel(buffer, stride, y, right, [255, 64, 64]);
                }
            }
        }
    }
}

fn tint_pixel(buffer: &mut [u8], stride: usize, y: usize, x: usize, color: [u8; 3]) {
    let idx = y * stride + x * 3;
    if idx + 2 >= buffer.len() {
        return;
    }
    buffer[idx] = color[0];
    buffer[idx + 1] = color[1];
    buffer[idx + 2] = color[2];
}

fn rect_thickness(rect: &Rect, width: usize, height: usize) -> usize {
    let max_possible =
        ((rect.x1.saturating_sub(rect.x0)).min(rect.y1.saturating_sub(rect.y0))).max(1);
    max_possible.min(2).min(width.max(height))
}

fn luma_to_rgb(buffer: &[u8]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(buffer.len() * 3);
    for &value in buffer {
        rgb.push(value);
        rgb.push(value);
        rgb.push(value);
    }
    rgb
}

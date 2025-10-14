use std::path::{Path, PathBuf};

use crate::cli::DumpFormat;
use crate::output::error::OutputError;
use crate::output::types::{FrameAnalysisSample, frame_identifier};
use crate::settings::ImageDumpSettings;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::{ColorType, ImageEncoder};
use subtitle_fast_validator::subtitle_detection::DetectionRegion;
use tokio::task;

pub(crate) struct ImageOutput {
    directory: PathBuf,
    format: DumpFormat,
}

impl ImageOutput {
    pub(crate) fn new(settings: ImageDumpSettings) -> Self {
        Self {
            directory: settings.dir,
            format: settings.format,
        }
    }

    pub(crate) async fn write(&self, sample: &FrameAnalysisSample) -> Result<(), OutputError> {
        let frame = &sample.frame;
        let detection = &sample.detection;
        let frame_index = frame_identifier(frame);
        write_frame(frame, frame_index, &self.directory, self.format, detection).await
    }
}

async fn write_frame(
    frame: &subtitle_fast_decoder::YPlaneFrame,
    index: u64,
    directory: &Path,
    format: DumpFormat,
    detection: &subtitle_fast_validator::subtitle_detection::SubtitleDetectionResult,
) -> Result<(), OutputError> {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    if width == 0 || height == 0 {
        return Ok(());
    }
    let stride = frame.stride();
    let required = stride.checked_mul(height).ok_or_else(|| {
        OutputError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "stride overflow",
        ))
    })?;
    let data = frame.data();
    if data.len() < required {
        return Err(OutputError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "invalid plane length: got {} expected at least {}",
                data.len(),
                required
            ),
        )));
    }

    let mut buffer = vec![0u8; width * height];
    for (row_idx, dest_row) in buffer.chunks_mut(width).enumerate() {
        let start = row_idx * stride;
        let end = start + width;
        dest_row.copy_from_slice(&data[start..end]);
    }

    let rects = regions_to_rects(&detection.regions, width, height);

    if matches!(format, DumpFormat::Yuv) {
        if !rects.is_empty() {
            draw_rectangles_luma(&mut buffer, width, height, &rects);
        }
        let filename = format!("frame_{index}.yuv");
        let path = directory.join(filename);
        let buffer = buffer;
        task::spawn_blocking(move || std::fs::write(path, buffer))
            .await
            .map_err(|err| {
                OutputError::Io(std::io::Error::new(
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
        DumpFormat::Jpeg => {
            let mut encoded = Vec::new();
            let mut encoder = JpegEncoder::new_with_quality(&mut encoded, 90);
            encoder.encode(&rgb_buffer, frame.width(), frame.height(), ColorType::Rgb8)?;
            (encoded, "jpg")
        }
        DumpFormat::Png => {
            let mut encoded = Vec::new();
            let encoder = PngEncoder::new(&mut encoded);
            encoder.write_image(&rgb_buffer, frame.width(), frame.height(), ColorType::Rgb8)?;
            (encoded, "png")
        }
        DumpFormat::Webp => {
            let mut encoded = Vec::new();
            let encoder = WebPEncoder::new_lossless(&mut encoded);
            encoder.encode(&rgb_buffer, frame.width(), frame.height(), ColorType::Rgb8)?;
            (encoded, "webp")
        }
        DumpFormat::Yuv => unreachable!(),
    };

    let filename = format!("frame_{index}.{extension}");
    let path = directory.join(filename);
    task::spawn_blocking(move || std::fs::write(path, encoded))
        .await
        .map_err(|err| {
            OutputError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("join error: {err}"),
            ))
        })??;
    Ok(())
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

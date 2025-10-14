use std::path::{Path, PathBuf};

use crate::cli::DumpFormat;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::{ColorType, ImageEncoder};
use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_validator::subtitle_detection::{
    DetectionRegion, RoiConfig, SubtitleDetectionResult,
};
use tokio::task;

pub struct YPlaneComposer;

pub struct YPlaneComposite {
    pub width: u32,
    pub height: u32,
    pub luma: Vec<u8>,
    pub rgb: Option<Vec<u8>>,
}

impl YPlaneComposer {
    pub fn new() -> Self {
        Self
    }

    pub fn compose(
        &self,
        frame: &YPlaneFrame,
        detection: &SubtitleDetectionResult,
        roi: Option<RoiConfig>,
    ) -> YPlaneComposite {
        let width = frame.width() as usize;
        let height = frame.height() as usize;
        let stride = frame.stride();

        let mut luma = vec![0u8; width * height];
        for (row_idx, dst) in luma.chunks_mut(width).enumerate() {
            let start = row_idx * stride;
            let end = start + width;
            dst.copy_from_slice(&frame.data()[start..end]);
        }

        let mut rects = regions_to_rects(&detection.regions, width, height);
        if let Some(roi) = roi {
            rects.push(roi_to_rect(roi, width, height));
        }

        if !rects.is_empty() {
            draw_rectangles_luma(&mut luma, width, height, &rects);
        }

        let rgb = if rects.is_empty() {
            None
        } else {
            let mut rgb = luma_to_rgb(&luma);
            draw_rectangles_rgb(&mut rgb, width, height, &rects);
            Some(rgb)
        };

        YPlaneComposite {
            width: frame.width(),
            height: frame.height(),
            luma,
            rgb,
        }
    }
}

pub struct YPlaneSaver {
    directory: PathBuf,
    format: DumpFormat,
    composer: YPlaneComposer,
}

impl YPlaneSaver {
    pub fn new(directory: PathBuf, format: DumpFormat) -> Self {
        Self {
            directory,
            format,
            composer: YPlaneComposer::new(),
        }
    }

    pub async fn save(
        &self,
        frame: &YPlaneFrame,
        detection: &SubtitleDetectionResult,
        roi: Option<RoiConfig>,
        index: u64,
    ) -> std::io::Result<()> {
        let composite = self.composer.compose(frame, detection, roi);

        match self.format {
            DumpFormat::Yuv => write_yuv(&self.directory, index, composite.luma).await,
            DumpFormat::Jpeg => {
                let rgb = composite
                    .rgb
                    .unwrap_or_else(|| luma_to_rgb(&composite.luma));
                write_encoded(&self.directory, index, "jpg", |writer| {
                    let mut encoder = JpegEncoder::new_with_quality(writer, 90);
                    encoder.encode(&rgb, composite.width, composite.height, ColorType::Rgb8)
                })
                .await
            }
            DumpFormat::Png => {
                let rgb = composite
                    .rgb
                    .unwrap_or_else(|| luma_to_rgb(&composite.luma));
                write_encoded(&self.directory, index, "png", |writer| {
                    let encoder = PngEncoder::new(writer);
                    encoder.write_image(&rgb, composite.width, composite.height, ColorType::Rgb8)
                })
                .await
            }
            DumpFormat::Webp => {
                let rgb = composite
                    .rgb
                    .unwrap_or_else(|| luma_to_rgb(&composite.luma));
                write_encoded(&self.directory, index, "webp", |writer| {
                    let encoder = WebPEncoder::new_lossless(writer);
                    encoder.encode(&rgb, composite.width, composite.height, ColorType::Rgb8)
                })
                .await
            }
        }
    }
}

async fn write_yuv(dir: &Path, index: u64, data: Vec<u8>) -> std::io::Result<()> {
    let path = dir.join(format!("frame_{index}.yuv"));
    task::spawn_blocking(move || std::fs::write(path, data))
        .await
        .map_err(join_error_to_io)?
}

async fn write_encoded<F>(
    dir: &Path,
    index: u64,
    extension: &str,
    encoder: F,
) -> std::io::Result<()>
where
    F: FnOnce(&mut Vec<u8>) -> image::ImageResult<()>,
{
    let mut buffer = Vec::new();
    encoder(&mut buffer).map_err(image_error_to_io)?;
    let path = dir.join(format!("frame_{index}.{extension}"));
    task::spawn_blocking(move || std::fs::write(path, buffer))
        .await
        .map_err(join_error_to_io)?
}

fn join_error_to_io(err: tokio::task::JoinError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, err)
}

fn image_error_to_io(err: image::ImageError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, err)
}

fn regions_to_rects(
    regions: &[DetectionRegion],
    frame_width: usize,
    frame_height: usize,
) -> Vec<Rect> {
    regions
        .iter()
        .filter_map(|region| region_to_rect(region, frame_width, frame_height))
        .collect()
}

fn region_to_rect(
    region: &DetectionRegion,
    frame_width: usize,
    frame_height: usize,
) -> Option<Rect> {
    let mut start_x = region.x.floor() as i32;
    let mut start_y = region.y.floor() as i32;
    let mut end_x = (region.x + region.width).ceil() as i32;
    let mut end_y = (region.y + region.height).ceil() as i32;

    if end_x <= 0 || end_y <= 0 {
        return None;
    }

    start_x = start_x.clamp(0, frame_width as i32 - 1);
    start_y = start_y.clamp(0, frame_height as i32 - 1);
    end_x = end_x.clamp(0, frame_width as i32);
    end_y = end_y.clamp(0, frame_height as i32);

    if end_x - start_x < 1 || end_y - start_y < 1 {
        return None;
    }

    Some(Rect {
        x0: start_x as usize,
        y0: start_y as usize,
        x1: end_x as usize,
        y1: end_y as usize,
    })
}

fn roi_to_rect(roi: RoiConfig, frame_width: usize, frame_height: usize) -> Rect {
    let x0 = (roi.x * frame_width as f32).clamp(0.0, frame_width as f32);
    let y0 = (roi.y * frame_height as f32).clamp(0.0, frame_height as f32);
    let x1 = ((roi.x + roi.width) * frame_width as f32).clamp(0.0, frame_width as f32);
    let y1 = ((roi.y + roi.height) * frame_height as f32).clamp(0.0, frame_height as f32);

    Rect {
        x0: x0 as usize,
        y0: y0 as usize,
        x1: x1.max(x0 + 1.0) as usize,
        y1: y1.max(y0 + 1.0) as usize,
    }
}

#[derive(Clone, Copy)]
struct Rect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

fn draw_rectangles_luma(buffer: &mut [u8], width: usize, height: usize, rects: &[Rect]) {
    for rect in rects {
        draw_rect_luma(buffer, width, height, *rect);
    }
}

fn draw_rectangles_rgb(buffer: &mut [u8], width: usize, height: usize, rects: &[Rect]) {
    for rect in rects {
        draw_rect_rgb(buffer, width, height, *rect);
    }
}

fn draw_rect_luma(buffer: &mut [u8], width: usize, height: usize, rect: Rect) {
    for x in rect.x0..rect.x1 {
        set_luma(buffer, width, height, x, rect.y0, 0);
        set_luma(buffer, width, height, x, rect.y1.saturating_sub(1), 0);
    }
    for y in rect.y0..rect.y1 {
        set_luma(buffer, width, height, rect.x0, y, 0);
        set_luma(buffer, width, height, rect.x1.saturating_sub(1), y, 0);
    }
}

fn draw_rect_rgb(buffer: &mut [u8], width: usize, height: usize, rect: Rect) {
    for x in rect.x0..rect.x1 {
        set_rgb(buffer, width, height, x, rect.y0, [255, 0, 0]);
        set_rgb(
            buffer,
            width,
            height,
            x,
            rect.y1.saturating_sub(1),
            [255, 0, 0],
        );
    }
    for y in rect.y0..rect.y1 {
        set_rgb(buffer, width, height, rect.x0, y, [255, 0, 0]);
        set_rgb(
            buffer,
            width,
            height,
            rect.x1.saturating_sub(1),
            y,
            [255, 0, 0],
        );
    }
}

fn set_luma(buffer: &mut [u8], width: usize, height: usize, x: usize, y: usize, value: u8) {
    if x >= width || y >= height {
        return;
    }
    buffer[y * width + x] = value;
}

fn set_rgb(buffer: &mut [u8], width: usize, height: usize, x: usize, y: usize, value: [u8; 3]) {
    if x >= width || y >= height {
        return;
    }
    let offset = (y * width + x) * 3;
    buffer[offset..offset + 3].copy_from_slice(&value);
}

fn luma_to_rgb(luma: &[u8]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(luma.len() * 3);
    for &value in luma {
        rgb.extend_from_slice(&[value, value, value]);
    }
    rgb
}

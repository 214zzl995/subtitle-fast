use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::{FrameDumpConfig, FrameMetadata, ImageOutputFormat};
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
        metadata: &FrameMetadata,
    ) -> Result<(), WriteFrameError> {
        write_frame(
            frame,
            metadata.frame_index,
            self.directory.as_ref(),
            self.format,
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

    let (encoded, extension): (Vec<u8>, &'static str) = match format {
        ImageOutputFormat::Jpeg { quality } => {
            let mut encoded = Vec::new();
            let mut encoder = JpegEncoder::new_with_quality(&mut encoded, quality);
            encoder.encode(&buffer, frame.width(), frame.height(), ColorType::L8)?;
            (encoded, "jpg")
        }
        ImageOutputFormat::Png => {
            let mut encoded = Vec::new();
            let encoder = PngEncoder::new(&mut encoded);
            encoder.write_image(&buffer, frame.width(), frame.height(), ColorType::L8)?;
            (encoded, "png")
        }
        ImageOutputFormat::Webp => {
            let mut encoded = Vec::new();
            let encoder = WebPEncoder::new_lossless(&mut encoded);
            encoder.encode(&buffer, frame.width(), frame.height(), ColorType::L8)?;
            (encoded, "webp")
        }
        ImageOutputFormat::Yuv => (buffer, "yuv"),
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

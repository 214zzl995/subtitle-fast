use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::{FrameDumpConfig, FrameMetadata, ImageOutputFormat};
use subtitle_fast_decoder::YPlaneFrame;
use thiserror::Error;
use tokio::sync::Mutex;
use tokio::task;

pub(crate) struct FrameDumpOperation {
    directory: Arc<PathBuf>,
    format: ImageOutputFormat,
    state: Mutex<FrameDumpState>,
}

impl FrameDumpOperation {
    pub fn new(config: FrameDumpConfig) -> Self {
        Self {
            directory: Arc::from(config.directory),
            format: config.format,
            state: Mutex::new(FrameDumpState::new(config.samples_per_second)),
        }
    }

    pub async fn process(
        &self,
        frame: &YPlaneFrame,
        metadata: &FrameMetadata,
    ) -> Result<(), WriteFrameError> {
        let ready = {
            let mut state = self.state.lock().await;
            state.enqueue_frame(frame.clone(), metadata.clone())
        };

        self.write_batch(ready).await
    }

    pub async fn finalize(&self) -> Result<(), WriteFrameError> {
        let remaining = {
            let mut state = self.state.lock().await;
            state.drain_pending()
        };

        self.write_batch(remaining).await
    }

    async fn write_batch(&self, frames: Vec<QueuedDumpFrame>) -> Result<(), WriteFrameError> {
        for frame in frames {
            write_frame(
                &frame.frame,
                frame.metadata.frame_index,
                self.directory.as_ref(),
                self.format,
            )
            .await?;
        }
        Ok(())
    }
}

struct FrameDumpState {
    sampler: FrameDumpSampler,
    pending: BTreeMap<u64, QueuedDumpFrame>,
    next_processed_index: u64,
}

impl FrameDumpState {
    fn new(samples_per_second: u32) -> Self {
        Self {
            sampler: FrameDumpSampler::new(samples_per_second),
            pending: BTreeMap::new(),
            next_processed_index: 1,
        }
    }

    fn enqueue_frame(
        &mut self,
        frame: YPlaneFrame,
        metadata: FrameMetadata,
    ) -> Vec<QueuedDumpFrame> {
        let index = metadata.processed_index;
        self.pending
            .insert(index, QueuedDumpFrame { frame, metadata });
        self.collect_ready()
    }

    fn drain_pending(&mut self) -> Vec<QueuedDumpFrame> {
        let mut drained = Vec::new();
        while !self.pending.is_empty() {
            let ready = self.collect_ready();
            if ready.is_empty() {
                if let Some(&next_index) = self.pending.keys().next() {
                    self.next_processed_index = next_index;
                }
            } else {
                drained.extend(ready);
            }
        }
        drained
    }

    fn collect_ready(&mut self) -> Vec<QueuedDumpFrame> {
        let mut ready = Vec::new();
        loop {
            let index = self.next_processed_index;
            match self.pending.remove(&index) {
                Some(frame) => {
                    self.next_processed_index = self.next_processed_index.saturating_add(1);
                    if self.sampler.should_write(&frame.frame, &frame.metadata) {
                        ready.push(frame);
                    }
                }
                None => break,
            }
        }
        ready
    }
}

struct QueuedDumpFrame {
    frame: YPlaneFrame,
    metadata: FrameMetadata,
}

struct FrameDumpSampler {
    samples_per_second: u32,
    current: Option<SamplerSecond>,
}

impl FrameDumpSampler {
    fn new(samples_per_second: u32) -> Self {
        Self {
            samples_per_second: samples_per_second.max(1),
            current: None,
        }
    }

    fn should_write(&mut self, frame: &YPlaneFrame, metadata: &FrameMetadata) -> bool {
        let (second_index, elapsed) = self.resolve_second(frame, metadata);

        if self.current.as_ref().map(|second| second.index) != Some(second_index) {
            self.current = Some(SamplerSecond::new(second_index, self.samples_per_second));
        }

        let Some(current) = self.current.as_mut() else {
            return false;
        };

        current.consume(elapsed)
    }

    fn resolve_second(&self, frame: &YPlaneFrame, metadata: &FrameMetadata) -> (u64, f64) {
        use std::time::Duration;

        if let Some(timestamp) = metadata.timestamp.or_else(|| frame.timestamp()) {
            let second_index = timestamp.as_secs();
            let elapsed = timestamp
                .checked_sub(std::time::Duration::from_secs(second_index))
                .unwrap_or_else(|| Duration::from_secs(0))
                .as_secs_f64();
            return (second_index, elapsed);
        }

        let samples = self.samples_per_second.max(1) as u64;
        let processed = metadata.processed_index.saturating_sub(1);
        let second_index = processed / samples;
        let offset = processed.saturating_sub(second_index * samples);
        let elapsed = offset as f64 / self.samples_per_second.max(1) as f64;
        (second_index, elapsed)
    }
}

struct SamplerSecond {
    index: u64,
    targets: Vec<f64>,
    next_target_idx: usize,
}

impl SamplerSecond {
    fn new(index: u64, samples_per_second: u32) -> Self {
        let slots = samples_per_second.max(1) as usize;
        let mut targets = Vec::with_capacity(slots);
        for i in 0..slots {
            if i == 0 {
                targets.push(0.0);
            } else {
                targets.push(i as f64 / samples_per_second as f64);
            }
        }
        Self {
            index,
            targets,
            next_target_idx: 0,
        }
    }

    fn consume(&mut self, elapsed: f64) -> bool {
        if self.targets.is_empty() {
            return false;
        }

        let mut should_write = false;
        let epsilon = 1e-6f64;

        while self.next_target_idx < self.targets.len()
            && elapsed + epsilon >= self.targets[self.next_target_idx]
        {
            should_write = true;
            self.next_target_idx += 1;
        }

        should_write
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

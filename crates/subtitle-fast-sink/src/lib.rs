use std::path::PathBuf;
use std::sync::Arc;

use image::codecs::jpeg::JpegEncoder;
use image::ColorType;
use subtitle_fast_decoder::YPlaneFrame;

pub mod subtitle_detection;
pub use subtitle_detection::{
    LogisticWeights, SubtitleDetectionConfig, SubtitleDetectionError, SubtitleDetectionResult,
    SubtitlePresenceDetector,
};
use thiserror::Error;
use tokio::sync::{
    mpsc::{self, Sender},
    Semaphore,
};
use tokio::task::{self, JoinHandle, JoinSet};

const DEFAULT_CHANNEL_CAPACITY: usize = 64;
const DEFAULT_MAX_CONCURRENCY: usize = 8;

pub struct FrameSink {
    sender: Sender<Job>,
    worker: JoinHandle<()>,
}

struct Job {
    frame: YPlaneFrame,
    index: u64,
}

#[derive(Clone, Debug)]
pub struct JpegOptions {
    pub quality: u8,
    pub channel_capacity: usize,
    pub max_concurrency: usize,
}

impl Default for JpegOptions {
    fn default() -> Self {
        Self {
            quality: 90,
            channel_capacity: DEFAULT_CHANNEL_CAPACITY,
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
        }
    }
}

impl FrameSink {
    pub fn jpeg_writer(dir: PathBuf, options: JpegOptions) -> Self {
        let capacity = options.channel_capacity.max(1);
        let concurrency = options.max_concurrency.max(1);
        let (tx, mut rx) = mpsc::channel::<Job>(capacity);
        let directory = Arc::new(dir);
        let quality = options.quality;
        let semaphore = Arc::new(Semaphore::new(concurrency));

        let worker = tokio::spawn({
            let directory = Arc::clone(&directory);
            let semaphore = Arc::clone(&semaphore);
            async move {
                let mut tasks = JoinSet::new();
                while let Some(job) = rx.recv().await {
                    let directory = Arc::clone(&directory);
                    let semaphore = Arc::clone(&semaphore);
                    tasks.spawn(async move {
                        let permit = match semaphore.acquire_owned().await {
                            Ok(permit) => permit,
                            Err(err) => {
                                eprintln!("frame sink semaphore error: {err}");
                                return;
                            }
                        };

                        let _permit = permit;
                        if let Err(err) =
                            write_frame(job.frame, job.index, directory, quality).await
                        {
                            eprintln!("frame sink worker error: {err}");
                        }
                    });
                }

                while let Some(result) = tasks.join_next().await {
                    match result {
                        Ok(()) => {}
                        Err(err) if err.is_cancelled() => {}
                        Err(err) => {
                            eprintln!("frame sink join error: {err}");
                        }
                    }
                }
            }
        });

        Self { sender: tx, worker }
    }

    pub fn push(&self, frame: YPlaneFrame, index: u64) -> bool {
        self.sender.try_send(Job { frame, index }).is_ok()
    }

    pub async fn shutdown(self) {
        let FrameSink { sender, worker } = self;
        drop(sender);
        let _ = worker.await;
    }
}

async fn write_frame(
    frame: YPlaneFrame,
    index: u64,
    directory: Arc<PathBuf>,
    quality: u8,
) -> Result<(), WriteFrameError> {
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

    let mut encoded = Vec::new();
    {
        let mut encoder = JpegEncoder::new_with_quality(&mut encoded, quality);
        encoder.encode(&buffer, frame.width(), frame.height(), ColorType::L8)?;
    }

    let filename = format!("frame_{index}.jpg");
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
enum WriteFrameError {
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

use std::sync::Arc;

use crate::config::{FrameMetadata, FrameSinkConfig};
use crate::detection::SubtitleDetectionOperation;
use crate::dump::FrameDumpOperation;
use subtitle_fast_decoder::YPlaneFrame;
use thiserror::Error;
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;

pub type FrameSinkProgress = mpsc::UnboundedReceiver<FrameMetadata>;

pub struct FrameSink {
    sender: mpsc::Sender<Job>,
    worker: tokio::task::JoinHandle<()>,
}

impl FrameSink {
    pub fn new(config: FrameSinkConfig) -> (Self, FrameSinkProgress) {
        let capacity = config.channel_capacity.max(1);
        let concurrency = config.max_concurrency.max(1);
        let (progress_tx, progress_rx) = mpsc::unbounded_channel();
        let operations = Arc::new(ProcessingOperations::new(config, progress_tx));
        let (sender, mut rx) = mpsc::channel::<Job>(capacity);
        let semaphore = Arc::new(Semaphore::new(concurrency));
        let worker_operations = Arc::clone(&operations);
        let worker_semaphore = Arc::clone(&semaphore);

        let worker = tokio::spawn(async move {
            let operations = worker_operations;
            let semaphore = worker_semaphore;
            let mut tasks = JoinSet::new();

            while let Some(job) = rx.recv().await {
                let permit = match semaphore.clone().acquire_owned().await {
                    Ok(permit) => permit,
                    Err(err) => {
                        eprintln!("frame sink semaphore error: {err}");
                        break;
                    }
                };

                let operations = Arc::clone(&operations);
                tasks.spawn(async move {
                    let Job { frame, metadata } = job;
                    let _permit = permit;
                    operations.process_frame(frame, metadata).await;
                });
            }

            while let Some(result) = tasks.join_next().await {
                if let Err(err) = result {
                    if !err.is_cancelled() {
                        eprintln!("frame sink join error: {err}");
                    }
                }
            }

            operations.finalize().await;
        });

        (Self { sender, worker }, progress_rx)
    }

    pub async fn submit(
        &self,
        frame: YPlaneFrame,
        metadata: FrameMetadata,
    ) -> Result<(), FrameSinkError> {
        self.sender
            .send(Job { frame, metadata })
            .await
            .map_err(|_| FrameSinkError::Stopped)
    }

    pub async fn shutdown(self) -> Result<(), FrameSinkError> {
        drop(self.sender);
        match self.worker.await {
            Ok(()) => Ok(()),
            Err(err) => {
                if !err.is_cancelled() {
                    eprintln!("frame sink worker task error: {err}");
                }
                Err(FrameSinkError::Stopped)
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum FrameSinkError {
    #[error("frame sink worker stopped")]
    Stopped,
}

struct Job {
    frame: YPlaneFrame,
    metadata: FrameMetadata,
}

struct ProcessingOperations {
    dump: Option<Arc<FrameDumpOperation>>,
    detection: Option<Arc<SubtitleDetectionOperation>>,
    progress: mpsc::UnboundedSender<FrameMetadata>,
}

impl ProcessingOperations {
    fn new(config: FrameSinkConfig, progress: mpsc::UnboundedSender<FrameMetadata>) -> Self {
        let FrameSinkConfig {
            dump, detection, ..
        } = config;

        let dump = dump.map(|cfg| Arc::new(FrameDumpOperation::new(cfg)));
        let detection = if detection.enabled {
            Some(Arc::new(SubtitleDetectionOperation::new(detection)))
        } else {
            None
        };
        Self {
            dump,
            detection,
            progress,
        }
    }

    async fn process_frame(&self, frame: YPlaneFrame, metadata: FrameMetadata) {
        if let Some(dump) = self.dump.as_ref() {
            if let Err(err) = dump.process(&frame, &metadata).await {
                eprintln!("frame sink dump error: {err}");
            }
        }

        if let Some(detection) = self.detection.as_ref() {
            detection.process(&frame, &metadata).await;
        }

        let _ = self.progress.send(metadata);
    }

    async fn finalize(&self) {
        if let Some(dump) = self.dump.as_ref() {
            if let Err(err) = dump.finalize().await {
                eprintln!("frame sink dump finalize error: {err}");
            }
        }

        if let Some(detection) = self.detection.as_ref() {
            detection.finalize().await;
        }
    }
}

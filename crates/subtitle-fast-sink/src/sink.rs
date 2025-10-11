use std::sync::Arc;

use crate::config::{FrameMetadata, FrameSinkConfig};
use crate::detection::SubtitleDetectionPipeline;
use crate::sampler::FrameSampleCoordinator;
use crate::sampler::SampledFrame;
use crate::subtitle_detection::SubtitleDetectionError;
use subtitle_fast_decoder::YPlaneFrame;
use thiserror::Error;
use tokio::sync::{mpsc, Mutex, Semaphore};
use tokio::task::JoinSet;

pub type FrameSinkProgress = mpsc::UnboundedReceiver<FrameMetadata>;

pub struct FrameSink {
    sender: mpsc::Sender<Job>,
    worker: tokio::task::JoinHandle<()>,
}

impl FrameSink {
    pub fn new(
        config: FrameSinkConfig,
    ) -> Result<(Self, FrameSinkProgress), SubtitleDetectionError> {
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

        Ok((Self { sender, worker }, progress_rx))
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
    detection: Option<Arc<SubtitleDetectionPipeline>>,
    sampler: Mutex<FrameSampleCoordinator>,
    progress: mpsc::UnboundedSender<FrameMetadata>,
}

impl ProcessingOperations {
    fn new(config: FrameSinkConfig, progress: mpsc::UnboundedSender<FrameMetadata>) -> Self {
        let FrameSinkConfig { detection, .. } = config;

        let samples_per_second = detection.samples_per_second.max(1);
        let sampler = FrameSampleCoordinator::new(samples_per_second);
        let detection = SubtitleDetectionPipeline::from_options(detection).map(Arc::new);
        Self {
            detection,
            sampler: Mutex::new(sampler),
            progress,
        }
    }

    async fn process_frame(&self, frame: YPlaneFrame, metadata: FrameMetadata) {
        let samples = {
            let mut sampler = self.sampler.lock().await;
            sampler.enqueue(frame, metadata)
        };

        self.process_samples(samples).await;
    }

    async fn finalize(&self) {
        let remaining = {
            let mut sampler = self.sampler.lock().await;
            sampler.drain()
        };

        self.process_samples(remaining).await;

        if let Some(detection) = self.detection.as_ref() {
            detection.finalize().await;
        }
    }

    async fn process_samples(&self, samples: Vec<SampledFrame>) {
        for sample in samples {
            let SampledFrame { frame, metadata } = sample;

            if let Some(detection) = self.detection.as_ref() {
                detection.process(&frame, &metadata).await;
            }

            let _ = self.progress.send(metadata);
        }
    }
}

use std::sync::Arc;

use crate::config::{FrameMetadata, FrameValidatorConfig};
use crate::detection::SubtitleDetectionPipeline;
use crate::sampler::{FrameSampleCoordinator, SampledFrame};
use crate::subtitle_detection::SubtitleDetectionError;
use subtitle_fast_decoder::YPlaneFrame;
use tokio::sync::Mutex;

#[derive(Clone)]
/// Validates sampled subtitle frames and optional detection pipelines.
pub struct FrameValidator {
    operations: Arc<ProcessingOperations>,
}

impl FrameValidator {
    pub fn new(config: FrameValidatorConfig) -> Result<Self, SubtitleDetectionError> {
        let operations = ProcessingOperations::new(config);
        Ok(Self {
            operations: Arc::new(operations),
        })
    }

    pub async fn process_frame(&self, frame: YPlaneFrame, metadata: FrameMetadata) {
        self.operations.process_frame(frame, metadata).await;
    }

    pub async fn finalize(&self) {
        self.operations.finalize().await;
    }
}

struct ProcessingOperations {
    detection: Option<Arc<SubtitleDetectionPipeline>>,
    sampler: Mutex<FrameSampleCoordinator>,
}

impl ProcessingOperations {
    fn new(config: FrameValidatorConfig) -> Self {
        let FrameValidatorConfig { detection } = config;
        let samples_per_second = detection.samples_per_second.max(1);
        let sampler = FrameSampleCoordinator::new(samples_per_second);
        let detection = SubtitleDetectionPipeline::from_options(detection).map(Arc::new);
        Self {
            detection,
            sampler: Mutex::new(sampler),
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
        }
    }
}

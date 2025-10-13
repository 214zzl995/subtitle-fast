use std::sync::Arc;

use crate::config::{FrameMetadata, FrameValidatorConfig};
use crate::detection::SubtitleDetectionPipeline;
use crate::subtitle_detection::SubtitleDetectionError;
use crate::subtitle_detection::SubtitleDetectionResult;
use subtitle_fast_decoder::YPlaneFrame;

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

    pub async fn process_frame(
        &self,
        frame: YPlaneFrame,
        metadata: FrameMetadata,
    ) -> Option<SubtitleDetectionResult> {
        self.operations.process_frame(frame, metadata).await
    }

    pub async fn finalize(&self) {
        self.operations.finalize().await;
    }
}

struct ProcessingOperations {
    detection: Option<Arc<SubtitleDetectionPipeline>>,
}

impl ProcessingOperations {
    fn new(config: FrameValidatorConfig) -> Self {
        let FrameValidatorConfig { detection } = config;
        let detection = SubtitleDetectionPipeline::from_options(detection).map(Arc::new);
        Self { detection }
    }

    async fn process_frame(
        &self,
        frame: YPlaneFrame,
        metadata: FrameMetadata,
    ) -> Option<SubtitleDetectionResult> {
        if let Some(pipeline) = self.detection.as_ref() {
            pipeline.process(&frame, &metadata).await
        } else {
            None
        }
    }

    async fn finalize(&self) {
        if let Some(detection) = self.detection.as_ref() {
            detection.finalize().await;
        }
    }
}

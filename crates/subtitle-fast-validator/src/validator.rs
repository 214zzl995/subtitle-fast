use std::sync::Arc;

use crate::config::FrameValidatorConfig;
use crate::detection::SubtitleDetectionPipeline;
use crate::subtitle_detection::{SubtitleDetectionError, SubtitleDetectionResult};
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
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        self.operations.process_frame(frame).await
    }

    pub async fn finalize(&self) -> Result<(), SubtitleDetectionError> {
        self.operations.finalize().await
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
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        if let Some(pipeline) = self.detection.as_ref() {
            pipeline.process(&frame).await
        } else {
            Ok(SubtitleDetectionResult::empty())
        }
    }

    async fn finalize(&self) -> Result<(), SubtitleDetectionError> {
        if let Some(detection) = self.detection.as_ref() {
            detection.finalize().await
        } else {
            Ok(())
        }
    }
}

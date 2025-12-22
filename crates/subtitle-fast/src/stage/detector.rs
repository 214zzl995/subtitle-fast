use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream::unfold};
use tokio::sync::mpsc;

use super::StreamBundle;
use super::sampler::{SampledFrame, SamplerResult};
use crate::settings::DetectionSettings;
use subtitle_fast_types::{FrameError, SubtitleDetectionResult};
use subtitle_fast_validator::subtitle_detection::SubtitleDetectionError;
use subtitle_fast_validator::{FrameValidator, FrameValidatorConfig, SubtitleDetectionOptions};

const DETECTOR_CHANNEL_CAPACITY: usize = 2;
pub type DetectionSampleResult = Result<DetectionSample, DetectorError>;

pub struct DetectionSample {
    pub sample: SampledFrame,
    pub detection: SubtitleDetectionResult,
    pub elapsed: Duration,
}

#[derive(Debug)]
pub enum DetectorError {
    Sampler(FrameError),
    Detection(SubtitleDetectionError),
}

pub struct Detector {
    validator: FrameValidator,
}

impl Detector {
    pub fn new(settings: &DetectionSettings) -> Result<Self, SubtitleDetectionError> {
        let mut detection_options = SubtitleDetectionOptions::default();
        detection_options.luma_band.target = settings.target;
        detection_options.luma_band.delta = settings.delta;
        detection_options.roi = settings.roi;

        let config = FrameValidatorConfig {
            detection: detection_options,
        };
        let validator = FrameValidator::new(config)?;
        Ok(Self { validator })
    }

    pub fn attach(self, input: StreamBundle<SamplerResult>) -> StreamBundle<DetectionSampleResult> {
        let StreamBundle {
            stream,
            total_frames,
        } = input;

        let (tx, rx) = mpsc::channel::<DetectionSampleResult>(DETECTOR_CHANNEL_CAPACITY);
        let validator = self.validator;

        tokio::spawn(async move {
            let worker = DetectorWorker::new(validator);
            let mut upstream = stream;

            while let Some(sample_result) = upstream.next().await {
                match sample_result {
                    Ok(sample) => {
                        let result = worker.handle_sample(sample).await;
                        let is_err = result.is_err();
                        if tx.send(result).await.is_err() {
                            break;
                        }
                        if is_err {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(DetectorError::Sampler(err))).await;
                        break;
                    }
                }
            }

            worker.finalize().await;
        });

        let stream = Box::pin(unfold(rx, |mut receiver| async {
            receiver.recv().await.map(|item| (item, receiver))
        }));

        StreamBundle::new(stream, total_frames)
    }
}

struct DetectorWorker {
    validator: FrameValidator,
}

impl DetectorWorker {
    fn new(validator: FrameValidator) -> Self {
        Self { validator }
    }

    async fn handle_sample(&self, sample: SampledFrame) -> Result<DetectionSample, DetectorError> {
        let frame = sample.frame().clone();
        let started = Instant::now();
        let detection = self
            .validator
            .process_frame(frame)
            .await
            .map_err(DetectorError::Detection)?;
        let elapsed = started.elapsed();

        Ok(DetectionSample {
            sample,
            detection,
            elapsed,
        })
    }

    async fn finalize(&self) {
        self.validator.finalize().await;
    }
}

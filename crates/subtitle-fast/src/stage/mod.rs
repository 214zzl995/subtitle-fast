pub mod detector;
pub mod progress;
pub mod sampler;
pub mod sorter;

use std::pin::Pin;

use futures_util::Stream;
use tokio_stream::StreamExt;

use crate::settings::{DetectionSettings, EffectiveSettings};
use detector::{Detector, DetectorError};
use progress::Progress;
use sampler::FrameSampler;
use sorter::FrameSorter;
use subtitle_fast_decoder::{DynYPlaneProvider, YPlaneError};
use subtitle_fast_validator::subtitle_detection::SubtitleDetectionError;

pub struct StreamBundle<T> {
    pub stream: Pin<Box<dyn Stream<Item = T> + Send>>,
    pub total_frames: Option<u64>,
}

impl<T> StreamBundle<T> {
    pub fn new(stream: Pin<Box<dyn Stream<Item = T> + Send>>, total_frames: Option<u64>) -> Self {
        Self {
            stream,
            total_frames,
        }
    }
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct PipelineConfig {
    pub detection: DetectionSettings,
}

impl PipelineConfig {
    pub fn from_settings(settings: &EffectiveSettings) -> Self {
        Self {
            detection: settings.detection.clone(),
        }
    }
}

pub async fn run_pipeline(
    provider: DynYPlaneProvider,
    pipeline: &PipelineConfig,
) -> Result<(), (YPlaneError, u64)> {
    let initial_total_frames = provider.total_frames();
    let initial_stream = provider.into_stream();

    let sorted = FrameSorter::new().attach(StreamBundle::new(initial_stream, initial_total_frames));

    let sampled = FrameSampler::new(pipeline.detection.samples_per_second).attach(sorted);

    let detector_stage =
        Detector::new(&pipeline.detection).map_err(|err| (detection_error_to_yplane(err), 0))?;

    let detected = detector_stage.attach(sampled);

    let monitored = Progress::new("pipeline").attach(detected);

    let StreamBundle { stream, .. } = monitored;
    let mut detection_stream = stream;
    let mut processed_candidates: u64 = 0;

    while let Some(event) = detection_stream.next().await {
        match event {
            Ok(_candidate) => {
                processed_candidates = processed_candidates.saturating_add(1);
            }
            Err(DetectorError::Sampler(err)) => {
                return Err((err, processed_candidates));
            }
            Err(DetectorError::Detection(err)) => {
                let yplane_err = detection_error_to_yplane(err);
                return Err((yplane_err, processed_candidates));
            }
        }
    }

    Ok(())
}

fn detection_error_to_yplane(err: SubtitleDetectionError) -> YPlaneError {
    YPlaneError::configuration(format!("subtitle detection error: {err}"))
}

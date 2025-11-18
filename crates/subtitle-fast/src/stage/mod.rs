pub mod detector;
pub mod progress;
pub mod sampler;
pub mod segmenter;
pub mod sorter;

use std::pin::Pin;

use futures_util::Stream;
use tokio_stream::StreamExt;

use crate::settings::{DetectionSettings, EffectiveSettings};
use detector::Detector;
use progress::Progress;
use sampler::FrameSampler;
use segmenter::{SegmenterError, SubtitleSegmenter};
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
    let segmented = SubtitleSegmenter::new(&pipeline.detection).attach(detected);
    let monitored = Progress::new("pipeline").attach(segmented);

    let StreamBundle { stream, .. } = monitored;
    let mut segment_stream = stream;
    let mut processed_samples: u64 = 0;

    while let Some(event) = segment_stream.next().await {
        match event {
            Ok(segment) => {
                if segment.sample.is_some() {
                    processed_samples = processed_samples.saturating_add(1);
                }
            }
            Err(SegmenterError::Detector(err)) => match err {
                detector::DetectorError::Sampler(sampler_err) => {
                    return Err((sampler_err, processed_samples));
                }
                detector::DetectorError::Detection(det_err) => {
                    let yplane_err = detection_error_to_yplane(det_err);
                    return Err((yplane_err, processed_samples));
                }
            },
        }
    }

    Ok(())
}

fn detection_error_to_yplane(err: SubtitleDetectionError) -> YPlaneError {
    YPlaneError::configuration(format!("subtitle detection error: {err}"))
}

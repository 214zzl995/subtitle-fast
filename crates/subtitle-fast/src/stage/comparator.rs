use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream::unfold};
use tokio::sync::mpsc;

use super::StreamBundle;
use super::detector::{DetectionSample, DetectionSampleResult, DetectorError};
use super::sampler::SampledFrame;
use subtitle_fast_comparator::{
    ComparatorFactory, ComparatorKind, ComparatorSettings, FeatureBlob, SubtitleComparator,
};
use subtitle_fast_validator::subtitle_detection::{
    DetectionRegion, RoiConfig, SubtitleDetectionResult,
};

const COMPARATOR_CHANNEL_CAPACITY: usize = 2;

pub type FeatureSampleResult = Result<FeatureSample, ComparatorStageError>;

#[allow(dead_code)]
pub struct FeatureSample {
    pub sample: SampledFrame,
    pub detection: SubtitleDetectionResult,
    pub detection_elapsed: Duration,
    pub comparator_elapsed: Duration,
    pub regions: Vec<RegionFeature>,
}

#[allow(dead_code)]
pub struct RegionFeature {
    pub region_index: usize,
    pub region: DetectionRegion,
    pub feature: FeatureBlob,
}

#[derive(Debug)]
pub enum ComparatorStageError {
    Detection(DetectorError),
    Extraction(ExtractionError),
}

#[derive(Debug)]
pub enum ExtractionError {
    MissingFeature { region_index: usize },
}

pub struct ComparatorStage {
    comparator: Arc<dyn SubtitleComparator>,
}

impl ComparatorStage {
    pub fn new(target: u8, delta: u8) -> Self {
        let settings = ComparatorSettings {
            kind: ComparatorKind::SpectralHash,
            target,
            delta,
        };
        let factory = ComparatorFactory::new(settings);
        let comparator = factory.build();
        Self { comparator }
    }

    pub fn attach(
        self,
        input: StreamBundle<DetectionSampleResult>,
    ) -> StreamBundle<FeatureSampleResult> {
        let StreamBundle {
            stream,
            total_frames,
        } = input;

        let (tx, rx) = mpsc::channel::<FeatureSampleResult>(COMPARATOR_CHANNEL_CAPACITY);
        let comparator = self.comparator;

        tokio::spawn(async move {
            let mut upstream = stream;
            let worker = ComparatorWorker::new(comparator);

            while let Some(event) = upstream.next().await {
                match event {
                    Ok(sample) => match worker.handle_sample(sample).await {
                        Ok(result) => {
                            if tx.send(Ok(result)).await.is_err() {
                                break;
                            }
                        }
                        Err(err) => {
                            let _ = tx.send(Err(ComparatorStageError::Extraction(err))).await;
                            break;
                        }
                    },
                    Err(err) => {
                        let _ = tx.send(Err(ComparatorStageError::Detection(err))).await;
                        break;
                    }
                }
            }
        });

        let stream = Box::pin(unfold(rx, |mut receiver| async {
            match receiver.recv().await {
                Some(item) => Some((item, receiver)),
                None => None,
            }
        }));

        StreamBundle::new(stream, total_frames)
    }
}

struct ComparatorWorker {
    comparator: Arc<dyn SubtitleComparator>,
}

impl ComparatorWorker {
    fn new(comparator: Arc<dyn SubtitleComparator>) -> Self {
        Self { comparator }
    }

    async fn handle_sample(
        &self,
        detection_sample: DetectionSample,
    ) -> Result<FeatureSample, ExtractionError> {
        let DetectionSample {
            sample,
            detection,
            elapsed,
        } = detection_sample;
        let frame = sample.frame();
        let detection_elapsed = elapsed;
        let mut regions = Vec::with_capacity(detection.regions.len());

        let started = Instant::now();
        for (region_index, region) in detection.regions.iter().cloned().enumerate() {
            let roi = region_to_roi(frame.width(), frame.height(), &region);
            if roi.width <= 0.0 || roi.height <= 0.0 {
                return Err(ExtractionError::MissingFeature { region_index });
            }
            if let Some(feature) = self.comparator.extract(frame, &roi) {
                regions.push(RegionFeature {
                    region_index,
                    region,
                    feature,
                });
            } else {
                return Err(ExtractionError::MissingFeature { region_index });
            }
        }
        let comparator_elapsed = started.elapsed();

        Ok(FeatureSample {
            sample,
            detection,
            detection_elapsed,
            comparator_elapsed,
            regions,
        })
    }
}

fn region_to_roi(frame_width: u32, frame_height: u32, region: &DetectionRegion) -> RoiConfig {
    let fw = frame_width.max(1) as f32;
    let fh = frame_height.max(1) as f32;
    let x0 = (region.x / fw).clamp(0.0, 1.0);
    let y0 = (region.y / fh).clamp(0.0, 1.0);
    let x1 = ((region.x + region.width) / fw).clamp(0.0, 1.0);
    let y1 = ((region.y + region.height) / fh).clamp(0.0, 1.0);
    RoiConfig {
        x: x0,
        y: y0,
        width: (x1 - x0).max(0.0),
        height: (y1 - y0).max(0.0),
    }
}

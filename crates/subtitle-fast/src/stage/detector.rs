use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream::unfold};
use tokio::sync::mpsc;

use super::StreamBundle;
use super::sampler::{SampledFrame, SamplerResult};
use crate::settings::DetectionSettings;
use subtitle_fast_types::{SubtitleDetectionResult, YPlaneError};
use subtitle_fast_validator::subtitle_detection::SubtitleDetectionError;
use subtitle_fast_validator::{FrameValidator, FrameValidatorConfig, SubtitleDetectionOptions};

const DETECTOR_CHANNEL_CAPACITY: usize = 2;
const DETECTOR_WORKER_CHANNEL_CAPACITY: usize = 2;
const DETECTOR_MAX_WORKERS: usize = 1;

pub type DetectionSampleResult = Result<DetectionSample, DetectorError>;

#[allow(dead_code)]
pub struct DetectionSample {
    pub sample: SampledFrame,
    pub detection: SubtitleDetectionResult,
    pub elapsed: Duration,
}

#[derive(Debug)]
pub enum DetectorError {
    Sampler(YPlaneError),
    Detection(SubtitleDetectionError),
}

struct DetectorJob {
    seq: u64,
    sample: SampledFrame,
}

struct OrderedDetectorResult {
    seq: u64,
    result: DetectionSampleResult,
}

fn detector_worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|count| count.get().min(DETECTOR_MAX_WORKERS))
        .unwrap_or(1)
        .max(1)
}

pub struct Detector {
    validators: Vec<FrameValidator>,
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
        let worker_count = detector_worker_count();
        let validators = (0..worker_count)
            .map(|_| FrameValidator::new(config.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { validators })
    }

    pub fn attach(self, input: StreamBundle<SamplerResult>) -> StreamBundle<DetectionSampleResult> {
        let StreamBundle {
            stream,
            total_frames,
        } = input;

        let (tx, rx) = mpsc::channel::<DetectionSampleResult>(DETECTOR_CHANNEL_CAPACITY);
        let validators = self.validators;

        tokio::spawn(async move {
            let worker_count = validators.len().max(1);
            let (result_tx, result_rx) = mpsc::channel::<OrderedDetectorResult>(
                worker_count * DETECTOR_CHANNEL_CAPACITY.max(1),
            );
            let mut worker_inputs = Vec::with_capacity(worker_count);

            for validator in validators {
                let (worker_tx, mut worker_rx) =
                    mpsc::channel::<DetectorJob>(DETECTOR_WORKER_CHANNEL_CAPACITY);
                worker_inputs.push(worker_tx);
                let worker = DetectorWorker::new(validator);
                let result_tx = result_tx.clone();
                tokio::spawn(async move {
                    while let Some(job) = worker_rx.recv().await {
                        let result = worker.handle_sample(job.sample).await;
                        let _ = result_tx
                            .send(OrderedDetectorResult {
                                seq: job.seq,
                                result,
                            })
                            .await;
                    }
                    worker.finalize().await;
                });
            }

            let forward = tokio::spawn(async move {
                forward_detector_results(result_rx, tx).await;
            });

            let mut upstream = stream;
            let mut seq: u64 = 0;
            let mut next_worker: usize = 0;
            let result_tx_main = result_tx;

            while let Some(sample_result) = upstream.next().await {
                match sample_result {
                    Ok(sample) => {
                        if worker_inputs.is_empty() {
                            break;
                        }
                        let job = DetectorJob { seq, sample };
                        let sender = &worker_inputs[next_worker];
                        next_worker = (next_worker + 1) % worker_inputs.len();
                        if sender.send(job).await.is_err() {
                            break;
                        }
                        seq = seq.saturating_add(1);
                    }
                    Err(err) => {
                        let ordered = OrderedDetectorResult {
                            seq,
                            result: Err(DetectorError::Sampler(err)),
                        };
                        let _ = result_tx_main.send(ordered).await;
                        break;
                    }
                }
            }

            drop(worker_inputs);
            drop(result_tx_main);

            let _ = forward.await;
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

async fn forward_detector_results(
    mut results: mpsc::Receiver<OrderedDetectorResult>,
    tx: mpsc::Sender<DetectionSampleResult>,
) {
    let mut next_seq: u64 = 0;
    let mut buffer: BTreeMap<u64, DetectionSampleResult> = BTreeMap::new();

    while let Some(OrderedDetectorResult { seq, result }) = results.recv().await {
        buffer.insert(seq, result);
        while let Some(item) = buffer.remove(&next_seq) {
            if tx.send(item).await.is_err() {
                return;
            }
            next_seq = next_seq.saturating_add(1);
        }
    }

    while let Some(item) = buffer.remove(&next_seq) {
        if tx.send(item).await.is_err() {
            return;
        }
        next_seq = next_seq.saturating_add(1);
    }
}

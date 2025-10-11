use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio_stream::StreamExt;

use crate::cli::{DetectionBackend, DumpFormat};
use crate::progress::{ProgressEvent, finalize_success, start_progress};
use crate::settings::EffectiveSettings;
use subtitle_fast_decoder::{DynYPlaneProvider, YPlaneError, YPlaneFrame};
use subtitle_fast_validator::subtitle_detection::{
    SubtitleDetectionError, SubtitleDetectionResult, preflight_detection,
};
use subtitle_fast_validator::{
    FrameMetadata, FrameValidator, FrameValidatorConfig, ImageOutputFormat, SubtitleDetectorKind,
};

const VALIDATOR_CHANNEL_CAPACITY: usize = 64;
const VALIDATOR_MAX_CONCURRENCY: usize = 16;
const DETECTION_OUTPUT_FILENAME: &str = "subtitle_detection_output.jsonl";

#[derive(Clone)]
pub struct PipelineConfig {
    pub dump_dir: Option<PathBuf>,
    pub dump_format: DumpFormat,
    pub detection_samples_per_second: u32,
    pub detection_backend: SubtitleDetectorKind,
    pub onnx_model_path: Option<PathBuf>,
    pub detection_luma_target: Option<u8>,
    pub detection_luma_delta: Option<u8>,
}

impl PipelineConfig {
    pub fn from_settings(settings: &EffectiveSettings, onnx_model_path: Option<PathBuf>) -> Self {
        Self {
            dump_dir: settings.dump_dir.clone(),
            dump_format: settings.dump_format,
            detection_samples_per_second: settings.detection_samples_per_second,
            detection_backend: map_detection_backend(settings.detection_backend),
            onnx_model_path,
            detection_luma_target: settings.detection_luma_target,
            detection_luma_delta: settings.detection_luma_delta,
        }
    }
}

pub async fn run_pipeline(
    provider: DynYPlaneProvider,
    pipeline: &PipelineConfig,
) -> Result<(), (YPlaneError, u64)> {
    let total_frames = provider.total_frames();
    let mut stream = provider.into_stream();

    let mut validator_config = FrameValidatorConfig::from_outputs(
        pipeline.dump_dir.clone(),
        map_dump_format(pipeline.dump_format),
    );
    let detection_enabled = validator_config.detection.enabled;
    let dump_enabled = validator_config.detection.frame_dump.is_some();
    validator_config.detection.detector = pipeline.detection_backend;
    validator_config.detection.onnx_model_path = pipeline.onnx_model_path.clone();
    if let Some(value) = pipeline.detection_luma_target {
        validator_config.detection.luma_band.target_luma = value;
    }
    if let Some(value) = pipeline.detection_luma_delta {
        validator_config.detection.luma_band.delta = value;
    }

    if validator_config.detection.enabled {
        let model_path = validator_config.detection.onnx_model_path.as_deref();
        if let Err(err) = preflight_detection(validator_config.detection.detector, model_path) {
            panic_with_detection_error(err);
        }
    }

    let mut sampler = FrameSampler::new(pipeline.detection_samples_per_second.max(1));
    let mut processed: u64 = 0;
    let sink_started = Instant::now();

    let (sink_bar, sink_progress_tx, sink_progress_task) =
        start_progress("processing", total_frames, sink_started);

    let (result_tx, result_task) = if detection_enabled {
        let (tx, rx) = mpsc::channel::<DetectionRecord>(VALIDATOR_CHANNEL_CAPACITY.max(1));
        let task = tokio::spawn(collect_detection_results(rx));
        (Some(tx), Some(task))
    } else {
        (None, None)
    };

    let validator = match FrameValidator::new(validator_config) {
        Ok(result) => result,
        Err(err) => panic_with_detection_error(err),
    };

    let (job_tx, worker) = spawn_validator_worker(validator, result_tx.clone());

    let mut failure: Option<YPlaneError> = None;

    while let Some(frame) = stream.next().await {
        match frame {
            Ok(frame) => {
                processed = processed.saturating_add(1);
                let timestamp = frame.timestamp();
                let decoder_frame_index = frame.frame_index();
                let frame_index = match decoder_frame_index {
                    Some(raw) if processed > 0 && raw == processed => raw.saturating_sub(1),
                    Some(raw) => raw,
                    None => processed.saturating_sub(1),
                };

                let metadata = FrameMetadata {
                    frame_index,
                    decoder_frame_index,
                    processed_index: processed,
                    timestamp,
                };

                let progress_event = ProgressEvent {
                    index: metadata.processed_index,
                    timestamp: metadata.timestamp,
                };
                let _ = sink_progress_tx.send(progress_event).await;

                let should_process =
                    (detection_enabled || dump_enabled) && sampler.should_sample(&frame, &metadata);

                if should_process {
                    if job_tx.send(Job { frame, metadata }).await.is_err() {
                        failure = Some(YPlaneError::configuration("frame validator unavailable"));
                        break;
                    }
                }
            }
            Err(err) => {
                failure = Some(err);
                break;
            }
        }
    }

    drop(job_tx);
    drop(result_tx);
    if let Err(err) = worker.await {
        if !err.is_cancelled() && failure.is_none() {
            failure = Some(YPlaneError::configuration(format!(
                "frame validator worker error: {err}"
            )));
        }
    }

    drop(sink_progress_tx);
    let sink_summary = sink_progress_task.await.expect("progress task panicked");

    let mut detection_records = match result_task {
        Some(task) => match task.await {
            Ok(records) => records,
            Err(err) => {
                eprintln!("detection result task failed: {err}");
                Vec::new()
            }
        },
        None => Vec::new(),
    };

    if detection_enabled || dump_enabled {
        if let Err(err) =
            write_detection_results(&mut detection_records, pipeline.dump_dir.as_deref())
        {
            eprintln!("failed to write detection results: {err}");
        }
    }

    if let Some(err) = failure {
        sink_bar.abandon_with_message(format!(
            "failed after decoding {processed} frames; processed {} frames",
            sink_summary.processed
        ));
        return Err((err, processed));
    }

    finalize_success(&sink_bar, &sink_summary, total_frames);

    Ok(())
}

struct DetectionRecord {
    frame_index: u64,
    result: SubtitleDetectionResult,
}

struct Job {
    frame: YPlaneFrame,
    metadata: FrameMetadata,
}

fn spawn_validator_worker(
    validator: FrameValidator,
    result_sender: Option<mpsc::Sender<DetectionRecord>>,
) -> (mpsc::Sender<Job>, tokio::task::JoinHandle<()>) {
    let (job_tx, mut job_rx) = mpsc::channel::<Job>(VALIDATOR_CHANNEL_CAPACITY.max(1));
    let semaphore = Arc::new(Semaphore::new(VALIDATOR_MAX_CONCURRENCY.max(1)));
    let worker = tokio::spawn(async move {
        let mut tasks = JoinSet::new();
        while let Some(job) = job_rx.recv().await {
            let permit = match semaphore.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(err) => {
                    eprintln!("frame validation semaphore error: {err}");
                    break;
                }
            };

            let validator = validator.clone();
            let result_sender = result_sender.clone();
            tasks.spawn(async move {
                let Job { frame, metadata } = job;
                let _permit = permit;
                let metadata_for_validator = metadata.clone();
                let detection = validator.process_frame(frame, metadata_for_validator).await;
                if let (Some(sender), Some(result)) = (result_sender, detection) {
                    let record = DetectionRecord {
                        frame_index: metadata.frame_index,
                        result,
                    };
                    let _ = sender.send(record).await;
                }
            });
        }

        while let Some(result) = tasks.join_next().await {
            if let Err(err) = result {
                if !err.is_cancelled() {
                    eprintln!("frame validation join error: {err}");
                }
            }
        }

        validator.finalize().await;
    });

    (job_tx, worker)
}

async fn collect_detection_results(
    mut receiver: mpsc::Receiver<DetectionRecord>,
) -> Vec<DetectionRecord> {
    let mut records = Vec::new();
    while let Some(record) = receiver.recv().await {
        records.push(record);
    }
    records
}

fn write_detection_results(
    records: &mut Vec<DetectionRecord>,
    dump_dir: Option<&Path>,
) -> std::io::Result<()> {
    records.sort_by_key(|record| record.frame_index);
    let output_path = dump_dir
        .map(|dir| dir.join(DETECTION_OUTPUT_FILENAME))
        .unwrap_or_else(|| PathBuf::from(DETECTION_OUTPUT_FILENAME));
    let mut file = File::create(&output_path)?;
    for record in records.iter() {
        let line = serde_json::to_string(&serde_json::json!({
            "frame_index": record.frame_index,
            "has_subtitle": record.result.has_subtitle,
            "max_score": record.result.max_score,
            "regions": record.result.regions,
        }))?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
    }
    Ok(())
}

struct FrameSampler {
    samples_per_second: u32,
    current: Option<SamplerSecond>,
}

impl FrameSampler {
    fn new(samples_per_second: u32) -> Self {
        Self {
            samples_per_second: samples_per_second.max(1),
            current: None,
        }
    }

    fn should_sample(&mut self, frame: &YPlaneFrame, metadata: &FrameMetadata) -> bool {
        let (second_index, elapsed) = self.resolve_second(frame, metadata);

        if self.current.as_ref().map(|second| second.index) != Some(second_index) {
            self.current = Some(SamplerSecond::new(second_index, self.samples_per_second));
        }

        let Some(current) = self.current.as_mut() else {
            return false;
        };

        current.consume(elapsed)
    }

    fn resolve_second(&self, frame: &YPlaneFrame, metadata: &FrameMetadata) -> (u64, f64) {
        use std::time::Duration;

        if let Some(timestamp) = metadata.timestamp.or_else(|| frame.timestamp()) {
            let second_index = timestamp.as_secs();
            let elapsed = timestamp
                .checked_sub(Duration::from_secs(second_index))
                .unwrap_or_else(|| Duration::from_secs(0))
                .as_secs_f64();
            return (second_index, elapsed);
        }

        let samples = self.samples_per_second.max(1) as u64;
        let processed = metadata.processed_index.saturating_sub(1);
        let second_index = processed / samples;
        let offset = processed.saturating_sub(second_index * samples);
        let elapsed = offset as f64 / self.samples_per_second.max(1) as f64;
        (second_index, elapsed)
    }
}

struct SamplerSecond {
    index: u64,
    targets: Vec<f64>,
    next_target_idx: usize,
}

impl SamplerSecond {
    fn new(index: u64, samples_per_second: u32) -> Self {
        let slots = samples_per_second.max(1) as usize;
        let mut targets = Vec::with_capacity(slots);
        for i in 0..slots {
            if i == 0 {
                targets.push(0.0);
            } else {
                targets.push(i as f64 / samples_per_second as f64);
            }
        }
        Self {
            index,
            targets,
            next_target_idx: 0,
        }
    }

    fn consume(&mut self, elapsed: f64) -> bool {
        if self.targets.is_empty() {
            return false;
        }

        let mut should_write = false;
        let epsilon = 1e-6f64;

        while self.next_target_idx < self.targets.len()
            && elapsed + epsilon >= self.targets[self.next_target_idx]
        {
            should_write = true;
            self.next_target_idx += 1;
        }

        should_write
    }
}

fn map_detection_backend(backend: DetectionBackend) -> SubtitleDetectorKind {
    match backend {
        DetectionBackend::Auto => SubtitleDetectorKind::Auto,
        DetectionBackend::Onnx => SubtitleDetectorKind::OnnxPpocr,
        DetectionBackend::Vision => SubtitleDetectorKind::MacVision,
        DetectionBackend::Luma => SubtitleDetectorKind::LumaBand,
    }
}

fn map_dump_format(format: DumpFormat) -> ImageOutputFormat {
    match format {
        DumpFormat::Jpeg => ImageOutputFormat::Jpeg { quality: 90 },
        DumpFormat::Png => ImageOutputFormat::Png,
        DumpFormat::Webp => ImageOutputFormat::Webp,
        DumpFormat::Yuv => ImageOutputFormat::Yuv,
    }
}

fn panic_with_detection_error(err: SubtitleDetectionError) -> ! {
    match err {
        SubtitleDetectionError::Environment(_)
        | SubtitleDetectionError::Session(_)
        | SubtitleDetectionError::ModelNotFound { .. }
        | SubtitleDetectionError::Input(_)
        | SubtitleDetectionError::Inference(_)
        | SubtitleDetectionError::InvalidOutputShape
        | SubtitleDetectionError::RuntimeSchemaConflict { .. } => panic!(
            "failed to initialize the ONNX subtitle detector: {err}\n\
             Install the ONNX Runtime 1.16.x shared libraries and ensure the dynamic library \
             directory is visible to the application (for example via ORT_DYLIB_PATH or your \
             system's library path). Documentation: https://onnxruntime.ai/docs/install/"
        ),
        _ => panic!("failed to initialize the subtitle detector: {err}"),
    }
}

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio_stream::StreamExt;

use crate::cli::{DetectionBackend, DumpFormat};
use crate::progress::{ProgressEvent, finalize_success, start_progress};
use crate::settings::EffectiveSettings;
use subtitle_fast_decoder::{DynYPlaneProvider, YPlaneError};
use subtitle_fast_validator::subtitle_detection::{SubtitleDetectionError, preflight_detection};
use subtitle_fast_validator::{
    FrameMetadata, FrameValidator, FrameValidatorConfig, ImageOutputFormat, SubtitleDetectorKind,
};

const VALIDATOR_CHANNEL_CAPACITY: usize = 64;
const VALIDATOR_MAX_CONCURRENCY: usize = 16;

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
        pipeline.detection_samples_per_second,
    );
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

    let mut processed: u64 = 0;
    let sink_started = Instant::now();

    let (sink_bar, sink_progress_tx, sink_progress_task) =
        start_progress("processing", total_frames, sink_started);

    let validator = match FrameValidator::new(validator_config) {
        Ok(result) => result,
        Err(err) => panic_with_detection_error(err),
    };

    let (job_tx, worker) = spawn_validator_worker(validator, sink_progress_tx.clone());

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
                if job_tx.send(Job { frame, metadata }).await.is_err() {
                    failure = Some(YPlaneError::configuration("frame validator unavailable"));
                    break;
                }
            }
            Err(err) => {
                failure = Some(err);
                break;
            }
        }
    }

    drop(job_tx);
    if let Err(err) = worker.await {
        if !err.is_cancelled() && failure.is_none() {
            failure = Some(YPlaneError::configuration(format!(
                "frame validator worker error: {err}"
            )));
        }
    }

    drop(sink_progress_tx);
    let sink_summary = sink_progress_task.await.expect("progress task panicked");

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

struct Job {
    frame: subtitle_fast_decoder::YPlaneFrame,
    metadata: FrameMetadata,
}

fn spawn_validator_worker(
    validator: FrameValidator,
    progress_sender: mpsc::Sender<ProgressEvent>,
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
            let progress_sender = progress_sender.clone();
            tasks.spawn(async move {
                let Job { frame, metadata } = job;
                let _permit = permit;
                validator.process_frame(frame, metadata.clone()).await;
                let progress_event = ProgressEvent {
                    index: metadata.processed_index,
                    timestamp: metadata.timestamp,
                };
                let _ = progress_sender.send(progress_event).await;
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

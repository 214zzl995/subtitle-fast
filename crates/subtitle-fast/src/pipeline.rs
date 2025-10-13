use std::path::PathBuf;
use std::sync::Arc;

use tokio_stream::StreamExt;

use crate::cli::{DetectionBackend, DumpFormat};
use crate::progress::{ProgressEvent, finalize_success, start_progress};
use crate::sampler::{FrameSampler, SampledFrame};
use crate::settings::EffectiveSettings;
use crate::sorter::FrameSorter;
use crate::stage::{PipelineStage, StageInput, StageOutput};
use std::time::Instant;
use subtitle_fast_decoder::{DynYPlaneProvider, YPlaneError};
use subtitle_fast_validator::SubtitleDetectorKind;

#[allow(dead_code)]
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
    _pipeline: &PipelineConfig,
) -> Result<(), (YPlaneError, u64)> {
    let initial_total_frames = provider.total_frames();
    let initial_stream = provider.into_stream();

    let StageOutput {
        stream: sorted_stream,
        total_frames: sorted_total_frames,
    } = Box::new(FrameSorter::new()).apply(StageInput {
        stream: initial_stream,
        total_frames: initial_total_frames,
    });

    let (progress_bar, progress_tx, progress_task) =
        start_progress("sampling", sorted_total_frames, Instant::now());

    let mut sampler = FrameSampler::new(_pipeline.detection_samples_per_second);
    let hook_tx = progress_tx.clone();
    sampler.set_progress_callback(Some(Arc::new(move |processed| {
        let tx = hook_tx.clone();
        tokio::spawn(async move {
            let _ = tx
                .send(ProgressEvent {
                    index: processed,
                    timestamp: None,
                })
                .await;
        });
    })));

    let StageOutput {
        stream: sampled_stream,
        total_frames: sampled_total_frames,
    } = Box::new(sampler).apply(StageInput {
        stream: sorted_stream,
        total_frames: sorted_total_frames,
    });

    let mut stream = sampled_stream;
    let mut processed: u64 = 0;
    let mut failure: Option<(YPlaneError, u64)> = None;

    while let Some(frame) = stream.next().await {
        match frame {
            Ok(sampled) => {
                processed = processed.saturating_add(1);
                handle_sampled_frame(sampled).await;
            }
            Err(err) => {
                failure = Some((err, processed));
                break;
            }
        }
    }

    drop(progress_tx);
    let progress_summary = progress_task.await.expect("progress task panicked");

    if let Some((err, processed_count)) = failure {
        progress_bar
            .abandon_with_message(format!("failed after processing {processed_count} frames"));
        return Err((err, processed_count));
    }

    finalize_success(&progress_bar, &progress_summary, sampled_total_frames);

    Ok(())
}

async fn handle_sampled_frame(frame: SampledFrame) {
    // Placeholder to keep the compiler happy until the detector stage is wired in.
    // Future implementations will route sampled vs skipped frames downstream.
    frame.finish().await;
}

fn map_detection_backend(backend: DetectionBackend) -> SubtitleDetectorKind {
    match backend {
        DetectionBackend::Auto => SubtitleDetectorKind::Auto,
        DetectionBackend::Onnx => SubtitleDetectorKind::OnnxPpocr,
        DetectionBackend::Vision => SubtitleDetectorKind::MacVision,
        DetectionBackend::Luma => SubtitleDetectorKind::LumaBand,
    }
}

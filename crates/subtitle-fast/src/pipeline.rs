use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio_stream::StreamExt;

use crate::cli::DetectionBackend;
use crate::output::OutputManager;
use crate::settings::{DebugOutputSettings, DetectionSettings, EffectiveSettings};
use crate::stage::detection::{
    DefaultSubtitleBandStrategy, SubtitleDetectionStage, SubtitleSegment, SubtitleStageError,
};
use crate::stage::sampler::FrameSampler;
use crate::stage::sorter::FrameSorter;
use crate::stage::{PipelineStage, StageInput, StageOutput};
use subtitle_fast_decoder::{DynYPlaneProvider, YPlaneError};
use subtitle_fast_validator::subtitle_detection::SubtitleDetectionError;
use subtitle_fast_validator::{FrameValidator, FrameValidatorConfig, SubtitleDetectorKind};

#[allow(dead_code)]
#[derive(Clone)]
pub struct PipelineConfig {
    pub output: Option<PathBuf>,
    pub debug: DebugOutputSettings,
    pub detection: DetectionSettings,
    pub onnx_model_path: Option<PathBuf>,
}

impl PipelineConfig {
    pub fn from_settings(settings: &EffectiveSettings, onnx_model_path: Option<PathBuf>) -> Self {
        Self {
            output: settings.output.clone(),
            debug: settings.debug.clone(),
            detection: settings.detection.clone(),
            onnx_model_path,
        }
    }
}

pub async fn run_pipeline(
    provider: DynYPlaneProvider,
    _pipeline: &PipelineConfig,
) -> Result<(), (YPlaneError, u64)> {
    let initial_total_frames = provider.total_frames();
    let initial_stream = provider.into_stream();

    let output_manager =
        OutputManager::new(_pipeline.debug.image.clone(), _pipeline.debug.json.clone());

    let validator = match build_validator(_pipeline) {
        Ok(validator) => validator,
        Err(err) => return Err((map_detection_error(err), 0)),
    };

    let StageOutput {
        stream: sorted_stream,
        total_frames: sorted_total_frames,
    } = Box::new(FrameSorter::new()).apply(StageInput {
        stream: initial_stream,
        total_frames: initial_total_frames,
    });

    let StageOutput {
        stream: sampled_stream,
        total_frames: _sampled_total_frames,
    } = Box::new(FrameSampler::new(_pipeline.detection.samples_per_second)).apply(StageInput {
        stream: sorted_stream,
        total_frames: sorted_total_frames,
    });

    let StageOutput {
        stream: mut detection_stream,
        total_frames: _detection_total_frames,
    } = Box::new(SubtitleDetectionStage::new(
        validator,
        _pipeline.detection.samples_per_second,
        Arc::new(DefaultSubtitleBandStrategy::default()),
        output_manager.clone(),
    ))
    .apply(StageInput {
        stream: sampled_stream,
        total_frames: _sampled_total_frames,
    });

    let mut failure: Option<(YPlaneError, u64)> = None;

    while let Some(event) = detection_stream.next().await {
        match event {
            Ok(segment) => {
                if let Some(manager) = output_manager.as_ref() {
                    if let Err(err) = manager.record_segment(&segment).await {
                        eprintln!("segment output error: {err}");
                    }
                }
                handle_segment(segment);
            }
            Err(SubtitleStageError::Decoder { error, processed }) => {
                failure = Some((error, processed));
                break;
            }
            Err(SubtitleStageError::Detection(err)) => {
                failure = Some((map_detection_error(err), 0));
                break;
            }
        }
    }

    if let Some((err, processed_count)) = failure {
        return Err((err, processed_count));
    }

    if let Some(manager) = output_manager {
        if let Err(err) = manager.finalize().await {
            eprintln!("output finalize error: {err}");
        }
    }

    Ok(())
}

fn map_detection_backend(backend: DetectionBackend) -> SubtitleDetectorKind {
    match backend {
        DetectionBackend::Auto => SubtitleDetectorKind::Auto,
        DetectionBackend::Onnx => SubtitleDetectorKind::OnnxPpocr,
        DetectionBackend::Vision => SubtitleDetectorKind::MacVision,
        DetectionBackend::Luma => SubtitleDetectorKind::LumaBand,
    }
}

fn build_validator(pipeline: &PipelineConfig) -> Result<FrameValidator, SubtitleDetectionError> {
    let mut config = FrameValidatorConfig::default();
    let detection = &mut config.detection;
    detection.detector = map_detection_backend(pipeline.detection.backend);
    detection.onnx_model_path = pipeline.onnx_model_path.clone();
    if let Some(target) = pipeline.detection.luma_target {
        detection.luma_band.target_luma = target;
    }
    if let Some(delta) = pipeline.detection.luma_delta {
        detection.luma_band.delta = delta;
    }
    FrameValidator::new(config)
}

fn handle_segment(segment: SubtitleSegment) {
    let SubtitleSegment {
        frame,
        max_score,
        region,
        start,
        end,
    } = segment;
    let start = format_timestamp(start);
    let end = format_timestamp(end);
    let frame_index = frame
        .frame_index()
        .map(|idx| idx.to_string())
        .unwrap_or_else(|| "n/a".into());
    let frame_ts = format_timestamp(frame.timestamp());
    println!(
        "subtitle segment {} -> {} (score {:.2}) frame {} @ {} region x:{:.1} y:{:.1} w:{:.1} h:{:.1}",
        start,
        end,
        max_score,
        frame_index,
        frame_ts,
        region.x,
        region.y,
        region.width,
        region.height
    );
}

fn format_timestamp(ts: Option<Duration>) -> String {
    match ts {
        Some(value) => format!("{:.3}s", value.as_secs_f64()),
        None => "n/a".into(),
    }
}

fn map_detection_error(err: SubtitleDetectionError) -> YPlaneError {
    YPlaneError::configuration(err.to_string())
}

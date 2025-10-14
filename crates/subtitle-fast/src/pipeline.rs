use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio_stream::StreamExt;

use crate::cli::{DetectionBackend, DumpFormat};
use crate::settings::EffectiveSettings;
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
    } = Box::new(FrameSampler::new(_pipeline.detection_samples_per_second)).apply(StageInput {
        stream: sorted_stream,
        total_frames: sorted_total_frames,
    });

    let StageOutput {
        stream: mut detection_stream,
        total_frames: _detection_total_frames,
    } = Box::new(SubtitleDetectionStage::new(
        validator,
        _pipeline.detection_samples_per_second,
        Arc::new(DefaultSubtitleBandStrategy::default()),
    ))
    .apply(StageInput {
        stream: sampled_stream,
        total_frames: _sampled_total_frames,
    });

    let mut failure: Option<(YPlaneError, u64)> = None;

    while let Some(event) = detection_stream.next().await {
        match event {
            Ok(segment) => {
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
    detection.detector = pipeline.detection_backend;
    detection.onnx_model_path = pipeline.onnx_model_path.clone();
    if let Some(target) = pipeline.detection_luma_target {
        detection.luma_band.target_luma = target;
    }
    if let Some(delta) = pipeline.detection_luma_delta {
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

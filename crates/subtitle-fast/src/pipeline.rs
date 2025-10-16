use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio_stream::StreamExt;

use crate::cli::{DetectionBackend, OcrBackend};
use crate::settings::{DebugOutputSettings, DetectionSettings, EffectiveSettings};
use crate::stage::detection::{
    DefaultSubtitleBandStrategy, SubtitleDetectionStage, SubtitleStageError,
};
use crate::stage::sampler::FrameSampler;
use crate::stage::sorter::FrameSorter;
use crate::stage::subtitle_gen::{GeneratedSubtitle, SubtitleGen, SubtitleGenError};
use crate::stage::{PipelineStage, StageInput, StageOutput};
use subtitle_fast_decoder::{DynYPlaneProvider, YPlaneError};
#[cfg(all(feature = "ocr-mlx-vlm", target_os = "macos"))]
use subtitle_fast_ocr::MlxVlmOcrEngine;
use subtitle_fast_ocr::{NoopOcrEngine, OcrEngine, OcrError};
#[cfg(all(target_os = "macos", feature = "ocr-vision"))]
use subtitle_fast_ocr::{VisionOcrConfig, VisionOcrEngine};
use subtitle_fast_validator::subtitle_detection::SubtitleDetectionError;
use subtitle_fast_validator::{FrameValidator, FrameValidatorConfig, SubtitleDetectorKind};

#[allow(dead_code)]
#[derive(Clone)]
pub struct PipelineConfig {
    pub output: PathBuf,
    pub debug: DebugOutputSettings,
    pub detection: DetectionSettings,
    pub ocr: OcrPipelineConfig,
}

#[derive(Clone)]
pub struct OcrPipelineConfig {
    pub backend: OcrBackend,
    pub mlx_vlm_model_path: Option<PathBuf>,
    pub languages: Vec<String>,
    pub auto_detect_language: bool,
}

impl PipelineConfig {
    pub fn from_settings(
        settings: &EffectiveSettings,
        ocr_mlx_model_path: Option<PathBuf>,
    ) -> Self {
        Self {
            output: settings.output.clone(),
            debug: settings.debug.clone(),
            detection: settings.detection.clone(),
            ocr: OcrPipelineConfig {
                backend: settings.ocr.backend,
                mlx_vlm_model_path: ocr_mlx_model_path,
                languages: settings.ocr.languages.clone(),
                auto_detect_language: settings.ocr.auto_detect_language,
            },
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
    } = Box::new(FrameSampler::new(_pipeline.detection.samples_per_second)).apply(StageInput {
        stream: sorted_stream,
        total_frames: sorted_total_frames,
    });

    let StageOutput {
        stream: detection_stream,
        total_frames: detection_total_frames,
    } = Box::new(SubtitleDetectionStage::new(
        validator,
        _pipeline.detection.samples_per_second,
        Arc::new(DefaultSubtitleBandStrategy::default()),
        _pipeline.debug.image.clone(),
        _pipeline.debug.json.clone(),
    ))
    .apply(StageInput {
        stream: sampled_stream,
        total_frames: _sampled_total_frames,
    });

    let ocr_engine = match build_ocr_engine(_pipeline) {
        Ok(engine) => engine,
        Err(err) => return Err((map_ocr_init_error(err), 0)),
    };

    let StageOutput {
        stream: mut gen_stream,
        total_frames: _gen_total_frames,
    } = Box::new(SubtitleGen::new(
        ocr_engine,
        _pipeline.output.clone(),
        _pipeline.debug.json.clone(),
    ))
    .apply(StageInput {
        stream: detection_stream,
        total_frames: detection_total_frames,
    });

    let mut failure: Option<(YPlaneError, u64)> = None;

    while let Some(event) = gen_stream.next().await {
        match event {
            Ok(subtitle) => {
                log_generated_subtitle(&subtitle);
            }
            Err(err) => {
                failure = Some(map_subtitle_gen_error(err));
                break;
            }
        }
    }

    if let Some((err, processed_count)) = failure {
        return Err((err, processed_count));
    }

    if failure.is_none() {
        println!("subtitle output written to {}", _pipeline.output.display());
    }

    Ok(())
}

fn map_detection_backend(backend: DetectionBackend) -> SubtitleDetectorKind {
    match backend {
        DetectionBackend::Auto => SubtitleDetectorKind::Auto,
        DetectionBackend::Vision => SubtitleDetectorKind::MacVision,
        DetectionBackend::Luma => SubtitleDetectorKind::LumaBand,
    }
}

fn build_validator(pipeline: &PipelineConfig) -> Result<FrameValidator, SubtitleDetectionError> {
    let mut config = FrameValidatorConfig::default();
    let detection = &mut config.detection;
    detection.detector = map_detection_backend(pipeline.detection.backend);
    if let Some(target) = pipeline.detection.luma_target {
        detection.luma_band.target_luma = target;
    }
    if let Some(delta) = pipeline.detection.luma_delta {
        detection.luma_band.delta = delta;
    }
    FrameValidator::new(config)
}

fn log_generated_subtitle(subtitle: &GeneratedSubtitle) {
    let start = format_timestamp(Some(subtitle.start));
    let end = format_timestamp(Some(subtitle.end));
    let mut parts = Vec::new();
    if let Some(frame_index) = subtitle.frame_index {
        parts.push(format!("frame {}", frame_index));
    }
    if let Some(confidence) = subtitle.confidence {
        parts.push(format!("conf {:.2}", confidence));
    }
    let meta = if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    };
    println!("subtitle {} -> {}{}: {}", start, end, meta, subtitle.text);
}

fn format_timestamp(ts: Option<Duration>) -> String {
    match ts {
        Some(value) => format!("{:.3}s", value.as_secs_f64()),
        None => "n/a".into(),
    }
}

fn build_ocr_engine(pipeline: &PipelineConfig) -> Result<Arc<dyn OcrEngine>, OcrError> {
    match pipeline.ocr.backend {
        OcrBackend::Vision => build_vision_engine(&pipeline.ocr),
        OcrBackend::MlxVlm => build_mlx_vlm_engine(pipeline.ocr.mlx_vlm_model_path.as_ref()),
        OcrBackend::Noop => build_noop_engine(),
        OcrBackend::Auto => build_auto_ocr_engine(pipeline),
    }
}

fn build_noop_engine() -> Result<Arc<dyn OcrEngine>, OcrError> {
    let engine = NoopOcrEngine::default();
    engine.warm_up()?;
    Ok(Arc::new(engine))
}

#[cfg(all(target_os = "macos", feature = "ocr-vision"))]
fn build_vision_engine(config: &OcrPipelineConfig) -> Result<Arc<dyn OcrEngine>, OcrError> {
    let vision_config = VisionOcrConfig {
        languages: config.languages.clone(),
        auto_detect_language: config.auto_detect_language,
    };
    let engine = VisionOcrEngine::with_config(vision_config)?;
    engine.warm_up()?;
    Ok(Arc::new(engine))
}

#[cfg(not(all(target_os = "macos", feature = "ocr-vision")))]
fn build_vision_engine(_config: &OcrPipelineConfig) -> Result<Arc<dyn OcrEngine>, OcrError> {
    Err(OcrError::backend(
        "vision OCR backend is not available on this platform",
    ))
}

#[cfg(all(feature = "ocr-mlx-vlm", target_os = "macos"))]
fn build_mlx_vlm_engine(path: Option<&PathBuf>) -> Result<Arc<dyn OcrEngine>, OcrError> {
    let model_path = path
        .cloned()
        .ok_or_else(|| OcrError::backend("mlx_vlm model path not configured"))?;
    let engine = MlxVlmOcrEngine::new(model_path)?;
    engine.warm_up()?;
    Ok(Arc::new(engine))
}

#[cfg(any(not(feature = "ocr-mlx-vlm"), not(target_os = "macos")))]
fn build_mlx_vlm_engine(_path: Option<&PathBuf>) -> Result<Arc<dyn OcrEngine>, OcrError> {
    Err(OcrError::backend(
        "mlx_vlm OCR backend is not enabled in this build",
    ))
}

#[cfg(all(target_os = "macos", feature = "ocr-vision"))]
fn build_auto_ocr_engine(pipeline: &PipelineConfig) -> Result<Arc<dyn OcrEngine>, OcrError> {
    match build_vision_engine(&pipeline.ocr) {
        Ok(engine) => Ok(engine),
        Err(err) => {
            if let Some(engine) = maybe_build_mlx(pipeline)? {
                Ok(engine)
            } else {
                Err(err)
            }
        }
    }
}

#[cfg(not(all(target_os = "macos", feature = "ocr-vision")))]
fn build_auto_ocr_engine(pipeline: &PipelineConfig) -> Result<Arc<dyn OcrEngine>, OcrError> {
    if let Some(engine) = maybe_build_mlx(pipeline)? {
        Ok(engine)
    } else {
        build_noop_engine()
    }
}

#[cfg(all(feature = "ocr-mlx-vlm", target_os = "macos"))]
fn maybe_build_mlx(pipeline: &PipelineConfig) -> Result<Option<Arc<dyn OcrEngine>>, OcrError> {
    if pipeline.ocr.mlx_vlm_model_path.is_some() {
        build_mlx_vlm_engine(pipeline.ocr.mlx_vlm_model_path.as_ref()).map(Some)
    } else {
        Ok(None)
    }
}

#[cfg(any(not(feature = "ocr-mlx-vlm"), not(target_os = "macos")))]
fn maybe_build_mlx(_pipeline: &PipelineConfig) -> Result<Option<Arc<dyn OcrEngine>>, OcrError> {
    Ok(None)
}

fn map_ocr_init_error(err: OcrError) -> YPlaneError {
    YPlaneError::configuration(format!("failed to initialize OCR engine: {err}"))
}

fn map_subtitle_gen_error(err: SubtitleGenError) -> (YPlaneError, u64) {
    match err {
        SubtitleGenError::Upstream(SubtitleStageError::Decoder { error, processed }) => {
            (error, processed)
        }
        SubtitleGenError::Upstream(SubtitleStageError::Detection(err)) => {
            (map_detection_error(err), 0)
        }
        SubtitleGenError::Ocr(err) => {
            (YPlaneError::configuration(format!("ocr failure: {err}")), 0)
        }
        SubtitleGenError::Writer(err) => (
            YPlaneError::configuration(format!("subtitle output error: {err}")),
            0,
        ),
        SubtitleGenError::Join(msg) => (
            YPlaneError::configuration(format!("subtitle task error: {msg}")),
            0,
        ),
    }
}

fn map_detection_error(err: SubtitleDetectionError) -> YPlaneError {
    YPlaneError::configuration(err.to_string())
}

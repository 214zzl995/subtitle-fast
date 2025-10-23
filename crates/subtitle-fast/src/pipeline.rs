use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio_stream::StreamExt;

use crate::cli::OcrBackend;
use crate::settings::{DebugOutputSettings, DetectionSettings, EffectiveSettings};
use crate::stage::detection::{
    DefaultSubtitleBandStrategy, FAST_DETECTOR_KIND, PRECISE_DETECTOR_KIND, SubtitleDetectionStage,
    SubtitleStageError,
};
use crate::stage::sampler::FrameSampler;
use crate::stage::sorter::FrameSorter;
use crate::stage::subtitle_gen::{GeneratedSubtitle, SubtitleGen, SubtitleGenError};
use crate::stage::StreamBundle;
use subtitle_fast_decoder::{DynYPlaneProvider, YPlaneError};
use subtitle_fast_ocr::{NoopOcrEngine, OcrEngine, OcrError};
#[cfg(all(target_os = "macos", feature = "ocr-vision"))]
use subtitle_fast_ocr::{VisionOcrConfig, VisionOcrEngine};
use subtitle_fast_validator::subtitle_detection::SubtitleDetectionError;
use subtitle_fast_validator::{FrameValidator, FrameValidatorConfig};

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
    pub languages: Vec<String>,
    pub auto_detect_language: bool,
}

impl PipelineConfig {
    pub fn from_settings(settings: &EffectiveSettings) -> Self {
        Self {
            output: settings.output.clone(),
            debug: settings.debug.clone(),
            detection: settings.detection.clone(),
            ocr: OcrPipelineConfig {
                backend: settings.ocr.backend,
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

    let (fast_validator, precise_validator) = match build_validators(_pipeline) {
        Ok(pair) => pair,
        Err(err) => return Err((map_detection_error(err), 0)),
    };

    let sorted = FrameSorter::new().attach(StreamBundle::new(initial_stream, initial_total_frames));

    let sampled =
        FrameSampler::new(_pipeline.detection.samples_per_second).attach(sorted);

    let detection = SubtitleDetectionStage::new(
        fast_validator,
        precise_validator,
        _pipeline.detection.samples_per_second,
        Arc::new(DefaultSubtitleBandStrategy::default()),
        _pipeline.debug.image.clone(),
        _pipeline.debug.json.clone(),
    )
    .attach(sampled);

    let ocr_engine = match build_ocr_engine(_pipeline) {
        Ok(engine) => engine,
        Err(err) => return Err((map_ocr_init_error(err), 0)),
    };

    let subtitles = SubtitleGen::new(
        ocr_engine,
        _pipeline.output.clone(),
        _pipeline.debug.json.clone(),
    )
    .attach(detection);

    let StreamBundle { stream, .. } = subtitles;
    let mut gen_stream = stream;

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

fn build_validators(
    pipeline: &PipelineConfig,
) -> Result<(FrameValidator, FrameValidator), SubtitleDetectionError> {
    let mut base_config = FrameValidatorConfig::default();
    {
        let detection = &mut base_config.detection;
        detection.luma_band.target_luma = pipeline.detection.luma_target;
        detection.luma_band.delta = pipeline.detection.luma_delta;
    }

    let mut fast_config = base_config.clone();
    fast_config.detection.detector = FAST_DETECTOR_KIND;

    let mut precise_config = base_config;
    precise_config.detection.detector = PRECISE_DETECTOR_KIND;

    let fast = FrameValidator::new(fast_config)?;
    let precise = FrameValidator::new(precise_config)?;
    Ok((fast, precise))
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

#[cfg(all(target_os = "macos", feature = "ocr-vision"))]
fn build_auto_ocr_engine(pipeline: &PipelineConfig) -> Result<Arc<dyn OcrEngine>, OcrError> {
    match build_vision_engine(&pipeline.ocr) {
        Ok(engine) => Ok(engine),
        Err(_) => build_noop_engine(),
    }
}

#[cfg(not(all(target_os = "macos", feature = "ocr-vision")))]
fn build_auto_ocr_engine(pipeline: &PipelineConfig) -> Result<Arc<dyn OcrEngine>, OcrError> {
    let _ = pipeline;
    build_noop_engine()
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

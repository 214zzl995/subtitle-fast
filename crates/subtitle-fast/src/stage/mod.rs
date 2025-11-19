pub mod detector;
pub mod ocr;
pub mod progress;
pub mod sampler;
pub mod segmenter;
pub mod sorter;
pub mod writer;

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use futures_util::Stream;
use tokio_stream::StreamExt;

use crate::settings::{DetectionSettings, EffectiveSettings};
use detector::Detector;
use ocr::{OcrStageError, SubtitleOcr};
use progress::Progress;
use sampler::FrameSampler;
use segmenter::{SegmenterError, SubtitleSegmenter};
use sorter::FrameSorter;
use subtitle_fast_decoder::{DynYPlaneProvider, YPlaneError};
#[cfg(all(feature = "ocr-vision", target_os = "macos"))]
use subtitle_fast_ocr::VisionOcrEngine;
use subtitle_fast_ocr::{NoopOcrEngine, OcrEngine};
use subtitle_fast_validator::subtitle_detection::SubtitleDetectionError;
use writer::{SubtitleWriter, SubtitleWriterError, WriterResult};

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
    pub ocr: OcrPipelineConfig,
    pub output: OutputPipelineConfig,
}

#[derive(Clone)]
pub struct OcrPipelineConfig {
    pub engine: Arc<dyn OcrEngine>,
}

#[derive(Clone)]
pub struct OutputPipelineConfig {
    pub path: PathBuf,
}

impl PipelineConfig {
    pub fn from_settings(settings: &EffectiveSettings, input: &Path) -> Result<Self, YPlaneError> {
        let engine = build_ocr_engine(settings);
        let output_path = settings
            .output
            .path
            .clone()
            .unwrap_or_else(|| default_output_path(input));
        Ok(Self {
            detection: settings.detection.clone(),
            ocr: OcrPipelineConfig { engine },
            output: OutputPipelineConfig { path: output_path },
        })
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
    let ocred = SubtitleOcr::new(Arc::clone(&pipeline.ocr.engine)).attach(segmented);
    let written = SubtitleWriter::new(pipeline.output.path.clone()).attach(ocred);
    let monitored = Progress::new("pipeline").attach(written);

    let StreamBundle { stream, .. }: StreamBundle<WriterResult> = monitored;
    let mut writer_stream = stream;
    let mut processed_samples: u64 = 0;

    while let Some(event) = writer_stream.next().await {
        match event {
            Ok(event) => {
                if event.sample.is_some() {
                    processed_samples = processed_samples.saturating_add(1);
                }
            }
            Err(err) => {
                let yplane_err = writer_error_to_yplane(err);
                return Err((yplane_err, processed_samples));
            }
        }
    }

    Ok(())
}

fn detection_error_to_yplane(err: SubtitleDetectionError) -> YPlaneError {
    YPlaneError::configuration(format!("subtitle detection error: {err}"))
}

fn writer_error_to_yplane(err: SubtitleWriterError) -> YPlaneError {
    match err {
        SubtitleWriterError::Ocr(ocr_err) => match ocr_err {
            OcrStageError::Segmenter(segmenter_err) => match segmenter_err {
                SegmenterError::Detector(detector_err) => match detector_err {
                    detector::DetectorError::Sampler(sampler_err) => sampler_err,
                    detector::DetectorError::Detection(det_err) => {
                        detection_error_to_yplane(det_err)
                    }
                },
            },
            OcrStageError::Engine(ocr_err) => {
                YPlaneError::configuration(format!("ocr error: {ocr_err}"))
            }
        },
        SubtitleWriterError::Io { path, source } => YPlaneError::configuration(format!(
            "failed to write subtitle file {}: {source}",
            path.display()
        )),
    }
}

fn build_ocr_engine(_settings: &EffectiveSettings) -> Arc<dyn OcrEngine> {
    #[cfg(all(feature = "ocr-vision", target_os = "macos"))]
    {
        match VisionOcrEngine::new() {
            Ok(engine) => return Arc::new(engine),
            Err(err) => {
                eprintln!("vision OCR engine failed to initialize: {err}");
            }
        }
    }
    Arc::new(NoopOcrEngine::default())
}

fn default_output_path(input: &Path) -> PathBuf {
    let mut path = input.to_path_buf();
    path.set_extension("srt");
    path
}

pub mod averager;
pub mod detector;
pub mod determiner;
pub mod lifecycle;
pub mod merge;
pub mod ocr;
pub mod sampler;
pub mod sorter;

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use averager::{Averager, AveragerResult};
use detector::Detector;
use futures_util::Stream;
use tokio_stream::wrappers::WatchStream;

use crate::settings::{DetectionSettings, EffectiveSettings};
use determiner::{RegionDeterminer, RegionDeterminerError};
use lifecycle::{RegionLifecycleError, RegionLifecycleTracker};
use merge::{Merge, MergeResult};
use ocr::{OcrStageError, SubtitleOcr};
use sampler::FrameSampler;
use sorter::FrameSorter;
use subtitle_fast_decoder::DynDecoderProvider;
#[cfg(all(feature = "ocr-vision", target_os = "macos"))]
use subtitle_fast_ocr::VisionOcrEngine;
use subtitle_fast_ocr::{NoopOcrEngine, OcrEngine};
use subtitle_fast_types::DecoderError;
use subtitle_fast_validator::subtitle_detection::SubtitleDetectionError;

pub use crate::subtitle::{
    MergedSubtitle, SubtitleLine, TimedSubtitle, render_srt, sort_subtitles,
};
pub use merge::{SubtitleStats, SubtitleUpdate, SubtitleUpdateKind};

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
    pub fn from_settings(settings: &EffectiveSettings, input: &Path) -> Result<Self, DecoderError> {
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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PipelineProgress {
    pub samples_seen: u64,
    pub latest_frame_index: u64,
    pub total_frames: Option<u64>,
    pub fps: f64,
    pub det_ms: f64,
    pub seg_ms: f64,
    pub ocr_ms: f64,
    pub cues: u64,
    pub merged: u64,
    pub ocr_empty: u64,
    pub progress: f64,
    pub completed: bool,
}

#[derive(Clone, Debug)]
pub struct PipelineUpdate {
    pub progress: PipelineProgress,
    pub updates: Vec<SubtitleUpdate>,
}

pub type PipelineResult = AveragerResult;

#[derive(Debug)]
pub enum PipelineError {
    Ocr(OcrStageError),
}

pub struct PipelineOutputs {
    pub stream: Pin<Box<dyn Stream<Item = PipelineResult> + Send>>,
    pub total_frames: Option<u64>,
    pub handle: PipelineHandle,
}

#[derive(Clone)]
pub struct PipelineHandle {
    pause_tx: tokio::sync::watch::Sender<bool>,
}

impl PipelineHandle {
    pub fn pause_sender(&self) -> tokio::sync::watch::Sender<bool> {
        self.pause_tx.clone()
    }

    pub fn set_paused(&self, paused: bool) {
        let _ = self.pause_tx.send(paused);
    }
}

pub fn build_pipeline(
    provider: DynDecoderProvider,
    pipeline: &PipelineConfig,
) -> Result<PipelineOutputs, DecoderError> {
    let initial_total_frames = provider.metadata().total_frames;
    let (_, initial_stream) = provider.open()?;

    let (pause_tx, pause_rx) = tokio::sync::watch::channel(false);

    let paused_stream = StreamBundle::new(
        Box::pin(PauseStream::new(initial_stream, pause_rx.clone())),
        initial_total_frames,
    );

    let sorted = FrameSorter::new().attach(paused_stream);
    let sampled = FrameSampler::new(pipeline.detection.samples_per_second).attach(sorted);

    let detector_stage = Detector::new(&pipeline.detection).map_err(detection_error_to_frame)?;

    let detected = detector_stage.attach(sampled);
    let determined = RegionDeterminer::new().attach(detected);
    let tracked = RegionLifecycleTracker::new(&pipeline.detection).attach(determined);
    let ocred = SubtitleOcr::new(Arc::clone(&pipeline.ocr.engine)).attach(tracked);
    let merged: StreamBundle<MergeResult> = Merge::with_default_window().attach(ocred);
    let averaged: StreamBundle<AveragerResult> = Averager::new().attach(merged);

    Ok(PipelineOutputs {
        stream: averaged.stream,
        total_frames: averaged.total_frames,
        handle: PipelineHandle { pause_tx },
    })
}

struct PauseStream<S> {
    inner: S,
    pause_updates: WatchStream<bool>,
    paused: bool,
}

impl<S> PauseStream<S> {
    fn new(inner: S, pause: tokio::sync::watch::Receiver<bool>) -> Self {
        let paused = *pause.borrow();
        Self {
            inner,
            paused,
            pause_updates: WatchStream::new(pause),
        }
    }
}

impl<S> Stream for PauseStream<S>
where
    S: Stream + Unpin + Send,
{
    type Item = <S as Stream>::Item;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            // Drain any immediately available pause updates.
            while let std::task::Poll::Ready(Some(paused)) =
                Pin::new(&mut this.pause_updates).poll_next(cx)
            {
                this.paused = paused;
            }

            if this.paused {
                // Wait for the next pause update to flip the flag.
                match Pin::new(&mut this.pause_updates).poll_next(cx) {
                    std::task::Poll::Ready(Some(paused)) => {
                        this.paused = paused;
                        continue;
                    }
                    std::task::Poll::Ready(None) => return std::task::Poll::Ready(None),
                    std::task::Poll::Pending => return std::task::Poll::Pending,
                }
            }

            // Not paused; drive the inner stream.
            match Pin::new(&mut this.inner).poll_next(cx) {
                std::task::Poll::Ready(item) => return std::task::Poll::Ready(item),
                std::task::Poll::Pending => {
                    // Allow pause updates to register before parking.
                    if let std::task::Poll::Ready(Some(paused)) =
                        Pin::new(&mut this.pause_updates).poll_next(cx)
                    {
                        this.paused = paused;
                        continue;
                    }
                    return std::task::Poll::Pending;
                }
            }
        }
    }
}

fn detection_error_to_frame(err: SubtitleDetectionError) -> DecoderError {
    DecoderError::configuration(format!("subtitle detection error: {err}"))
}

pub fn pipeline_error_to_frame(err: PipelineError) -> DecoderError {
    match err {
        PipelineError::Ocr(ocr_err) => match ocr_err {
            OcrStageError::Lifecycle(lifecycle_err) => match lifecycle_err {
                RegionLifecycleError::Determiner(det_err) => match det_err {
                    RegionDeterminerError::Detector(detector_err) => match detector_err {
                        detector::DetectorError::Sampler(sampler_err) => sampler_err,
                        detector::DetectorError::Detection(det_err) => {
                            detection_error_to_frame(det_err)
                        }
                    },
                },
            },
            OcrStageError::Engine(ocr_err) => {
                DecoderError::configuration(format!("ocr error: {ocr_err}"))
            }
        },
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
    Arc::new(NoopOcrEngine)
}

fn default_output_path(input: &Path) -> PathBuf {
    let mut path = input.to_path_buf();
    path.set_extension("srt");
    path
}

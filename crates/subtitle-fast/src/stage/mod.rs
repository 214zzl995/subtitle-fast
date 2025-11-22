pub mod detector;
pub mod ocr;
pub mod progress;
pub mod progress_gui;
pub mod sampler;
pub mod segmenter;
pub mod sorter;
pub mod writer;

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use futures_util::Stream;
use tokio_stream::{StreamExt, wrappers::WatchStream};

use crate::settings::{DetectionSettings, EffectiveSettings};
use detector::Detector;
use ocr::{OcrStageError, SubtitleOcr};
use progress::Progress;
use progress_gui::{GuiProgress, GuiProgressInner};
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
    pub(crate) progress: Option<Arc<GuiProgressInner>>,
    pub(crate) pause: Option<tokio::sync::watch::Receiver<bool>>,
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
            progress: None,
            pause: None,
        })
    }
}

pub async fn run_pipeline(
    provider: DynYPlaneProvider,
    pipeline: &PipelineConfig,
) -> Result<(), (YPlaneError, u64)> {
    let initial_total_frames = provider.total_frames();
    let initial_stream = provider.into_stream();
    let paused_stream = if let Some(pause_rx) = pipeline.pause.as_ref() {
        StreamBundle::new(
            Box::pin(PauseStream::new(initial_stream, pause_rx.clone())),
            initial_total_frames,
        )
    } else {
        StreamBundle::new(initial_stream, initial_total_frames)
    };

    let sorted = FrameSorter::new().attach(paused_stream);

    let sampled = FrameSampler::new(pipeline.detection.samples_per_second).attach(sorted);

    let detector_stage =
        Detector::new(&pipeline.detection).map_err(|err| (detection_error_to_yplane(err), 0))?;

    let detected = detector_stage.attach(sampled);
    let segmented = SubtitleSegmenter::new(&pipeline.detection).attach(detected);
    let ocred = SubtitleOcr::new(Arc::clone(&pipeline.ocr.engine)).attach(segmented);
    let written = SubtitleWriter::new(pipeline.output.path.clone()).attach(ocred);
    let monitored = if let Some(handle) = &pipeline.progress {
        GuiProgress::new(Arc::clone(handle)).attach(written)
    } else {
        Progress::new("pipeline").attach(written)
    };

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

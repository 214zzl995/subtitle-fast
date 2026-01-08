use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream::unfold};
use tokio::sync::{mpsc, watch};

use super::StreamBundle;
use super::detector::DetectionSample;
use super::lifecycle::RegionTimings;
use super::ocr::OcrTimings;
use super::writer::{SubtitleWriterError, WriterResult, WriterStatus, WriterTimings};

const GUI_PROGRESS_CHANNEL_CAPACITY: usize = 4;
const SUBTITLE_TEXT_MAX_CHARS: usize = 2048;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GuiProgressUpdate {
    pub samples_seen: u64,
    pub latest_frame_index: u64,
    pub total_frames: Option<u64>,
    pub fps: f64,
    pub det_ms: f64,
    pub seg_ms: f64,
    pub ocr_ms: f64,
    pub writer_ms: f64,
    pub cues: u64,
    pub merged: u64,
    pub ocr_empty: u64,
    pub progress: f64,
    pub completed: bool,
    pub subtitle: Option<GuiSubtitleEvent>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GuiSubtitleEvent {
    pub start_ms: f64,
    pub end_ms: f64,
    pub text: String,
}

#[derive(Clone)]
pub struct GuiProgressHandle {
    inner: Arc<GuiProgressInner>,
    progress_rx: watch::Receiver<GuiProgressUpdate>,
}

impl GuiProgressHandle {
    pub fn new() -> Self {
        let (progress_tx, progress_rx) = watch::channel(GuiProgressUpdate::default());
        let inner = Arc::new(GuiProgressInner::new(progress_tx));
        Self { inner, progress_rx }
    }

    pub fn subscribe(&self) -> watch::Receiver<GuiProgressUpdate> {
        self.progress_rx.clone()
    }

    pub fn snapshot(&self) -> GuiProgressUpdate {
        self.progress_rx.borrow().clone()
    }

    pub fn reset(&self) {
        self.inner.reset(None);
    }

    pub(crate) fn inner(&self) -> Arc<GuiProgressInner> {
        Arc::clone(&self.inner)
    }
}

impl Default for GuiProgressHandle {
    fn default() -> Self {
        Self::new()
    }
}

struct GuiProgressState {
    total_frames: Option<u64>,
    samples_seen: u64,
    latest_frame_index: Option<u64>,
    started: Instant,
    avg_detection_ms: Option<f64>,
    region_frames: u64,
    region_total: Duration,
    ocr_intervals: u64,
    ocr_total: Duration,
    writer_cues: u64,
    writer_merged: u64,
    writer_empty_ocr: u64,
    writer_total: Duration,
    last_subtitle: Option<GuiSubtitleEvent>,
    last_error: Option<String>,
}

impl GuiProgressState {
    fn new(total_frames: Option<u64>) -> Self {
        Self {
            total_frames,
            samples_seen: 0,
            latest_frame_index: None,
            started: Instant::now(),
            avg_detection_ms: None,
            region_frames: 0,
            region_total: Duration::ZERO,
            ocr_intervals: 0,
            ocr_total: Duration::ZERO,
            writer_cues: 0,
            writer_merged: 0,
            writer_empty_ocr: 0,
            writer_total: Duration::ZERO,
            last_subtitle: None,
            last_error: None,
        }
    }
}

pub(crate) struct GuiProgressInner {
    state: Mutex<GuiProgressState>,
    progress_tx: watch::Sender<GuiProgressUpdate>,
}

impl GuiProgressInner {
    fn new(progress_tx: watch::Sender<GuiProgressUpdate>) -> Self {
        Self {
            state: Mutex::new(GuiProgressState::new(None)),
            progress_tx,
        }
    }

    fn reset(&self, total_frames: Option<u64>) {
        if let Ok(mut state) = self.state.lock() {
            *state = GuiProgressState::new(total_frames);
            let update = Self::snapshot(&state, false);
            drop(state);
            self.emit_progress(update);
        }
    }

    fn observe(&self, event: &WriterResult) {
        match event {
            Ok(event) => {
                self.observe_writer_event(event);
            }
            Err(err) => {
                self.emit_error(err);
            }
        }
    }

    fn observe_writer_event(&self, event: &super::writer::WriterEvent) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(sample) = &event.sample {
                Self::observe_sample(&mut state, sample);
            }
            Self::observe_region_time(&mut state, event.region_timings);
            Self::observe_ocr_time(&mut state, event.ocr_timings);
            let completed = matches!(event.status, WriterStatus::Completed { .. });
            Self::observe_writer_time(&mut state, event.writer_timings, completed);
            if let Some(subtitle) = event.last_subtitle.as_ref() {
                let limited = subtitle
                    .text
                    .chars()
                    .take(SUBTITLE_TEXT_MAX_CHARS)
                    .collect::<String>();
                state.last_subtitle = Some(GuiSubtitleEvent {
                    start_ms: subtitle.start_ms,
                    end_ms: subtitle.end_ms,
                    text: limited,
                });
            }

            let update = Self::snapshot(&state, completed);
            drop(state);
            self.emit_progress(update);
        }
    }

    fn finish(&self) {
        if let Ok(state) = self.state.lock() {
            let update = Self::snapshot(&state, true);
            drop(state);
            self.emit_progress(update);
        }
    }

    fn observe_sample(state: &mut GuiProgressState, sample: &DetectionSample) {
        state.samples_seen = state.samples_seen.saturating_add(1);
        if let Some(total) = state.total_frames {
            let frame_index = sample.sample.frame_index();
            state.latest_frame_index = Some(frame_index);
            let next = std::cmp::min(frame_index.saturating_add(1), total);
            state.samples_seen = next;
        }
        Self::observe_detection_time(state, sample.elapsed);
    }

    fn observe_detection_time(state: &mut GuiProgressState, elapsed: Duration) {
        let millis = elapsed.as_secs_f64() * 1000.0;
        let alpha = 0.1;
        state.avg_detection_ms = Some(match state.avg_detection_ms {
            Some(current) => (1.0 - alpha) * current + alpha * millis,
            None => millis,
        });
    }

    fn observe_region_time(state: &mut GuiProgressState, timings: Option<RegionTimings>) {
        let Some(timings) = timings else {
            return;
        };
        state.region_frames = state.region_frames.saturating_add(timings.frames);
        state.region_total = state.region_total.saturating_add(timings.total);
    }

    fn observe_ocr_time(state: &mut GuiProgressState, timings: Option<OcrTimings>) {
        let Some(timings) = timings else {
            return;
        };
        state.ocr_intervals = state.ocr_intervals.saturating_add(timings.intervals);
        state.ocr_total = state.ocr_total.saturating_add(timings.total);
    }

    fn observe_writer_time(
        state: &mut GuiProgressState,
        timings: Option<WriterTimings>,
        completed: bool,
    ) {
        let Some(timings) = timings else {
            return;
        };
        if completed {
            state.writer_cues = timings.cues;
            state.writer_merged = timings.merged;
        } else if timings.cues > 0 {
            state.writer_cues = state.writer_cues.saturating_add(timings.cues);
        }
        if !completed {
            state.writer_merged = state.writer_merged.saturating_add(timings.merged);
        }
        state.writer_empty_ocr = state.writer_empty_ocr.saturating_add(timings.ocr_empty);
        state.writer_total = state.writer_total.saturating_add(timings.total);
    }

    fn snapshot(state: &GuiProgressState, completed: bool) -> GuiProgressUpdate {
        let total_frames = state.total_frames;
        let latest = state.latest_frame_index.unwrap_or(state.samples_seen);
        let elapsed = state.started.elapsed().as_secs_f64();
        GuiProgressUpdate {
            samples_seen: state.samples_seen,
            latest_frame_index: latest,
            total_frames,
            fps: if elapsed > 0.0 {
                (latest as f64) / elapsed
            } else {
                0.0
            },
            det_ms: state.avg_detection_ms.unwrap_or(0.0),
            seg_ms: average_ms(state.region_total, state.region_frames),
            ocr_ms: average_ms(state.ocr_total, state.ocr_intervals),
            writer_ms: average_ms(state.writer_total, state.writer_cues),
            cues: state.writer_cues,
            merged: state.writer_merged,
            ocr_empty: state.writer_empty_ocr,
            progress: if let Some(total) = total_frames {
                if total > 0 {
                    (latest as f64) / (total as f64)
                } else {
                    0.0
                }
            } else {
                0.0
            },
            completed,
            subtitle: state.last_subtitle.clone(),
            error: state.last_error.clone(),
        }
    }

    fn emit_progress(&self, update: GuiProgressUpdate) {
        let _ = self.progress_tx.send_if_modified(|current| {
            if *current == update {
                return false;
            }
            *current = update;
            true
        });
    }

    fn emit_error(&self, err: &SubtitleWriterError) {
        if let Ok(mut state) = self.state.lock() {
            state.last_error = Some(describe_error(err));
            let update = Self::snapshot(&state, false);
            drop(state);
            self.emit_progress(update);
        }
    }
}

pub struct GuiProgress {
    handle: Arc<GuiProgressInner>,
}

impl GuiProgress {
    pub(crate) fn new(handle: Arc<GuiProgressInner>) -> Self {
        Self { handle }
    }

    pub fn attach(self, input: StreamBundle<WriterResult>) -> StreamBundle<WriterResult> {
        let StreamBundle {
            stream,
            total_frames,
        } = input;

        self.handle.reset(total_frames);

        let (tx, rx) = mpsc::channel::<WriterResult>(GUI_PROGRESS_CHANNEL_CAPACITY);
        let handle = self.handle;

        tokio::spawn(async move {
            let mut upstream = stream;
            while let Some(event) = upstream.next().await {
                handle.observe(&event);
                if tx.send(event).await.is_err() {
                    return;
                }
            }
            handle.finish();
        });

        let stream = Box::pin(unfold(rx, |mut receiver| async {
            receiver.recv().await.map(|item| (item, receiver))
        }));

        StreamBundle::new(stream, total_frames)
    }
}

fn average_ms(total: Duration, units: u64) -> f64 {
    if units == 0 {
        return 0.0;
    }
    total.as_secs_f64() * 1000.0 / units as f64
}

fn describe_error(err: &SubtitleWriterError) -> String {
    match err {
        SubtitleWriterError::Ocr(ocr_err) => match ocr_err {
            super::ocr::OcrStageError::Lifecycle(lifecycle_err) => match lifecycle_err {
                super::lifecycle::RegionLifecycleError::Determiner(det_err) => match det_err {
                    super::determiner::RegionDeterminerError::Detector(detector_err) => {
                        match detector_err {
                            super::detector::DetectorError::Sampler(sampler_err) => {
                                format!("sampler error: {sampler_err}")
                            }
                            super::detector::DetectorError::Detection(det_err) => {
                                format!("detector error: {det_err}")
                            }
                        }
                    }
                },
            },
            super::ocr::OcrStageError::Engine(engine_err) => {
                format!("ocr error: {engine_err}")
            }
        },
        SubtitleWriterError::Io { path, source } => {
            format!("writer error ({}): {source}", path.display())
        }
    }
}

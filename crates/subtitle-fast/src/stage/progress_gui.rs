use std::collections::HashMap;
use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream::unfold};
use tokio::sync::mpsc;

use super::StreamBundle;
use super::detector::DetectionSample;
use super::lifecycle::RegionTimings;
use super::ocr::OcrTimings;
use super::writer::{SubtitleWriterError, WriterResult, WriterStatus, WriterTimings};

const GUI_PROGRESS_CHANNEL_CAPACITY: usize = 4;
const SUBTITLE_TEXT_MAX_BYTES: usize = 2048;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GuiProgressCallbacks {
    pub user_data: *mut std::os::raw::c_void,
    pub on_progress: Option<extern "C" fn(*const GuiProgressUpdate, *mut std::os::raw::c_void)>,
    pub on_error: Option<extern "C" fn(*const GuiProgressError, *mut std::os::raw::c_void)>,
}

unsafe impl Send for GuiProgressCallbacks {}
unsafe impl Sync for GuiProgressCallbacks {}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct GuiProgressUpdate {
    pub handle_id: u64,
    pub samples_seen: u64,
    pub latest_frame_index: u64,
    pub total_frames: u64,
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
    pub subtitle_start_ms: f64,
    pub subtitle_end_ms: f64,
    pub subtitle_text: *const c_char,
    pub subtitle_present: u8,
}

#[repr(C)]
pub struct GuiProgressError {
    pub message: *const c_char,
}

pub struct GuiProgressHandle {
    inner: Arc<GuiProgressInner>,
}

impl GuiProgressHandle {
    pub(crate) fn new(
        handle_id: u64,
        callbacks: GuiProgressCallbacks,
        total_frames: Option<u64>,
    ) -> Self {
        Self {
            inner: Arc::new(GuiProgressInner::new(handle_id, callbacks, total_frames)),
        }
    }

    pub(crate) fn inner(&self) -> Arc<GuiProgressInner> {
        Arc::clone(&self.inner)
    }
}

struct GuiProgressState {
    handle_id: u64,
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
}

pub(crate) struct GuiProgressInner {
    callbacks: GuiProgressCallbacks,
    state: Mutex<GuiProgressState>,
    last_subtitle: Mutex<Option<GuiSubtitleEvent>>,
}

#[derive(Clone)]
struct GuiSubtitleEvent {
    start_ms: f64,
    end_ms: f64,
    text: CString,
}

impl GuiProgressInner {
    fn new(handle_id: u64, callbacks: GuiProgressCallbacks, total_frames: Option<u64>) -> Self {
        let state = GuiProgressState {
            handle_id,
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
        };
        Self {
            callbacks,
            state: Mutex::new(state),
            last_subtitle: Mutex::new(None),
        }
    }

    fn set_total_frames(&self, total_frames: Option<u64>) {
        if let Ok(mut state) = self.state.lock() {
            state.total_frames = total_frames;
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
                    .take(SUBTITLE_TEXT_MAX_BYTES)
                    .collect::<String>();
                if let Ok(text) = CString::new(limited) {
                    let event = GuiSubtitleEvent {
                        start_ms: subtitle.start_ms,
                        end_ms: subtitle.end_ms,
                        text,
                    };
                    if let Ok(mut slot) = self.last_subtitle.lock() {
                        *slot = Some(event);
                    }
                }
            }

            let update = Self::snapshot(&state, completed);
            drop(state);
            self.emit_progress(&update);
        }
    }

    fn finish(&self) {
        if let Ok(state) = self.state.lock() {
            let update = Self::snapshot(&state, true);
            drop(state);
            self.emit_progress(&update);
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
        let total_frames = state.total_frames.unwrap_or(0);
        let latest = state.latest_frame_index.unwrap_or(state.samples_seen);
        let elapsed = state.started.elapsed().as_secs_f64();
        GuiProgressUpdate {
            handle_id: state.handle_id,
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
            progress: if total_frames > 0 {
                (latest as f64) / (total_frames as f64)
            } else {
                0.0
            },
            completed,
            subtitle_start_ms: 0.0,
            subtitle_end_ms: 0.0,
            subtitle_text: std::ptr::null(),
            subtitle_present: 0,
        }
    }

    fn emit_progress(&self, update: &GuiProgressUpdate) {
        let callbacks = self.callbacks;
        if let Some(on_progress) = callbacks.on_progress {
            let mut update_with_sub = *update;
            if let Ok(mut slot) = self.last_subtitle.lock()
                && let Some(sub) = slot.take()
            {
                update_with_sub.subtitle_present = 1;
                update_with_sub.subtitle_start_ms = sub.start_ms;
                update_with_sub.subtitle_end_ms = sub.end_ms;
                update_with_sub.subtitle_text = sub.text.as_ptr();
                on_progress(
                    &update_with_sub as *const GuiProgressUpdate,
                    callbacks.user_data,
                );
                return;
            }
            on_progress(
                &update_with_sub as *const GuiProgressUpdate,
                callbacks.user_data,
            );
        }
    }

    fn emit_error(&self, err: &SubtitleWriterError) {
        let Some(on_error) = self.callbacks.on_error else {
            return;
        };

        let message = describe_error(err);
        if let Ok(c_string) = CString::new(message) {
            let err = GuiProgressError {
                message: c_string.as_ptr(),
            };
            on_error(&err, self.callbacks.user_data);
            // c_string is dropped after callback; consumers must copy if needed.
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

        self.handle.set_total_frames(total_frames);

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

static GLOBAL_CALLBACKS: Mutex<Option<GuiProgressCallbacks>> = Mutex::new(None);
static HANDLE_MAP: LazyLock<Mutex<HashMap<u64, Arc<GuiProgressInner>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) fn install_callbacks(callbacks: GuiProgressCallbacks) {
    if let Ok(mut slot) = GLOBAL_CALLBACKS.lock() {
        *slot = Some(callbacks);
    }
}

pub(crate) fn callbacks() -> Option<GuiProgressCallbacks> {
    GLOBAL_CALLBACKS
        .lock()
        .ok()
        .and_then(|c| c.as_ref().copied())
}

pub(crate) fn create_progress_handle(
    handle_id: u64,
    total_frames: Option<u64>,
) -> Option<Arc<GuiProgressInner>> {
    let callbacks = callbacks()?;
    let handle = GuiProgressHandle::new(handle_id, callbacks, total_frames).inner();
    if let Ok(mut map) = HANDLE_MAP.lock() {
        map.insert(handle_id, Arc::clone(&handle));
    }
    Some(handle)
}

pub(crate) fn drop_progress_handle(handle_id: u64) {
    if let Ok(mut map) = HANDLE_MAP.lock() {
        map.remove(&handle_id);
    }
}

pub(crate) fn progress_for_handle(handle_id: u64) -> Option<Arc<GuiProgressInner>> {
    HANDLE_MAP
        .lock()
        .ok()
        .and_then(|m| m.get(&handle_id).cloned())
}

/// # Safety
/// `callbacks` must point to a valid `GuiProgressCallbacks` struct for the lifetime of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn progress_gui_init(callbacks: *const GuiProgressCallbacks) {
    if callbacks.is_null() {
        return;
    }
    unsafe {
        install_callbacks(*callbacks);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn progress_gui_shutdown() {
    if let Ok(mut map) = HANDLE_MAP.lock() {
        map.clear();
    }
    if let Ok(mut slot) = GLOBAL_CALLBACKS.lock() {
        *slot = None;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn progress_gui_version() -> *const c_char {
    static VERSION: &[u8] = b"0.1.0\0";
    VERSION.as_ptr() as *const c_char
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

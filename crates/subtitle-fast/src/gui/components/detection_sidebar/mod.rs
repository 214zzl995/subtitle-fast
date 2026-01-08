use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures_util::StreamExt;
use tokio::sync::{broadcast, oneshot, watch};

use crate::gui::components::{VideoLumaHandle, VideoRoiHandle};
use crate::gui::runtime;
use crate::settings::{DecoderSettings, DetectionSettings, EffectiveSettings, OutputSettings};
use crate::stage::{
    self, MergedSubtitle, PipelineConfig, PipelineHandle, PipelineProgress, SubtitleUpdate,
    SubtitleUpdateKind, TimedSubtitle,
};
use subtitle_fast_decoder::{Backend, Configuration};
use subtitle_fast_types::{DecoderError, RoiConfig};
use subtitle_fast_validator::subtitle_detection::{DEFAULT_DELTA, DEFAULT_TARGET};

pub mod controls;
pub mod metrics;
pub mod panel;
pub mod subtitles;

pub use controls::DetectionControls;
pub use metrics::DetectionMetrics;
pub use panel::DetectionSidebar;
pub use subtitles::DetectedSubtitlesList;

const DEFAULT_SAMPLES_PER_SECOND: u32 = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectionRunState {
    Idle,
    Running,
    Paused,
}

impl DetectionRunState {
    pub fn is_running(self) -> bool {
        matches!(self, Self::Running | Self::Paused)
    }

    pub fn is_paused(self) -> bool {
        matches!(self, Self::Paused)
    }
}

#[derive(Clone, Debug)]
pub enum SubtitleMessage {
    Reset,
    New(TimedSubtitle),
    Updated(TimedSubtitle),
}

#[derive(Clone)]
pub struct DetectionHandle {
    inner: Arc<DetectionPipelineInner>,
}

impl DetectionHandle {
    pub fn new() -> Self {
        let (state_tx, state_rx) = watch::channel(DetectionRunState::Idle);
        let (progress_tx, progress_rx) = watch::channel(PipelineProgress::default());
        let (subtitle_tx, _subtitle_rx) = broadcast::channel(128);
        let inner = Arc::new(DetectionPipelineInner {
            state_tx,
            state_rx,
            pause_handle: Mutex::new(None),
            progress_tx,
            progress_rx,
            video_path: Mutex::new(None),
            luma_handle: Mutex::new(None),
            roi_handle: Mutex::new(None),
            cancel_tx: Mutex::new(None),
            subtitle_tx,
            subtitles: Mutex::new(Vec::new()),
        });
        Self { inner }
    }

    pub fn set_video_path(&self, path: Option<PathBuf>) {
        self.inner.set_video_path(path);
    }

    pub fn set_luma_handle(&self, handle: Option<VideoLumaHandle>) {
        self.inner.set_luma_handle(handle);
    }

    pub fn set_roi_handle(&self, handle: Option<VideoRoiHandle>) {
        self.inner.set_roi_handle(handle);
    }

    pub fn subscribe_state(&self) -> watch::Receiver<DetectionRunState> {
        self.inner.subscribe_state()
    }

    pub fn subscribe_progress(&self) -> watch::Receiver<PipelineProgress> {
        self.inner.subscribe_progress()
    }

    pub fn progress_snapshot(&self) -> PipelineProgress {
        self.inner.progress_snapshot()
    }

    pub fn run_state(&self) -> DetectionRunState {
        self.inner.run_state()
    }

    pub fn has_video(&self) -> bool {
        self.inner.has_video()
    }

    pub fn start(&self) -> DetectionRunState {
        self.inner.start()
    }

    pub fn toggle_pause(&self) -> DetectionRunState {
        self.inner.toggle_pause()
    }

    pub fn cancel(&self) -> DetectionRunState {
        self.inner.cancel()
    }

    pub fn subscribe_subtitles(&self) -> broadcast::Receiver<SubtitleMessage> {
        self.inner.subscribe_subtitles()
    }

    pub fn subtitles_snapshot(&self) -> Vec<TimedSubtitle> {
        self.inner.subtitles_snapshot()
    }

    pub fn has_subtitles(&self) -> bool {
        self.inner.has_subtitles()
    }

    pub fn export_dialog_seed(&self) -> (PathBuf, Option<String>) {
        self.inner.export_dialog_seed()
    }

    pub fn export_subtitles_to(&self, path: PathBuf) {
        self.inner.export_subtitles_to(path);
    }
}

impl Default for DetectionHandle {
    fn default() -> Self {
        Self::new()
    }
}

struct DetectionPipelineInner {
    state_tx: watch::Sender<DetectionRunState>,
    state_rx: watch::Receiver<DetectionRunState>,
    pause_handle: Mutex<Option<PipelineHandle>>,
    progress_tx: watch::Sender<PipelineProgress>,
    progress_rx: watch::Receiver<PipelineProgress>,
    video_path: Mutex<Option<PathBuf>>,
    luma_handle: Mutex<Option<VideoLumaHandle>>,
    roi_handle: Mutex<Option<VideoRoiHandle>>,
    cancel_tx: Mutex<Option<oneshot::Sender<()>>>,
    subtitle_tx: broadcast::Sender<SubtitleMessage>,
    subtitles: Mutex<Vec<MergedSubtitle>>,
}

impl DetectionPipelineInner {
    fn set_video_path(&self, path: Option<PathBuf>) {
        if let Ok(mut slot) = self.video_path.lock() {
            *slot = path;
        }
    }

    fn set_luma_handle(&self, handle: Option<VideoLumaHandle>) {
        if let Ok(mut slot) = self.luma_handle.lock() {
            *slot = handle;
        }
    }

    fn set_roi_handle(&self, handle: Option<VideoRoiHandle>) {
        if let Ok(mut slot) = self.roi_handle.lock() {
            *slot = handle;
        }
    }

    fn subscribe_state(&self) -> watch::Receiver<DetectionRunState> {
        self.state_rx.clone()
    }

    fn subscribe_progress(&self) -> watch::Receiver<PipelineProgress> {
        self.progress_rx.clone()
    }

    fn progress_snapshot(&self) -> PipelineProgress {
        self.progress_rx.borrow().clone()
    }

    fn run_state(&self) -> DetectionRunState {
        *self.state_rx.borrow()
    }

    fn has_video(&self) -> bool {
        let path = self.video_path.lock().ok().and_then(|slot| slot.clone());
        path.is_some_and(|path| path.exists())
    }

    fn start(self: &Arc<Self>) -> DetectionRunState {
        if self.run_state() != DetectionRunState::Idle {
            return self.run_state();
        }

        let path = match self.video_path.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => None,
        };
        let Some(path) = path else {
            eprintln!("detection start ignored: no video selected");
            return self.run_state();
        };
        if !path.exists() {
            eprintln!("detection start ignored: selected video is missing");
            return self.run_state();
        }

        let detection_settings = self.current_detection_settings();
        let settings = EffectiveSettings {
            detection: detection_settings,
            decoder: DecoderSettings {
                backend: None,
                channel_capacity: None,
            },
            output: OutputSettings { path: None },
        };
        let plan = match build_detection_plan(&path, &settings) {
            Ok(plan) => plan,
            Err(err) => {
                eprintln!("detection start failed: {err}");
                return self.run_state();
            }
        };

        let (cancel_tx, cancel_rx) = oneshot::channel();

        let inner = Arc::clone(self);
        if runtime::spawn(run_detection_task(inner, plan, cancel_rx)).is_none() {
            eprintln!("detection start failed: tokio runtime not initialized");
            let _ = self.state_tx.send(DetectionRunState::Idle);
            return self.run_state();
        }

        if let Ok(mut slot) = self.cancel_tx.lock() {
            *slot = Some(cancel_tx);
        }

        self.reset_subtitles();
        self.update_progress(PipelineProgress::default());
        let _ = self.state_tx.send(DetectionRunState::Running);
        DetectionRunState::Running
    }

    fn toggle_pause(&self) -> DetectionRunState {
        let Some(handle) = self.pause_handle.lock().ok().and_then(|slot| slot.clone()) else {
            return self.run_state();
        };

        match self.run_state() {
            DetectionRunState::Running => {
                handle.set_paused(true);
                let _ = self.state_tx.send(DetectionRunState::Paused);
                DetectionRunState::Paused
            }
            DetectionRunState::Paused => {
                handle.set_paused(false);
                let _ = self.state_tx.send(DetectionRunState::Running);
                DetectionRunState::Running
            }
            DetectionRunState::Idle => DetectionRunState::Idle,
        }
    }

    fn cancel(&self) -> DetectionRunState {
        if !self.run_state().is_running() {
            return self.run_state();
        }

        if let Ok(mut slot) = self.cancel_tx.lock() {
            if let Some(cancel_tx) = slot.take() {
                let _ = cancel_tx.send(());
            }
        }

        self.clear_pause(false);
        self.update_progress(PipelineProgress::default());
        self.reset_subtitles();
        if let Ok(mut pause_slot) = self.pause_handle.lock() {
            *pause_slot = None;
        }
        let _ = self.state_tx.send(DetectionRunState::Idle);
        DetectionRunState::Idle
    }

    fn finish(&self) {
        self.clear_pause(false);
        if let Ok(mut pause_slot) = self.pause_handle.lock() {
            *pause_slot = None;
        }
        if let Ok(mut slot) = self.cancel_tx.lock() {
            *slot = None;
        }
        let _ = self.state_tx.send(DetectionRunState::Idle);
    }

    fn set_pause_handle(&self, handle: PipelineHandle) {
        if let Ok(mut slot) = self.pause_handle.lock() {
            *slot = Some(handle);
        }
    }

    fn clear_pause(&self, paused: bool) {
        if let Ok(slot) = self.pause_handle.lock() {
            if let Some(handle) = slot.as_ref() {
                handle.set_paused(paused);
            }
        }
    }

    fn update_progress(&self, progress: PipelineProgress) {
        let _ = self.progress_tx.send_if_modified(|current| {
            if *current == progress {
                return false;
            }
            *current = progress;
            true
        });
    }

    fn apply_updates(&self, updates: &[SubtitleUpdate]) {
        if updates.is_empty() {
            return;
        }

        if let Ok(mut slot) = self.subtitles.lock() {
            for update in updates {
                match update.kind {
                    SubtitleUpdateKind::New => slot.push(update.subtitle.clone()),
                    SubtitleUpdateKind::Updated => {
                        if let Some(existing) = slot
                            .iter_mut()
                            .find(|subtitle| subtitle.id == update.subtitle.id)
                        {
                            *existing = update.subtitle.clone();
                        } else {
                            slot.push(update.subtitle.clone());
                        }
                    }
                }

                let timed = update.subtitle.as_timed();
                let message = match update.kind {
                    SubtitleUpdateKind::New => SubtitleMessage::New(timed),
                    SubtitleUpdateKind::Updated => SubtitleMessage::Updated(timed),
                };
                let _ = self.subtitle_tx.send(message);
            }
        }
    }

    fn current_detection_settings(&self) -> DetectionSettings {
        let luma_handle = self
            .luma_handle
            .lock()
            .ok()
            .and_then(|handle| handle.clone());
        let roi_handle = self
            .roi_handle
            .lock()
            .ok()
            .and_then(|handle| handle.clone());

        let (target, delta) = luma_handle
            .map(|handle| {
                let values = handle.latest();
                (values.target, values.delta)
            })
            .unwrap_or((DEFAULT_TARGET, DEFAULT_DELTA));

        let roi = roi_handle
            .map(|handle| handle.latest())
            .unwrap_or_else(full_frame_roi);

        DetectionSettings {
            samples_per_second: DEFAULT_SAMPLES_PER_SECOND,
            target,
            delta,
            comparator: None,
            roi: Some(roi),
        }
    }

    fn subscribe_subtitles(&self) -> broadcast::Receiver<SubtitleMessage> {
        self.subtitle_tx.subscribe()
    }

    fn has_subtitles(&self) -> bool {
        self.subtitles
            .lock()
            .map(|slot| !slot.is_empty())
            .unwrap_or(false)
    }

    fn subtitles_snapshot(&self) -> Vec<TimedSubtitle> {
        let mut snapshot = self
            .subtitles
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or_default();
        stage::sort_subtitles(&mut snapshot);
        snapshot
            .into_iter()
            .map(|subtitle| subtitle.as_timed())
            .collect()
    }

    fn reset_subtitles(&self) {
        if let Ok(mut slot) = self.subtitles.lock() {
            slot.clear();
        }
        let _ = self.subtitle_tx.send(SubtitleMessage::Reset);
    }

    fn export_dialog_seed(&self) -> (PathBuf, Option<String>) {
        let video_path = self.video_path.lock().ok().and_then(|slot| slot.clone());
        if let Some(path) = video_path {
            let directory = path
                .parent()
                .map(|parent| parent.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            let suggested_name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(|stem| format!("{stem}.srt"))
                .or_else(|| Some("subtitles.srt".to_string()));
            return (directory, suggested_name);
        }

        let directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        (directory, Some("subtitles.srt".to_string()))
    }

    fn export_subtitles_to(&self, path: PathBuf) {
        let subtitles = self
            .subtitles
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or_default();
        if subtitles.is_empty() {
            eprintln!("export ignored: no subtitles detected");
            return;
        }

        let mut ordered = subtitles;
        stage::sort_subtitles(&mut ordered);
        let contents = stage::render_srt(&ordered);
        let task = runtime::spawn(async move {
            if let Err(err) = tokio::fs::write(&path, contents).await {
                eprintln!("subtitle export failed: {err}");
            } else {
                eprintln!("exported subtitles to {}", path.display());
            }
        });

        if task.is_none() {
            eprintln!("subtitle export failed: tokio runtime not initialized");
        }
    }
}

async fn run_detection_task(
    inner: Arc<DetectionPipelineInner>,
    plan: DetectionPlan,
    mut cancel_rx: oneshot::Receiver<()>,
) {
    let DetectionPlan {
        config,
        backend_locked,
        pipeline,
    } = plan;

    let available = Configuration::available_backends();
    if available.is_empty() {
        eprintln!("detection start failed: no decoding backend available");
        inner.finish();
        return;
    }
    if !available.contains(&config.backend) {
        eprintln!(
            "detection start failed: backend '{}' is unavailable",
            config.backend.as_str()
        );
        inner.finish();
        return;
    }

    let mut attempt_config = config.clone();
    let mut tried = Vec::new();

    loop {
        if !tried.contains(&attempt_config.backend) {
            tried.push(attempt_config.backend);
        }

        let provider_started = Instant::now();
        let provider_result = attempt_config.create_provider();
        let provider_elapsed = provider_started.elapsed();

        let provider = match provider_result {
            Ok(provider) => {
                eprintln!(
                    "initialized decoder backend '{}' in {:.2?}",
                    attempt_config.backend.as_str(),
                    provider_elapsed
                );
                provider
            }
            Err(err) => {
                eprintln!(
                    "decoder backend '{}' failed to initialize in {:.2?}: {err}",
                    attempt_config.backend.as_str(),
                    provider_elapsed
                );
                if !backend_locked {
                    if let Some(next_backend) = select_next_backend(&available, &tried) {
                        let failed_backend = attempt_config.backend;
                        eprintln!(
                            "backend {failed} failed to initialize ({reason}); trying {next}",
                            failed = failed_backend.as_str(),
                            reason = err,
                            next = next_backend.as_str()
                        );
                        attempt_config.backend = next_backend;
                        continue;
                    }
                }
                inner.finish();
                return;
            }
        };

        let streams = match stage::build_pipeline(provider, &pipeline) {
            Ok(streams) => streams,
            Err(err) => {
                eprintln!("detection pipeline setup failed: {err}");
                if !backend_locked {
                    if let Some(next_backend) = select_next_backend(&available, &tried) {
                        attempt_config.backend = next_backend;
                        continue;
                    }
                }
                inner.finish();
                return;
            }
        };

        inner.set_pause_handle(streams.handle.clone());

        let result = drive_gui_pipeline(Arc::clone(&inner), streams, &mut cancel_rx).await;

        match result {
            Ok(()) => {
                inner.finish();
                return;
            }
            Err((err, processed)) => {
                eprintln!("detection pipeline failed: {err}");
                if processed == 0 && !backend_locked {
                    if let Some(next_backend) = select_next_backend(&available, &tried) {
                        let failed_backend = attempt_config.backend;
                        eprintln!(
                            "backend {failed} failed to decode ({reason}); trying {next}",
                            failed = failed_backend.as_str(),
                            reason = err,
                            next = next_backend.as_str()
                        );
                        attempt_config.backend = next_backend;
                        continue;
                    }
                }
                inner.finish();
                return;
            }
        }
    }
}

async fn drive_gui_pipeline(
    inner: Arc<DetectionPipelineInner>,
    streams: stage::PipelineOutputs,
    cancel_rx: &mut oneshot::Receiver<()>,
) -> Result<(), (DecoderError, u64)> {
    let mut processed = 0;
    let mut stream = streams.stream;

    loop {
        tokio::select! {
            _ = &mut *cancel_rx => return Ok(()),
            maybe_event = stream.next() => {
                match maybe_event {
                    Some(Ok(update)) => {
                        processed = processed.max(update.progress.samples_seen);
                        inner.update_progress(update.progress);
                        inner.apply_updates(&update.updates);
                    }
                    Some(Err(err)) => {
                        return Err((stage::pipeline_error_to_frame(err), processed));
                    }
                    None => break,
                }
            }
        }
    }

    Ok(())
}

fn build_detection_plan(
    input: &Path,
    settings: &EffectiveSettings,
) -> Result<DetectionPlan, DecoderError> {
    if !input.exists() {
        return Err(DecoderError::configuration(format!(
            "input file '{}' does not exist",
            input.display()
        )));
    }

    let pipeline = PipelineConfig::from_settings(settings, input)?;

    let env_backend_present = std::env::var("SUBFAST_BACKEND").is_ok();
    let mut config = Configuration::from_env().unwrap_or_default();
    let backend_override = match settings.decoder.backend.as_deref() {
        Some(name) => Some(parse_backend_value(name)?),
        None => None,
    };
    let backend_locked = backend_override.is_some() || env_backend_present;
    if let Some(backend_value) = backend_override {
        config.backend = backend_value;
    }
    config.input = Some(input.to_path_buf());
    if let Some(capacity) = settings.decoder.channel_capacity
        && let Some(non_zero) = NonZeroUsize::new(capacity)
    {
        config.channel_capacity = Some(non_zero);
    }

    Ok(DetectionPlan {
        config,
        backend_locked,
        pipeline,
    })
}

fn full_frame_roi() -> RoiConfig {
    RoiConfig {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    }
}

fn parse_backend_value(value: &str) -> Result<Backend, DecoderError> {
    use std::str::FromStr;
    Backend::from_str(value)
}

fn select_next_backend(available: &[Backend], tried: &[Backend]) -> Option<Backend> {
    available
        .iter()
        .copied()
        .find(|backend| !tried.contains(backend))
}

struct DetectionPlan {
    config: Configuration,
    backend_locked: bool,
    pipeline: PipelineConfig,
}

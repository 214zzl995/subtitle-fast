use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;

use tokio::sync::{oneshot, watch};

use crate::backend::{self, ExecutionPlan};
use crate::gui::components::{VideoLumaHandle, VideoRoiHandle};
use crate::gui::runtime;
use crate::settings::{DecoderSettings, DetectionSettings, EffectiveSettings, OutputSettings};
use crate::stage::PipelineConfig;
use crate::stage::progress_gui::{GuiProgressHandle, GuiProgressUpdate};
use subtitle_fast_decoder::Configuration;
use subtitle_fast_types::{DecoderError, RoiConfig};
use subtitle_fast_validator::subtitle_detection::{DEFAULT_DELTA, DEFAULT_TARGET};

pub mod controls;
pub mod panel;

pub use controls::DetectionControls;
pub use panel::DetectionSidebar;

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

#[derive(Clone)]
pub struct DetectionHandle {
    inner: Arc<DetectionPipelineInner>,
}

impl DetectionHandle {
    pub fn new() -> Self {
        let (state_tx, state_rx) = watch::channel(DetectionRunState::Idle);
        let (pause_tx, _pause_rx) = watch::channel(false);
        Self {
            inner: Arc::new(DetectionPipelineInner {
                state_tx,
                state_rx,
                pause_tx,
                progress: GuiProgressHandle::new(),
                video_path: Mutex::new(None),
                luma_handle: Mutex::new(None),
                roi_handle: Mutex::new(None),
                cancel_tx: Mutex::new(None),
            }),
        }
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

    pub fn subscribe_progress(&self) -> watch::Receiver<GuiProgressUpdate> {
        self.inner.subscribe_progress()
    }

    pub fn progress_snapshot(&self) -> GuiProgressUpdate {
        self.inner.progress_snapshot()
    }

    pub fn run_state(&self) -> DetectionRunState {
        self.inner.run_state()
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
}

impl Default for DetectionHandle {
    fn default() -> Self {
        Self::new()
    }
}

struct DetectionPipelineInner {
    state_tx: watch::Sender<DetectionRunState>,
    state_rx: watch::Receiver<DetectionRunState>,
    pause_tx: watch::Sender<bool>,
    progress: GuiProgressHandle,
    video_path: Mutex<Option<PathBuf>>,
    luma_handle: Mutex<Option<VideoLumaHandle>>,
    roi_handle: Mutex<Option<VideoRoiHandle>>,
    cancel_tx: Mutex<Option<oneshot::Sender<()>>>,
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

    fn subscribe_progress(&self) -> watch::Receiver<GuiProgressUpdate> {
        self.progress.subscribe()
    }

    fn progress_snapshot(&self) -> GuiProgressUpdate {
        self.progress.snapshot()
    }

    fn run_state(&self) -> DetectionRunState {
        *self.state_rx.borrow()
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
        let plan = match build_execution_plan(&path, &settings) {
            Ok(plan) => plan,
            Err(err) => {
                eprintln!("detection start failed: {err}");
                return self.run_state();
            }
        };

        let pause_rx = self.pause_tx.subscribe();
        let (cancel_tx, cancel_rx) = oneshot::channel();

        let inner = Arc::clone(self);
        if runtime::spawn(run_detection_task(inner, plan, pause_rx, cancel_rx)).is_none() {
            eprintln!("detection start failed: tokio runtime not initialized");
            let _ = self.state_tx.send(DetectionRunState::Idle);
            return self.run_state();
        }

        if let Ok(mut slot) = self.cancel_tx.lock() {
            *slot = Some(cancel_tx);
        }

        let _ = self.pause_tx.send(false);
        let _ = self.state_tx.send(DetectionRunState::Running);
        DetectionRunState::Running
    }

    fn toggle_pause(&self) -> DetectionRunState {
        match self.run_state() {
            DetectionRunState::Running => {
                let _ = self.pause_tx.send(true);
                let _ = self.state_tx.send(DetectionRunState::Paused);
                DetectionRunState::Paused
            }
            DetectionRunState::Paused => {
                let _ = self.pause_tx.send(false);
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

        let _ = self.pause_tx.send(false);
        let _ = self.state_tx.send(DetectionRunState::Idle);
        DetectionRunState::Idle
    }

    fn finish(&self) {
        let _ = self.pause_tx.send(false);
        let _ = self.state_tx.send(DetectionRunState::Idle);
        if let Ok(mut slot) = self.cancel_tx.lock() {
            *slot = None;
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
}

async fn run_detection_task(
    inner: Arc<DetectionPipelineInner>,
    plan: ExecutionPlan,
    pause_rx: watch::Receiver<bool>,
    cancel_rx: oneshot::Receiver<()>,
) {
    let progress = inner.progress.inner();
    let result = tokio::select! {
        _ = cancel_rx => Ok(()),
        result = backend::run_with_progress(plan, progress, pause_rx) => result,
    };

    if let Err(err) = result {
        eprintln!("detection pipeline failed: {err}");
    }

    inner.finish();
}

fn build_execution_plan(
    input: &Path,
    settings: &EffectiveSettings,
) -> Result<ExecutionPlan, DecoderError> {
    if !input.exists() {
        return Err(DecoderError::configuration(format!(
            "input file '{}' does not exist",
            input.display()
        )));
    }

    let pipeline = PipelineConfig::from_settings(settings, input)?;

    let mut config = Configuration::default();
    let backend_override = match settings.decoder.backend.as_deref() {
        Some(name) => Some(backend::parse_backend(name)?),
        None => None,
    };
    if let Some(backend_value) = backend_override {
        config.backend = backend_value;
    }
    config.input = Some(input.to_path_buf());
    if let Some(capacity) = settings.decoder.channel_capacity
        && let Some(non_zero) = NonZeroUsize::new(capacity)
    {
        config.channel_capacity = Some(non_zero);
    }

    Ok(ExecutionPlan {
        config,
        backend_locked: backend_override.is_some(),
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

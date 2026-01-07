use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{oneshot, watch};

use crate::backend::{self, ExecutionPlan};
use crate::cli::{CliArgs, CliSources};
use crate::gui::runtime;
use crate::settings::{ConfigError, resolve_settings};
use crate::stage::PipelineConfig;
use subtitle_fast_decoder::Configuration;
use subtitle_fast_types::DecoderError;

pub mod controls;

pub use controls::DetectionControls;

static NEXT_PROGRESS_HANDLE: AtomicU64 = AtomicU64::new(1);

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
                video_path: Mutex::new(None),
                cancel_tx: Mutex::new(None),
            }),
        }
    }

    pub fn set_video_path(&self, path: Option<PathBuf>) {
        self.inner.set_video_path(path);
    }

    pub fn subscribe_state(&self) -> watch::Receiver<DetectionRunState> {
        self.inner.subscribe_state()
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
    video_path: Mutex<Option<PathBuf>>,
    cancel_tx: Mutex<Option<oneshot::Sender<()>>>,
}

impl DetectionPipelineInner {
    fn set_video_path(&self, path: Option<PathBuf>) {
        if let Ok(mut slot) = self.video_path.lock() {
            *slot = path;
        }
    }

    fn subscribe_state(&self) -> watch::Receiver<DetectionRunState> {
        self.state_rx.clone()
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

        let plan = match build_execution_plan(&path) {
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
}

async fn run_detection_task(
    inner: Arc<DetectionPipelineInner>,
    plan: ExecutionPlan,
    pause_rx: watch::Receiver<bool>,
    cancel_rx: oneshot::Receiver<()>,
) {
    let handle_id = NEXT_PROGRESS_HANDLE.fetch_add(1, Ordering::Relaxed);
    let result = tokio::select! {
        _ = cancel_rx => Ok(()),
        result = backend::run_with_progress(plan, handle_id, pause_rx) => result,
    };

    if let Err(err) = result {
        eprintln!("detection pipeline failed: {err}");
    }

    inner.finish();
}

fn build_execution_plan(input: &Path) -> Result<ExecutionPlan, DecoderError> {
    if !input.exists() {
        return Err(DecoderError::configuration(format!(
            "input file '{}' does not exist",
            input.display()
        )));
    }

    let cli = default_gui_cli_args();
    let sources = CliSources::default();
    let resolved = resolve_settings(&cli, &sources).map_err(map_config_error)?;
    let settings = resolved.settings;
    let pipeline = PipelineConfig::from_settings(&settings, input)?;

    let env_backend_present = std::env::var("SUBFAST_BACKEND").is_ok();
    let mut config = Configuration::from_env().unwrap_or_default();
    let backend_override = match settings.decoder.backend.as_deref() {
        Some(name) => Some(backend::parse_backend(name)?),
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

    Ok(ExecutionPlan {
        config,
        backend_locked,
        pipeline,
    })
}

fn default_gui_cli_args() -> CliArgs {
    CliArgs {
        backend: None,
        config: None,
        list_backends: false,
        detection_samples_per_second: 7,
        decoder_channel_capacity: None,
        detector_target: None,
        detector_delta: None,
        comparator: None,
        roi: None,
        output: None,
        input: None,
    }
}

fn map_config_error(err: ConfigError) -> DecoderError {
    DecoderError::configuration(err.to_string())
}

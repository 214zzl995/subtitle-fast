use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use futures_util::future::{AbortHandle, Abortable};
use subtitle_fast_decoder::{Backend, Configuration, YPlaneError};
use subtitle_fast_validator::subtitle_detection::{DEFAULT_DELTA, DEFAULT_TARGET, RoiConfig};

pub mod backend;
pub mod cli;
pub mod settings;
pub mod stage;

pub use stage::progress_gui::{
    GuiProgressCallbacks, GuiProgressError, GuiProgressUpdate, clear_global_gui_callbacks,
    progress_gui_init, progress_gui_shutdown, set_global_gui_callbacks,
};

use backend::ExecutionPlan;
use settings::{DecoderSettings, DetectionSettings, EffectiveSettings, OutputSettings};
use stage::PipelineConfig;

#[repr(C)]
pub struct GuiRunConfig {
    pub input_path: *const c_char,
    pub output_path: *const c_char,
    pub decoder_backend: *const c_char,
    pub detection_samples_per_second: u32,
    pub detector_target: u8,
    pub detector_delta: u8,
    pub roi_x: f32,
    pub roi_y: f32,
    pub roi_width: f32,
    pub roi_height: f32,
    /// 0 for disabled, non-zero for enabled
    pub roi_enabled: u8,
}

unsafe impl Send for GuiRunConfig {}
unsafe impl Sync for GuiRunConfig {}

#[repr(C)]
pub struct GuiRunResult {
    /// 0 indicates failure to start; otherwise a handle id usable for cancellation
    pub handle_id: u64,
    /// 0 for success, non-zero for error
    pub error_code: i32,
}

static NEXT_HANDLE_ID: AtomicU64 = AtomicU64::new(1);
static HANDLES: LazyLock<Mutex<HashMap<u64, AbortHandle>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[unsafe(no_mangle)]
pub extern "C" fn subtitle_fast_gui_start(config: *const GuiRunConfig) -> GuiRunResult {
    let cfg = if let Some(cfg) = unsafe { config.as_ref() } {
        cfg
    } else {
        return GuiRunResult {
            handle_id: 0,
            error_code: 1,
        };
    };

    match build_plan(cfg) {
        Ok(plan) => {
            let handle_id = NEXT_HANDLE_ID.fetch_add(1, Ordering::SeqCst);
            let (abort_handle, abort_reg) = AbortHandle::new_pair();
            if let Ok(mut map) = HANDLES.lock() {
                map.insert(handle_id, abort_handle);
            }

            std::thread::spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(_) => return,
                };
                let fut = Abortable::new(async move { backend::run(plan).await }, abort_reg);
                let _ = rt.block_on(fut);
                if let Ok(mut map) = HANDLES.lock() {
                    map.remove(&handle_id);
                }
            });

            GuiRunResult {
                handle_id,
                error_code: 0,
            }
        }
        Err(_) => GuiRunResult {
            handle_id: 0,
            error_code: 2,
        },
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn subtitle_fast_gui_cancel(handle_id: u64) -> i32 {
    if handle_id == 0 {
        return 1;
    }
    if let Ok(mut map) = HANDLES.lock() {
        if let Some(abort) = map.remove(&handle_id) {
            abort.abort();
            return 0;
        }
    }
    2
}

fn build_plan(cfg: &GuiRunConfig) -> Result<ExecutionPlan, YPlaneError> {
    let input = parse_path(cfg.input_path)
        .ok_or_else(|| YPlaneError::configuration("gui config missing input_path".to_string()))?;

    let output = parse_path(cfg.output_path);
    let backend = parse_backend(cfg.decoder_backend);
    let backend_locked = backend.is_some();

    let mut decoder_config = Configuration::default();
    decoder_config.input = Some(input.clone());
    if let Some(backend) = backend {
        decoder_config.backend = backend;
    }

    let detection = DetectionSettings {
        samples_per_second: if cfg.detection_samples_per_second == 0 {
            7
        } else {
            cfg.detection_samples_per_second
        },
        target: if cfg.detector_target == 0 {
            DEFAULT_TARGET
        } else {
            cfg.detector_target
        },
        delta: if cfg.detector_delta == 0 {
            DEFAULT_DELTA
        } else {
            cfg.detector_delta
        },
        comparator: None,
        roi: gui_roi(cfg),
    };

    let effective = EffectiveSettings {
        detection,
        decoder: DecoderSettings {
            backend: backend.map(|b| b.as_str().to_string()),
            channel_capacity: None,
        },
        output: OutputSettings { path: output },
    };

    let pipeline = PipelineConfig::from_settings(&effective, &input)?;

    Ok(ExecutionPlan {
        config: decoder_config,
        backend_locked,
        pipeline,
    })
}

fn gui_roi(cfg: &GuiRunConfig) -> Option<RoiConfig> {
    if cfg.roi_enabled == 0 {
        return None;
    }
    let clamp_unit = |v: f32| v.clamp(0.0, 1.0);
    let x = clamp_unit(cfg.roi_x);
    let y = clamp_unit(cfg.roi_y);
    let max_width = (1.0 - x).max(0.0);
    let max_height = (1.0 - y).max(0.0);
    let width = clamp_unit(cfg.roi_width).min(max_width);
    let height = clamp_unit(cfg.roi_height).min(max_height);
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some(RoiConfig {
        x,
        y,
        width,
        height,
    })
}

fn parse_path(ptr: *const c_char) -> Option<PathBuf> {
    if ptr.is_null() {
        return None;
    }
    let c_str = unsafe { CStr::from_ptr(ptr) };
    let s = c_str.to_str().ok()?;
    if s.trim().is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

fn parse_backend(ptr: *const c_char) -> Option<Backend> {
    if ptr.is_null() {
        return None;
    }
    let c_str = unsafe { CStr::from_ptr(ptr) };
    let s = c_str.to_str().ok()?;
    if s.trim().is_empty() {
        None
    } else {
        Backend::from_str(s).ok()
    }
}

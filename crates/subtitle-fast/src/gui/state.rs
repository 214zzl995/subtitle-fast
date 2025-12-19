use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::backend::ExecutionPlan;
use crate::gui::components::AnimatedPanelState;
use crate::gui::theme::AppTheme;
use crate::settings::EffectiveSettings;
use crate::stage::PipelineConfig;
use subtitle_fast_types::RoiConfig;

const LEFT_SIDEBAR_MIN_WIDTH: f32 = 150.0;
const LEFT_SIDEBAR_MAX_WIDTH: f32 = 400.0;
const LEFT_SIDEBAR_DEFAULT_WIDTH: f32 = 175.0;

const RIGHT_SIDEBAR_MIN_WIDTH: f32 = 200.0;
const RIGHT_SIDEBAR_MAX_WIDTH: f32 = 500.0;
const RIGHT_SIDEBAR_DEFAULT_WIDTH: f32 = 280.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FileStatus {
    Idle,
    Detecting,
    Paused,
    Completed,
    Failed,
    Canceled,
}

#[derive(Clone, Debug)]
pub struct TrackedFile {
    pub id: FileId,
    pub path: PathBuf,
    pub status: FileStatus,
    pub progress: f64,
}

#[derive(Clone, Debug)]
pub struct DetectionMetrics {
    pub fps: f64,
    pub det_ms: f64,
    pub ocr_ms: f64,
    pub cues: u64,
    pub merged: u64,
    pub ocr_empty: u64,
}

impl Default for DetectionMetrics {
    fn default() -> Self {
        Self {
            fps: 0.0,
            det_ms: 0.0,
            ocr_ms: 0.0,
            cues: 0,
            merged: 0,
            ocr_empty: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SubtitleCue {
    pub start_ms: f64,
    pub end_ms: f64,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct RoiSelection {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

pub struct AppState {
    files: RwLock<HashMap<FileId, TrackedFile>>,
    active_file_id: RwLock<Option<FileId>>,
    next_file_id: RwLock<u64>,

    threshold: RwLock<f64>,
    tolerance: RwLock<f64>,
    roi: RwLock<Option<RoiSelection>>,
    selection_visible: RwLock<bool>,
    highlight_enabled: RwLock<bool>,

    metrics: RwLock<DetectionMetrics>,
    subtitles: RwLock<Vec<SubtitleCue>>,

    error_message: RwLock<Option<String>>,
    playhead_ms: RwLock<f64>,
    duration_ms: RwLock<f64>,
    playing: RwLock<bool>,

    left_sidebar_panel: RwLock<AnimatedPanelState>,

    left_sidebar_width: RwLock<f32>,
    right_sidebar_width: RwLock<f32>,

    left_panel_resizing: RwLock<bool>,
    right_panel_resizing: RwLock<bool>,
    resize_start_x: RwLock<f32>,
    resize_start_width: RwLock<f32>,

    current_theme: RwLock<AppTheme>,
}

impl AppState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            files: RwLock::new(HashMap::new()),
            active_file_id: RwLock::new(None),
            next_file_id: RwLock::new(1),
            threshold: RwLock::new(230.0),
            tolerance: RwLock::new(20.0),
            roi: RwLock::new(Some(Self::default_roi())),
            selection_visible: RwLock::new(true),
            highlight_enabled: RwLock::new(false),
            metrics: RwLock::new(DetectionMetrics::default()),
            subtitles: RwLock::new(Vec::new()),
            error_message: RwLock::new(None),
            playhead_ms: RwLock::new(2000.0),
            duration_ms: RwLock::new(30000.0),
            playing: RwLock::new(false),
            left_sidebar_panel: RwLock::new(AnimatedPanelState::new()),
            left_sidebar_width: RwLock::new(LEFT_SIDEBAR_DEFAULT_WIDTH),
            right_sidebar_width: RwLock::new(RIGHT_SIDEBAR_DEFAULT_WIDTH),
            left_panel_resizing: RwLock::new(false),
            right_panel_resizing: RwLock::new(false),
            resize_start_x: RwLock::new(0.0),
            resize_start_width: RwLock::new(0.0),
            current_theme: RwLock::new(AppTheme::auto()),
        })
    }

    pub fn add_file(&self, path: PathBuf) -> FileId {
        let id = FileId(*self.next_file_id.read());
        *self.next_file_id.write() += 1;

        let file = TrackedFile {
            id,
            path,
            status: FileStatus::Idle,
            progress: 0.0,
        };

        self.files.write().insert(id, file);
        *self.active_file_id.write() = Some(id);
        id
    }

    pub fn remove_file(&self, id: FileId) {
        self.files.write().remove(&id);
        if *self.active_file_id.read() == Some(id) {
            *self.active_file_id.write() = None;
        }
    }

    pub fn get_files(&self) -> Vec<TrackedFile> {
        self.files.read().values().cloned().collect()
    }

    pub fn get_file(&self, id: FileId) -> Option<TrackedFile> {
        self.files.read().get(&id).cloned()
    }

    pub fn set_active_file(&self, id: FileId) {
        *self.active_file_id.write() = Some(id);
    }

    pub fn get_active_file_id(&self) -> Option<FileId> {
        *self.active_file_id.read()
    }

    pub fn get_active_file(&self) -> Option<TrackedFile> {
        let id = *self.active_file_id.read();
        id.and_then(|id| self.get_file(id))
    }

    pub fn update_file_status(&self, id: FileId, status: FileStatus) {
        if let Some(file) = self.files.write().get_mut(&id) {
            file.status = status;
        }
    }

    pub fn update_file_progress(&self, id: FileId, progress: f64) {
        if let Some(file) = self.files.write().get_mut(&id) {
            file.progress = progress;
        }
    }

    pub fn get_threshold(&self) -> f64 {
        *self.threshold.read()
    }

    pub fn set_threshold(&self, value: f64) {
        *self.threshold.write() = value;
    }

    pub fn get_tolerance(&self) -> f64 {
        *self.tolerance.read()
    }

    pub fn set_tolerance(&self, value: f64) {
        *self.tolerance.write() = value;
    }

    pub fn get_roi(&self) -> Option<RoiSelection> {
        self.roi.read().clone()
    }

    pub fn set_roi(&self, roi: Option<RoiSelection>) {
        *self.roi.write() = roi;
    }

    pub fn get_metrics(&self) -> DetectionMetrics {
        self.metrics.read().clone()
    }

    pub fn set_metrics(&self, metrics: DetectionMetrics) {
        *self.metrics.write() = metrics;
    }

    pub fn get_subtitles(&self) -> Vec<SubtitleCue> {
        self.subtitles.read().clone()
    }

    pub fn add_subtitle(&self, cue: SubtitleCue) {
        self.subtitles.write().push(cue);
    }

    pub fn clear_subtitles(&self) {
        self.subtitles.write().clear();
    }

    pub fn get_error_message(&self) -> Option<String> {
        self.error_message.read().clone()
    }

    pub fn set_error_message(&self, msg: Option<String>) {
        *self.error_message.write() = msg;
    }

    pub fn selection_visible(&self) -> bool {
        *self.selection_visible.read()
    }

    pub fn toggle_selection_visibility(&self) {
        let mut guard = self.selection_visible.write();
        *guard = !*guard;
    }

    pub fn highlight_enabled(&self) -> bool {
        *self.highlight_enabled.read()
    }

    pub fn toggle_highlight(&self) {
        let mut guard = self.highlight_enabled.write();
        *guard = !*guard;
    }

    pub fn playhead_ms(&self) -> f64 {
        *self.playhead_ms.read()
    }

    pub fn set_playhead_ms(&self, value: f64) {
        let duration = *self.duration_ms.read();
        let clamped = value.clamp(0.0, duration);
        *self.playhead_ms.write() = clamped;
    }

    pub fn duration_ms(&self) -> f64 {
        *self.duration_ms.read()
    }

    pub fn set_duration_ms(&self, value: f64) {
        *self.duration_ms.write() = value.max(1.0);
    }

    pub fn is_playing(&self) -> bool {
        *self.playing.read()
    }

    pub fn toggle_playing(&self) {
        let mut guard = self.playing.write();
        *guard = !*guard;
    }

    pub fn left_sidebar_panel_state(&self) -> AnimatedPanelState {
        *self.left_sidebar_panel.read()
    }

    pub fn sidebar_collapsed(&self) -> bool {
        self.left_sidebar_panel.read().is_collapsed()
    }

    pub fn toggle_sidebar(&self) {
        self.left_sidebar_panel.write().toggle();
    }

    pub fn open_sidebar(&self) {
        self.left_sidebar_panel.write().open();
    }

    pub fn close_sidebar(&self) {
        self.left_sidebar_panel.write().close();
    }

    pub fn set_sidebar_collapsed(&self, collapsed: bool) {
        self.left_sidebar_panel.write().set_collapsed(collapsed);
    }

    pub fn left_sidebar_width(&self) -> f32 {
        *self.left_sidebar_width.read()
    }

    pub fn set_left_sidebar_width(&self, width: f32) {
        let clamped = width.clamp(LEFT_SIDEBAR_MIN_WIDTH, LEFT_SIDEBAR_MAX_WIDTH);
        *self.left_sidebar_width.write() = clamped;
    }

    pub fn right_sidebar_width(&self) -> f32 {
        *self.right_sidebar_width.read()
    }

    pub fn set_right_sidebar_width(&self, width: f32) {
        let clamped = width.clamp(RIGHT_SIDEBAR_MIN_WIDTH, RIGHT_SIDEBAR_MAX_WIDTH);
        *self.right_sidebar_width.write() = clamped;
    }

    pub fn is_resizing_left(&self) -> bool {
        *self.left_panel_resizing.read()
    }

    pub fn is_resizing_right(&self) -> bool {
        *self.right_panel_resizing.read()
    }

    pub fn is_resizing(&self) -> bool {
        self.is_resizing_left() || self.is_resizing_right()
    }

    pub fn start_resize_left(&self, mouse_x: f32) {
        *self.left_panel_resizing.write() = true;
        *self.resize_start_x.write() = mouse_x;
        *self.resize_start_width.write() = self.left_sidebar_width();
    }

    pub fn start_resize_right(&self, mouse_x: f32) {
        *self.right_panel_resizing.write() = true;
        *self.resize_start_x.write() = mouse_x;
        *self.resize_start_width.write() = self.right_sidebar_width();
    }

    pub fn update_resize(&self, mouse_x: f32) -> bool {
        if self.is_resizing_left() {
            let delta = mouse_x - *self.resize_start_x.read();
            let new_width = *self.resize_start_width.read() + delta;
            self.set_left_sidebar_width(new_width);
            true
        } else if self.is_resizing_right() {
            let delta = *self.resize_start_x.read() - mouse_x;
            let new_width = *self.resize_start_width.read() + delta;
            self.set_right_sidebar_width(new_width);
            true
        } else {
            false
        }
    }

    pub fn finish_resize(&self) {
        *self.left_panel_resizing.write() = false;
        *self.right_panel_resizing.write() = false;
    }

    fn default_roi() -> RoiSelection {
        RoiSelection {
            x: 0.15,
            y: 0.75,
            width: 0.70,
            height: 0.25,
        }
    }

    pub fn build_execution_plan(&self, file: &TrackedFile) -> anyhow::Result<ExecutionPlan> {
        let input = file.path.clone();

        let mut output = input.clone();
        output.set_extension("srt");

        let roi_config = self.roi.read().clone().map(|r| RoiConfig {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        });

        let settings = EffectiveSettings {
            detection: crate::settings::DetectionSettings {
                samples_per_second: 7,
                target: self.threshold.read().clone() as u8,
                delta: self.tolerance.read().clone() as u8,
                comparator: None,
                roi: roi_config,
            },
            decoder: crate::settings::DecoderSettings {
                backend: None,
                channel_capacity: None,
            },
            output: crate::settings::OutputSettings { path: Some(output) },
        };

        let pipeline = PipelineConfig::from_settings(&settings, &input)
            .map_err(|e| anyhow::anyhow!("Failed to create pipeline: {}", e))?;

        let config = subtitle_fast_decoder::Configuration {
            input: Some(input),
            ..Default::default()
        };

        Ok(ExecutionPlan {
            config,
            backend_locked: false,
            pipeline,
        })
    }

    pub fn get_theme(&self) -> AppTheme {
        *self.current_theme.read()
    }

    pub fn set_theme(&self, theme: AppTheme) {
        *self.current_theme.write() = theme;
    }

    pub fn update_theme_from_system(&self) -> bool {
        let new_theme = AppTheme::auto();
        let current = *self.current_theme.read();

        if new_theme.is_dark != current.is_dark {
            *self.current_theme.write() = new_theme;
            true
        } else {
            false
        }
    }
}

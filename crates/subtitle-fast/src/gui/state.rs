use gpui::WindowAppearance;
use parking_lot::RwLock;

use crate::gui::components::AnimatedPanelState;
use crate::gui::theme::AppTheme;

const LEFT_SIDEBAR_MIN_WIDTH: f32 = 225.0;
const LEFT_SIDEBAR_MAX_WIDTH: f32 = 400.0;
const LEFT_SIDEBAR_DEFAULT_WIDTH: f32 = 225.0;

const RIGHT_SIDEBAR_MIN_WIDTH: f32 = 200.0;
const RIGHT_SIDEBAR_MAX_WIDTH: f32 = 500.0;
const RIGHT_SIDEBAR_DEFAULT_WIDTH: f32 = 280.0;

pub struct AppState {
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
    pub fn new() -> Self {
        Self {
            left_sidebar_panel: RwLock::new(AnimatedPanelState::new()),
            left_sidebar_width: RwLock::new(LEFT_SIDEBAR_DEFAULT_WIDTH),
            right_sidebar_width: RwLock::new(RIGHT_SIDEBAR_DEFAULT_WIDTH),
            left_panel_resizing: RwLock::new(false),
            right_panel_resizing: RwLock::new(false),
            resize_start_x: RwLock::new(0.0),
            resize_start_width: RwLock::new(0.0),
            current_theme: RwLock::new(AppTheme::dark()),
        }
    }

    pub fn left_sidebar_panel_state(&self) -> AnimatedPanelState {
        *self.left_sidebar_panel.read()
    }

    pub fn toggle_sidebar(&self) {
        self.left_sidebar_panel.write().toggle();
    }

    pub fn left_sidebar_width(&self) -> f32 {
        *self.left_sidebar_width.read()
    }

    pub fn set_left_sidebar_width(&self, width: f32) {
        *self.left_sidebar_width.write() =
            width.clamp(LEFT_SIDEBAR_MIN_WIDTH, LEFT_SIDEBAR_MAX_WIDTH);
    }

    pub fn right_sidebar_width(&self) -> f32 {
        *self.right_sidebar_width.read()
    }

    pub fn set_right_sidebar_width(&self, width: f32) {
        *self.right_sidebar_width.write() =
            width.clamp(RIGHT_SIDEBAR_MIN_WIDTH, RIGHT_SIDEBAR_MAX_WIDTH);
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

    pub fn get_theme(&self) -> AppTheme {
        *self.current_theme.read()
    }

    pub fn update_theme_from_window_appearance(&self, appearance: WindowAppearance) -> bool {
        let new_theme = AppTheme::from_window_appearance(appearance);
        let current = *self.current_theme.read();

        if new_theme.is_dark != current.is_dark {
            *self.current_theme.write() = new_theme;
            true
        } else {
            false
        }
    }
}

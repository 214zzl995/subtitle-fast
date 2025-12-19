use gpui::prelude::*;
use gpui::*;
use parking_lot::RwLock;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResizeEdge {
    Left,
    #[default]
    Right,
}

#[derive(Clone, Copy, Debug)]
pub struct ResizablePanelConfig {
    pub min_width: f32,
    pub max_width: f32,
    pub default_width: f32,
    pub resize_edge: ResizeEdge,
    pub handle_width: f32,
}

impl Default for ResizablePanelConfig {
    fn default() -> Self {
        Self {
            min_width: 150.0,
            max_width: 400.0,
            default_width: 200.0,
            resize_edge: ResizeEdge::Right,
            handle_width: 4.0,
        }
    }
}

impl ResizablePanelConfig {
    pub fn new(min_width: f32, max_width: f32, default_width: f32) -> Self {
        Self {
            min_width,
            max_width,
            default_width: default_width.clamp(min_width, max_width),
            ..Default::default()
        }
    }

    pub fn with_resize_edge(mut self, edge: ResizeEdge) -> Self {
        self.resize_edge = edge;
        self
    }

    pub fn with_handle_width(mut self, width: f32) -> Self {
        self.handle_width = width;
        self
    }
}

#[derive(Debug, Default)]
struct ResizeState {
    is_resizing: bool,
    start_x: f32,
    start_width: f32,
}

#[derive(Debug)]
pub struct ResizablePanelState {
    config: ResizablePanelConfig,
    width: RwLock<f32>,
    resize_state: RwLock<ResizeState>,
}

impl ResizablePanelState {
    pub fn new(config: ResizablePanelConfig) -> Arc<Self> {
        Arc::new(Self {
            width: RwLock::new(config.default_width),
            config,
            resize_state: RwLock::new(ResizeState::default()),
        })
    }

    pub fn width(&self) -> f32 {
        *self.width.read()
    }

    pub fn set_width(&self, width: f32) {
        let clamped = width.clamp(self.config.min_width, self.config.max_width);
        *self.width.write() = clamped;
    }

    pub fn config(&self) -> ResizablePanelConfig {
        self.config
    }

    pub fn is_resizing(&self) -> bool {
        self.resize_state.read().is_resizing
    }

    pub fn start_resize(&self, mouse_x: f32) {
        let mut state = self.resize_state.write();
        state.is_resizing = true;
        state.start_x = mouse_x;
        state.start_width = self.width();
    }

    pub fn update_resize(&self, mouse_x: f32) -> bool {
        let state = self.resize_state.read();
        if !state.is_resizing {
            return false;
        }

        let delta = match self.config.resize_edge {
            ResizeEdge::Right => mouse_x - state.start_x,
            ResizeEdge::Left => state.start_x - mouse_x,
        };

        let new_width = state.start_width + delta;
        let old_width = self.width();
        self.set_width(new_width);
        self.width() != old_width
    }

    pub fn finish_resize(&self) {
        self.resize_state.write().is_resizing = false;
    }

    pub fn reset_width(&self) {
        *self.width.write() = self.config.default_width;
    }
}

pub struct ResizablePanel {
    state: Arc<ResizablePanelState>,
    accent_color: Hsla,
}

impl ResizablePanel {
    pub fn new(state: Arc<ResizablePanelState>, accent_color: Hsla) -> Self {
        Self {
            state,
            accent_color,
        }
    }

    fn render_resize_handle(&self, cx: &mut Context<Self>) -> Div {
        let is_resizing = self.state.is_resizing();
        let config = self.state.config();
        let accent = self.accent_color;

        let mut handle = div()
            .absolute()
            .top_0()
            .h_full()
            .w(px(config.handle_width))
            .cursor(CursorStyle::ResizeLeftRight);

        handle = match config.resize_edge {
            ResizeEdge::Right => handle.right(px(-config.handle_width / 2.0)),
            ResizeEdge::Left => handle.left(px(-config.handle_width / 2.0)),
        };

        handle = handle
            .when(is_resizing, |d| d.bg(accent.opacity(0.5)))
            .when(!is_resizing, |d| d.hover(|s| s.bg(accent.opacity(0.3))));

        handle.on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                this.state.start_resize(f32::from(event.position.x));
                cx.notify();
            }),
        )
    }
}

impl Render for ResizablePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = self.state.width();

        div()
            .relative()
            .w(px(width))
            .h_full()
            .child(self.render_resize_handle(cx))
    }
}

pub fn resizable_panel_container(
    state: Arc<ResizablePanelState>,
    accent_color: Hsla,
    child: impl IntoElement,
) -> Div {
    let is_resizing = state.is_resizing();
    let config = state.config();
    let width = state.width();

    let mut handle = div()
        .absolute()
        .top_0()
        .h_full()
        .w(px(config.handle_width))
        .cursor(CursorStyle::ResizeLeftRight);

    handle = match config.resize_edge {
        ResizeEdge::Right => handle.right(px(-config.handle_width / 2.0)),
        ResizeEdge::Left => handle.left(px(-config.handle_width / 2.0)),
    };

    handle = handle
        .when(is_resizing, |d| d.bg(accent_color.opacity(0.5)))
        .when(!is_resizing, |d| {
            d.hover(|s| s.bg(accent_color.opacity(0.3)))
        });

    div()
        .relative()
        .w(px(width))
        .h_full()
        .child(child)
        .child(handle)
}

pub fn resize_handle(state: &ResizablePanelState, accent_color: Hsla) -> Div {
    let is_resizing = state.is_resizing();
    let config = state.config();

    let mut handle = div()
        .absolute()
        .top_0()
        .h_full()
        .w(px(config.handle_width))
        .cursor(CursorStyle::ResizeLeftRight);

    handle = match config.resize_edge {
        ResizeEdge::Right => handle.right(px(-config.handle_width / 2.0)),
        ResizeEdge::Left => handle.left(px(-config.handle_width / 2.0)),
    };

    handle
        .when(is_resizing, |d| d.bg(accent_color.opacity(0.5)))
        .when(!is_resizing, |d| {
            d.hover(|s| s.bg(accent_color.opacity(0.3)))
        })
}

use gpui::prelude::*;
use gpui::*;
use std::ops::ControlFlow;
use std::sync::Arc;

use crate::gui::components::*;
use crate::gui::frame_image::frame_to_image;
use crate::gui::icons::{Icon, icon_sm};
use crate::gui::state::{AppState, PreviewCommand, PreviewWorkerHandle};
use crate::gui::theme::AppTheme;
use subtitle_fast_decoder::Configuration;
use subtitle_fast_types::{PlaneFrame, RawFrameFormat};
use tokio_stream::StreamExt;

const SIDEBAR_TOGGLE_SIZE: f32 = 26.0;
const SIDEBAR_TOGGLE_PADDING_X: f32 = 10.0;
const SIDEBAR_TOGGLE_GAP: f32 = 20.0;
const TITLEBAR_HEIGHT_DEFAULT: f32 = 32.0;
const TITLEBAR_COLLAPSED_WIDTH_DEFAULT: f32 = 48.0;
const MACOS_TITLEBAR_HEIGHT: f32 = 40.0;
const MACOS_TRAFFIC_LIGHT_OFFSET_X: f32 = 12.0;
const MACOS_TRAFFIC_LIGHT_DIAMETER: f32 = 12.0;
const MACOS_TRAFFIC_LIGHT_GAP: f32 = 6.0;
const MACOS_TRAFFIC_LIGHT_OFFSET_Y: f32 =
    (MACOS_TITLEBAR_HEIGHT - MACOS_TRAFFIC_LIGHT_DIAMETER) / 2.0;

fn macos_titlebar_collapsed_width() -> f32 {
    let traffic_light_width = MACOS_TRAFFIC_LIGHT_DIAMETER * 3.0 + MACOS_TRAFFIC_LIGHT_GAP * 2.0;
    let traffic_light_end = MACOS_TRAFFIC_LIGHT_OFFSET_X + traffic_light_width;
    traffic_light_end + SIDEBAR_TOGGLE_GAP + SIDEBAR_TOGGLE_PADDING_X + SIDEBAR_TOGGLE_SIZE
}

pub struct SubtitleFastApp {
    state: Arc<AppState>,
}

impl SubtitleFastApp {
    pub fn new(_cx: &mut App) -> Self {
        let state = AppState::new();
        Self { state }
    }

    pub fn open_window(&self, cx: &mut App) -> WindowHandle<MainWindow> {
        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);

        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("subtitle-fast".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(
                            px(MACOS_TRAFFIC_LIGHT_OFFSET_X),
                            px(MACOS_TRAFFIC_LIGHT_OFFSET_Y),
                        )),
                    }),
                    window_min_size: Some(size(px(1150.0), px(720.0))),
                    ..Default::default()
                },
                |_, cx| cx.new(|_| MainWindow::new(Arc::clone(&self.state))),
            )
            .unwrap();

        window
    }
}

pub struct MainWindow {
    state: Arc<AppState>,
    appearance_subscription: Option<Subscription>,
    sidebar: Option<Entity<Sidebar>>,
    preview: Option<Entity<PreviewPanel>>,
    control_panel: Option<Entity<ControlPanel>>,
    status_panel: Option<Entity<StatusPanel>>,
    subtitle_list: Option<Entity<SubtitleList>>,
    theme_is_dark: Option<bool>,
}

impl MainWindow {
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            appearance_subscription: None,
            sidebar: None,
            preview: None,
            control_panel: None,
            status_panel: None,
            subtitle_list: None,
            theme_is_dark: None,
        }
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_theme_listener(window, cx);
        self.ensure_preview_worker(cx);

        let theme = self.state.get_theme();
        self.ensure_child_views(theme, cx);

        let sidebar_panel_state = self.state.left_sidebar_panel_state();
        let sidebar_collapsed = sidebar_panel_state.is_collapsed();
        let max_width = self.state.left_sidebar_width();
        let animation_duration = std::time::Duration::from_millis(200);
        let titlebar_height = if cfg!(target_os = "macos") {
            px(MACOS_TITLEBAR_HEIGHT)
        } else {
            px(TITLEBAR_HEIGHT_DEFAULT)
        };
        let titlebar_collapsed_width = if cfg!(target_os = "macos") {
            macos_titlebar_collapsed_width()
        } else {
            TITLEBAR_COLLAPSED_WIDTH_DEFAULT
        };

        let sidebar_config = AnimatedPanelConfig::new(max_width)
            .with_collapsed_width(0.0)
            .with_duration(animation_duration);

        let sidebar = self.sidebar.as_ref().expect("sidebar view missing").clone();
        let preview = self.preview.as_ref().expect("preview view missing").clone();
        let control_panel = self
            .control_panel
            .as_ref()
            .expect("control panel view missing")
            .clone();
        let status_panel = self
            .status_panel
            .as_ref()
            .expect("status panel view missing")
            .clone();
        let subtitle_list = self
            .subtitle_list
            .as_ref()
            .expect("subtitle list view missing")
            .clone();

        div()
            .relative()
            .flex()
            .flex_row()
            .w_full()
            .h_full()
            .bg(theme.background())
            .when(self.state.is_resizing(), |d| {
                d.cursor(CursorStyle::ResizeLeftRight)
            })
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.state.update_resize(f32::from(event.position.x)) {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.state.is_resizing() {
                        this.state.finish_resize();
                        cx.notify();
                    }
                }),
            )
            .child(animated_panel_container(
                sidebar_panel_state,
                sidebar_config,
                "left-sidebar",
                div()
                    .relative()
                    .w_full()
                    .h_full()
                    .child(
                        div()
                            .relative()
                            .w(px(max_width))
                            .h_full()
                            .flex()
                            .flex_col()
                            .bg(theme.surface())
                            .child(self.render_sidebar_titlebar(theme, titlebar_height))
                            .child(
                                div()
                                    .relative()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .w_full()
                                    .h_full()
                                    .child(sidebar),
                            )
                            .when(!sidebar_collapsed, |d| {
                                d.child(self.render_resize_handle_left(theme, cx))
                            }),
                    )
                    .when(!sidebar_collapsed, |d| {
                        d.child(
                            div()
                                .absolute()
                                .right(px(0.0))
                                .top_0()
                                .bottom_0()
                                .w(px(1.0))
                                .bg(theme.titlebar_border()),
                        )
                    }),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .h_full()
                    .child(self.render_controls_titlebar(
                        theme,
                        window,
                        cx,
                        titlebar_height,
                        sidebar_collapsed,
                        titlebar_collapsed_width,
                    ))
                    .child(
                        div()
                            .relative()
                            .flex()
                            .flex_1()
                            .gap(px(1.0))
                            .overflow_hidden()
                            .child(
                                div().relative().flex().flex_1().overflow_hidden().child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .h_full()
                                        .child(div().flex_1().child(preview))
                                        .child(div().child(control_panel)),
                                ),
                            )
                            .child(
                                div()
                                    .relative()
                                    .flex()
                                    .flex_col()
                                    .w(px(self.state.right_sidebar_width()))
                                    .h_full()
                                    .gap(px(1.0))
                                    .child(self.render_resize_handle_right(theme, cx))
                                    .child(div().child(status_panel))
                                    .child(div().flex_1().child(subtitle_list)),
                            ),
                    ),
            )
            .child(self.render_sidebar_toggle_overlay(
                theme,
                cx,
                sidebar_panel_state,
                max_width,
                titlebar_height,
                titlebar_collapsed_width,
                animation_duration,
            ))
    }
}

impl MainWindow {
    fn ensure_theme_listener(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.appearance_subscription.is_some() {
            return;
        }

        self.state
            .update_theme_from_window_appearance(window.appearance());

        let state = Arc::clone(&self.state);
        self.appearance_subscription =
            Some(cx.observe_window_appearance(window, move |_, window, cx| {
                if state.update_theme_from_window_appearance(window.appearance()) {
                    cx.notify();
                }
            }));
    }

    fn ensure_child_views(&mut self, theme: AppTheme, cx: &mut Context<Self>) {
        if self.sidebar.is_none() {
            self.sidebar = Some(cx.new(|_| Sidebar::new(Arc::clone(&self.state), theme)));
            self.preview = Some(cx.new(|_| PreviewPanel::new(Arc::clone(&self.state), theme)));
            self.control_panel =
                Some(cx.new(|_| ControlPanel::new(Arc::clone(&self.state), theme)));
            self.status_panel = Some(cx.new(|_| StatusPanel::new(Arc::clone(&self.state), theme)));
            self.subtitle_list =
                Some(cx.new(|_| SubtitleList::new(Arc::clone(&self.state), theme)));
            self.theme_is_dark = Some(theme.is_dark);
            return;
        }

        if self.theme_is_dark != Some(theme.is_dark) {
            if let Some(sidebar) = &self.sidebar {
                let _ = sidebar.update(cx, |view, cx| {
                    view.set_theme(theme);
                    cx.notify();
                });
            }
            if let Some(preview) = &self.preview {
                let _ = preview.update(cx, |view, cx| {
                    view.set_theme(theme);
                    cx.notify();
                });
            }
            if let Some(control_panel) = &self.control_panel {
                let _ = control_panel.update(cx, |view, cx| {
                    view.set_theme(theme);
                    cx.notify();
                });
            }
            if let Some(status_panel) = &self.status_panel {
                let _ = status_panel.update(cx, |view, cx| {
                    view.set_theme(theme);
                    cx.notify();
                });
            }
            if let Some(subtitle_list) = &self.subtitle_list {
                let _ = subtitle_list.update(cx, |view, cx| {
                    view.set_theme(theme);
                    cx.notify();
                });
            }
            self.theme_is_dark = Some(theme.is_dark);
        }
    }

    fn ensure_preview_worker(&mut self, cx: &mut Context<Self>) {
        let active = self.state.get_active_file();
        let existing = self.state.preview_worker();

        match (active, existing) {
            (None, Some(_)) => {
                self.state.set_preview_worker(None);
                self.state.reset_preview_state();
                cx.notify();
            }
            (Some(file), Some(worker)) if worker.file_id == file.id => {}
            (Some(file), _) => {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                let handle = PreviewWorkerHandle {
                    file_id: file.id,
                    sender: tx,
                };
                self.state.set_preview_worker(Some(handle));
                self.state.reset_preview_state();

                let state = Arc::clone(&self.state);
                cx.spawn(move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    let async_app = (*cx).clone();
                    async move {
                        run_preview_worker(state, file, rx, this, async_app).await;
                    }
                })
                .detach();
                cx.notify();
            }
            (None, None) => {}
        }
    }

    fn render_sidebar_titlebar(&self, theme: AppTheme, titlebar_height: Pixels) -> Div {
        div()
            .relative()
            .h(titlebar_height)
            .w_full()
            .bg(theme.titlebar_bg())
    }

    fn render_controls_titlebar(
        &self,
        theme: AppTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
        titlebar_height: Pixels,
        sidebar_collapsed: bool,
        titlebar_collapsed_width: f32,
    ) -> Div {
        #[cfg(not(target_os = "windows"))]
        let _ = window;

        #[cfg(target_os = "windows")]
        let controls = self.render_windows_controls(theme, window, cx);

        #[cfg(target_os = "macos")]
        let controls = self.render_macos_controls(theme);

        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let controls = self.render_linux_controls(theme, cx);

        let file_button_offset = if sidebar_collapsed {
            px(titlebar_collapsed_width)
        } else {
            px(12.0)
        };

        let controls_slot = div()
            .relative()
            .flex()
            .items_center()
            .justify_end()
            .flex_1()
            .h_full()
            .child(controls);

        let titlebar = div()
            .relative()
            .flex()
            .items_center()
            .h(titlebar_height)
            .w_full()
            .bg(theme.titlebar_bg());

        titlebar
            .child(
                self.render_file_picker_button(theme, cx)
                    .ml(file_button_offset),
            )
            .child(controls_slot)
            .child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .h(px(1.0))
                    .bg(theme.titlebar_border()),
            )
    }

    fn render_sidebar_toggle_overlay(
        &self,
        theme: AppTheme,
        cx: &mut Context<Self>,
        sidebar_panel_state: AnimatedPanelState,
        max_width: f32,
        titlebar_height: Pixels,
        titlebar_collapsed_width: f32,
        animation_duration: std::time::Duration,
    ) -> AnimationElement<Div> {
        let expanded_x = max_width - SIDEBAR_TOGGLE_PADDING_X - SIDEBAR_TOGGLE_SIZE;
        let collapsed_x = titlebar_collapsed_width - SIDEBAR_TOGGLE_PADDING_X - SIDEBAR_TOGGLE_SIZE;
        let (from, to) = if sidebar_panel_state.is_collapsed() {
            (expanded_x, collapsed_x)
        } else {
            (collapsed_x, expanded_x)
        };
        let animation = Animation::new(animation_duration).with_easing(ease_out_quint());
        let animation_id = sidebar_panel_state.animation_id("left-sidebar-toggle");

        div()
            .absolute()
            .top(px(0.0))
            .h(titlebar_height)
            .w(px(SIDEBAR_TOGGLE_SIZE))
            .flex()
            .items_center()
            .justify_center()
            .child(self.render_sidebar_toggle(theme, cx, sidebar_panel_state.is_collapsed()))
            .with_animation(animation_id, animation, move |this, t| {
                let x = from + (to - from) * t;
                this.left(px(x))
            })
    }

    #[cfg(target_os = "windows")]
    fn render_windows_controls(
        &self,
        theme: AppTheme,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let button_height = px(32.0);
        let button_width = px(46.0);
        let icon_color = theme.text_primary();
        let button_hover_color = theme.surface_hover();
        let close_hover_color = hsla(0.0, 0.72, 0.52, 1.0);

        let font_family: SharedString = "Segoe Fluent Icons".into();

        div()
            .id("windows-window-controls")
            .font_family(font_family)
            .flex()
            .flex_row()
            .justify_center()
            .content_stretch()
            .max_h(button_height)
            .min_h(button_height)
            .child(
                div()
                    .id("minimize")
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .occlude()
                    .w(button_width)
                    .h_full()
                    .text_size(px(10.0))
                    .text_color(icon_color)
                    .hover(|s| s.bg(button_hover_color))
                    .window_control_area(WindowControlArea::Min)
                    .child("\u{e921}"),
            )
            .child(
                div()
                    .id("maximize-or-restore")
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .occlude()
                    .w(button_width)
                    .h_full()
                    .text_size(px(10.0))
                    .text_color(icon_color)
                    .hover(|s| s.bg(button_hover_color))
                    .window_control_area(WindowControlArea::Max)
                    .child(if window.is_maximized() {
                        "\u{e923}"
                    } else {
                        "\u{e922}"
                    }),
            )
            .child(
                div()
                    .id("close")
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .occlude()
                    .w(button_width)
                    .h_full()
                    .text_size(px(10.0))
                    .text_color(icon_color)
                    .hover(|s| s.bg(close_hover_color).text_color(gpui::rgb(0xffffff)))
                    .window_control_area(WindowControlArea::Close)
                    .child("\u{e8bb}"),
            )
    }

    #[cfg(target_os = "macos")]
    fn render_macos_controls(&self, _theme: AppTheme) -> Div {
        div()
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    fn render_linux_controls(&self, theme: AppTheme, _cx: &mut Context<Self>) -> impl IntoElement {
        let button_size = px(28.0);
        let icon_size = px(16.0);
        let icon_color = theme.text_secondary();
        let hover_bg = theme.surface_hover();
        let close_hover_bg = hsla(0.0, 0.72, 0.52, 1.0);

        div()
            .flex()
            .gap(px(4.0))
            .px(px(8.0))
            .child(
                div()
                    .id("linux-minimize")
                    .size(button_size)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .window_control_area(WindowControlArea::Min)
                    .hover(|s| s.bg(hover_bg))
                    .child(
                        svg()
                            .size(icon_size)
                            .path("M 4,8 H 12")
                            .text_color(icon_color),
                    ),
            )
            .child(
                div()
                    .id("linux-maximize")
                    .size(button_size)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .window_control_area(WindowControlArea::Max)
                    .hover(|s| s.bg(hover_bg))
                    .child(
                        svg()
                            .size(icon_size)
                            .path("M 4,4 H 12 V 12 H 4 Z")
                            .text_color(icon_color),
                    ),
            )
            .child(
                div()
                    .id("linux-close")
                    .group("close")
                    .size(button_size)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .window_control_area(WindowControlArea::Close)
                    .hover(|s| s.bg(close_hover_bg))
                    .child(
                        svg()
                            .size(icon_size)
                            .path("M 4,4 L 12,12 M 12,4 L 4,12")
                            .text_color(icon_color)
                            .group_hover("close", |s| s.text_color(gpui::rgb(0xffffff))),
                    ),
            )
    }

    fn render_sidebar_toggle(
        &self,
        theme: AppTheme,
        cx: &mut Context<Self>,
        collapsed: bool,
    ) -> Div {
        div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(SIDEBAR_TOGGLE_SIZE))
            .h(px(SIDEBAR_TOGGLE_SIZE))
            .rounded(px(6.0))
            .bg(theme.surface_elevated())
            .border_1()
            .border_color(theme.border())
            .cursor_pointer()
            .hover(|s| s.bg(theme.surface_hover()))
            .child(icon_sm(
                if collapsed {
                    Icon::PanelLeftOpen
                } else {
                    Icon::PanelLeftClose
                },
                theme.text_secondary(),
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.state.toggle_sidebar();
                    cx.notify();
                }),
            )
    }

    fn render_file_picker_button(&self, theme: AppTheme, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(SIDEBAR_TOGGLE_SIZE))
            .h(px(SIDEBAR_TOGGLE_SIZE))
            .rounded(px(6.0))
            .bg(theme.surface_elevated())
            .border_1()
            .border_color(theme.border())
            .cursor_pointer()
            .hover(|s| s.bg(theme.surface_hover()))
            .child(icon_sm(Icon::Upload, theme.text_secondary()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.state.debug_event("file picker click");
                    let receiver = cx.prompt_for_paths(PathPromptOptions {
                        files: true,
                        directories: false,
                        multiple: true,
                        prompt: Some("Select video files".into()),
                    });
                    cx.spawn(move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                        let mut async_app = (*cx).clone();
                        async move {
                            let result = match receiver.await {
                                Ok(result) => result,
                                Err(_) => return,
                            };
                            match result {
                                Ok(Some(paths)) => {
                                    let _ = this.update(&mut async_app, |this, cx| {
                                        for path in paths {
                                            this.state.add_file(path);
                                        }
                                        this.state.set_error_message(None);
                                        cx.notify();
                                    });
                                }
                                Ok(None) => {}
                                Err(err) => {
                                    let _ = this.update(&mut async_app, |this, cx| {
                                        this.state.set_error_message(Some(format!(
                                            "Failed to open file picker: {err}"
                                        )));
                                        cx.notify();
                                    });
                                }
                            }
                        }
                    })
                    .detach();
                }),
            )
    }

    fn render_resize_handle_left(&self, theme: AppTheme, cx: &mut Context<Self>) -> Div {
        let is_resizing = self.state.is_resizing_left();

        div()
            .absolute()
            .right(px(-2.0))
            .top_0()
            .h_full()
            .w(px(4.0))
            .cursor(CursorStyle::ResizeLeftRight)
            .when(is_resizing, |d| d.bg(theme.accent().opacity(0.5)))
            .when(!is_resizing, |d| {
                d.hover(|s| s.bg(theme.accent().opacity(0.3)))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.state.start_resize_left(f32::from(event.position.x));
                    cx.notify();
                }),
            )
    }

    fn render_resize_handle_right(&self, theme: AppTheme, cx: &mut Context<Self>) -> Div {
        let is_resizing = self.state.is_resizing_right();

        div()
            .absolute()
            .left(px(-2.0))
            .top_0()
            .h_full()
            .w(px(4.0))
            .cursor(CursorStyle::ResizeLeftRight)
            .when(is_resizing, |d| d.bg(theme.accent().opacity(0.5)))
            .when(!is_resizing, |d| {
                d.hover(|s| s.bg(theme.accent().opacity(0.3)))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.state.start_resize_right(f32::from(event.position.x));
                    cx.notify();
                }),
            )
    }
}

async fn run_preview_worker(
    state: Arc<AppState>,
    file: crate::gui::state::TrackedFile,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<PreviewCommand>,
    window: WeakEntity<MainWindow>,
    async_app: AsyncApp,
) {
    run_preview_loop(&state, &file, &mut rx, &window, &async_app).await;
}

async fn frame_to_image_async(frame: PlaneFrame) -> Result<Arc<Image>, String> {
    let task = gpui::background_executor().spawn(async move { frame_to_image(&frame) });
    task.await.map_err(|err| err.to_string())
}

async fn run_preview_loop(
    state: &AppState,
    file: &crate::gui::state::TrackedFile,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<PreviewCommand>,
    window: &WeakEntity<MainWindow>,
    async_app: &AsyncApp,
) {
    let mut config = Configuration::default();
    config.input = Some(file.path.clone());
    let mut output_format = RawFrameFormat::NV12;
    let mut tried_luma_fallback = false;

    let needs_luma_fallback = |message: &str| {
        let message = message.to_ascii_lowercase();
        message.contains("nv12") && (message.contains("uv") || message.contains("buffer"))
    };

    'preview: loop {
        config.output_format = output_format;
        let provider = match config.create_provider() {
            Ok(provider) => provider,
            Err(err) => {
                if !tried_luma_fallback && output_format == RawFrameFormat::NV12 {
                    output_format = RawFrameFormat::Y;
                    tried_luma_fallback = true;
                    continue 'preview;
                }
                state.set_error_message(Some(format!("Failed to start preview decoder: {err}")));
                notify_ui(window, async_app);
                return;
            }
        };

        let total_frames = provider.total_frames();
        state.set_preview_total_frames(total_frames);
        notify_ui(window, async_app);

        let mut stream = provider.into_stream();
        let resume_ms = state.playhead_ms();
        if resume_ms.is_finite() && resume_ms > 0.0 {
            let _ = stream.seek_to_time(std::time::Duration::from_millis(resume_ms as u64));
        }

        let mut paused = !state.is_playing();
        let mut needs_frame = true;

        loop {
            tokio::select! {
                command = rx.recv() => {
                    match command {
                        Some(command) => {
                            let needs_seek_frame = matches!(command, PreviewCommand::SeekMs(_));
                            if let ControlFlow::Break(()) = handle_preview_command(
                                state,
                                window,
                                async_app,
                                &stream,
                                command,
                                &mut paused,
                            ) {
                                return;
                            }
                            if paused && needs_seek_frame {
                                needs_frame = true;
                            }
                        }
                        None => return,
                    }
                }
                frame = stream.next(), if !paused || needs_frame => {
                    match frame {
                        Some(Ok(frame)) => {
                            let height = frame.height();
                            if height > 0 {
                                let ratio = frame.width() as f32 / height as f32;
                                if ratio.is_finite() && ratio > 0.0 {
                                    state.set_preview_aspect_ratio(Some(ratio));
                                }
                            }

                            let frame_index = frame.frame_index();
                            let timestamp = frame.timestamp();
                            match frame_to_image_async(frame).await {
                                Ok(image) => {
                                    state.update_preview_frame(Some(image), frame_index, timestamp);
                                }
                                Err(message) => {
                                    if !tried_luma_fallback
                                        && output_format == RawFrameFormat::NV12
                                        && needs_luma_fallback(&message)
                                    {
                                        output_format = RawFrameFormat::Y;
                                        tried_luma_fallback = true;
                                        state.set_error_message(None);
                                        notify_ui(window, async_app);
                                        continue 'preview;
                                    }
                                    state.set_error_message(Some(format!("Preview frame error: {message}")));
                                }
                            }
                            needs_frame = false;
                            notify_ui(window, async_app);
                        }
                        Some(Err(err)) => {
                            let message = err.to_string();
                            if !tried_luma_fallback
                                && output_format == RawFrameFormat::NV12
                                && needs_luma_fallback(&message)
                            {
                                output_format = RawFrameFormat::Y;
                                tried_luma_fallback = true;
                                state.set_error_message(None);
                                notify_ui(window, async_app);
                                continue 'preview;
                            }
                            state.set_error_message(Some(format!("Preview decode error: {message}")));
                            notify_ui(window, async_app);
                            return;
                        }
                        None => return,
                    }
                }
            }
        }
    }
}

fn handle_preview_command(
    state: &AppState,
    window: &WeakEntity<MainWindow>,
    async_app: &AsyncApp,
    stream: &subtitle_fast_decoder::PlaneStreamHandle,
    command: PreviewCommand,
    paused: &mut bool,
) -> ControlFlow<()> {
    match command {
        PreviewCommand::Play => {
            *paused = false;
            state.set_playing(true);
            notify_ui(window, async_app);
        }
        PreviewCommand::Pause => {
            *paused = true;
            state.set_playing(false);
            notify_ui(window, async_app);
        }
        PreviewCommand::SeekMs(ms) => {
            if let Err(err) =
                stream.seek_to_time(std::time::Duration::from_millis(ms.max(0.0) as u64))
            {
                state.set_error_message(Some(format!("Preview seek error: {err}")));
                notify_ui(window, async_app);
            }
        }
    }
    ControlFlow::Continue(())
}

fn notify_ui(window: &WeakEntity<MainWindow>, async_app: &AsyncApp) {
    let mut async_app = async_app.clone();
    let _ = window.update(&mut async_app, |_, cx| {
        cx.notify();
    });
}

use gpui::prelude::*;
use gpui::*;
use std::sync::Arc;

use crate::gui::components::*;
use crate::gui::icons::{Icon, icon_sm};
use crate::gui::state::AppState;
use crate::gui::theme::AppTheme;

const SIDEBAR_TOGGLE_SIZE: f32 = 26.0;
const SIDEBAR_TOGGLE_PADDING_X: f32 = 10.0;
const SIDEBAR_TOGGLE_GAP: f32 = 16.0;
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
}

impl MainWindow {
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            appearance_subscription: None,
        }
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_theme_listener(window, cx);

        let theme = self.state.get_theme();

        let sidebar_panel_state = self.state.left_sidebar_panel_state();
        let sidebar_collapsed = sidebar_panel_state.is_collapsed();
        let max_width = self.state.left_sidebar_width();
        let animation_duration = std::time::Duration::from_millis(200);
        let titlebar_collapsed_width = if cfg!(target_os = "macos") {
            macos_titlebar_collapsed_width()
        } else {
            TITLEBAR_COLLAPSED_WIDTH_DEFAULT
        };

        let sidebar_config = AnimatedPanelConfig::new(max_width)
            .with_collapsed_width(0.0)
            .with_duration(animation_duration);
        let titlebar_sidebar_config = AnimatedPanelConfig::new(max_width)
            .with_collapsed_width(titlebar_collapsed_width)
            .with_duration(animation_duration);

        let sidebar = cx.new(|_| Sidebar::new(Arc::clone(&self.state), theme));
        let preview = cx.new(|_| PreviewPanel::new(Arc::clone(&self.state), theme));
        let control_panel = cx.new(|_| ControlPanel::new(Arc::clone(&self.state), theme));
        let status_panel = cx.new(|_| StatusPanel::new(Arc::clone(&self.state), theme));
        let subtitle_list = cx.new(|_| SubtitleList::new(Arc::clone(&self.state), theme));

        div()
            .flex()
            .flex_col()
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
            .child(self.render_titlebar(
                theme,
                window,
                cx,
                sidebar_panel_state,
                titlebar_sidebar_config,
            ))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .gap(px(1.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .relative()
                            .flex()
                            .flex_1()
                            .overflow_hidden()
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
                                            .bg(theme.surface())
                                            .child(sidebar)
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
                                    .gap(px(1.0))
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
            )
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

    fn render_titlebar(
        &self,
        theme: AppTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
        sidebar_panel_state: AnimatedPanelState,
        sidebar_config: AnimatedPanelConfig,
    ) -> Div {
        let titlebar_height = if cfg!(target_os = "macos") {
            px(MACOS_TITLEBAR_HEIGHT)
        } else {
            px(TITLEBAR_HEIGHT_DEFAULT)
        };

        #[cfg(not(target_os = "windows"))]
        let _ = window;

        #[cfg(target_os = "windows")]
        let controls = self.render_windows_controls(theme, window, cx);

        #[cfg(target_os = "macos")]
        let controls = self.render_macos_controls(theme);

        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let controls = self.render_linux_controls(theme, cx);

        let sidebar_collapsed = sidebar_panel_state.is_collapsed();
        let sidebar_controls = div()
            .flex()
            .items_center()
            .justify_end()
            .h_full()
            .w_full()
            .px(px(SIDEBAR_TOGGLE_PADDING_X))
            .child(self.render_sidebar_toggle(theme, cx, sidebar_collapsed));

        let sidebar_slot = div()
            .relative()
            .h_full()
            .bg(theme.titlebar_bg())
            .child(sidebar_controls)
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
            })
            .when(sidebar_collapsed, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .h(px(1.0))
                        .bg(theme.titlebar_border()),
                )
            })
            .overflow_hidden()
            .with_animated_width(sidebar_panel_state, sidebar_config, "left-titlebar");

        let controls_slot = div()
            .relative()
            .flex()
            .items_center()
            .justify_end()
            .flex_1()
            .h_full()
            .child(controls)
            .child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .h(px(1.0))
                    .bg(theme.titlebar_border()),
            );

        div()
            .flex()
            .items_center()
            .h(titlebar_height)
            .w_full()
            .bg(theme.titlebar_bg())
            .window_control_area(WindowControlArea::Drag)
            .child(sidebar_slot)
            .child(controls_slot)
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
                    Icon::ChevronRight
                } else {
                    Icon::ChevronLeft
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

use gpui::prelude::*;
use gpui::*;
use std::sync::Arc;
use std::time::Duration;

use crate::gui::components::*;
use crate::gui::icons::{Icon, icon_sm};
use crate::gui::state::AppState;
use crate::gui::theme::AppTheme;

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
                        traffic_light_position: Some(point(px(12.0), px(12.0))),
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
}

impl MainWindow {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {

        let theme = self.state.get_theme();

        let sidebar_panel_state = self.state.left_sidebar_panel_state();
        let sidebar_collapsed = sidebar_panel_state.is_collapsed();
        let max_width = self.state.left_sidebar_width();

        let sidebar_config = AnimatedPanelConfig::new(max_width)
            .with_collapsed_width(0.0)
            .with_duration(std::time::Duration::from_millis(200));

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

            .child(self.render_titlebar(theme, window, cx))

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
            .child(
                div()
                    .flex()
                    .flex_1()
                    .gap(px(1.0))
                    .overflow_hidden()

                    .child(
                        div()
                            .flex()
                            .h_full()
                            .child(

                                self.render_sidebar_toggle(theme, cx, sidebar_collapsed),
                            )
                            .child(

                                animated_panel_container(
                                    sidebar_panel_state,
                                    sidebar_config,
                                    "left-sidebar",

                                    div()
                                        .w(px(max_width))
                                        .h_full()
                                        .child(sidebar)
                                        .when(!sidebar_collapsed, |d| {
                                            d.child(self.render_resize_handle_left(theme, cx))
                                        }),
                                ),
                            ),
                    )

                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .h_full()
                            .gap(px(1.0))
                            .child(div().flex_1().child(preview))
                            .child(div().child(control_panel)),
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
    fn render_titlebar(&self, theme: AppTheme, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let titlebar_height = px(38.0);

        #[cfg(target_os = "windows")]
        let controls = self.render_windows_controls(theme, window, cx);

        #[cfg(target_os = "macos")]
        let controls = self.render_macos_controls(theme);

        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let controls = self.render_linux_controls(theme, cx);

        #[cfg(target_os = "macos")]
        let left_padding = px(80.0);

        #[cfg(not(target_os = "macos"))]
        let left_padding = px(12.0);

        div()
            .flex()
            .items_center()
            .justify_between()
            .h(titlebar_height)
            .w_full()
            .bg(theme.titlebar_bg())
            .window_control_area(WindowControlArea::Drag)
            .child(

                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .pl(left_padding)
                    .child(

                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(20.0))
                            .h(px(20.0))
                            .rounded(px(4.0))
                            .bg(theme.surface_elevated())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_secondary())
                                    .child("⌘"),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_primary())
                            .child("subtitle-fast"),
                    ),
            )
            .child(controls)
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
        let icon = if collapsed {
            Icon::ChevronRight
        } else {
            Icon::ChevronLeft
        };

        div()
            .flex()
            .flex_col()
            .h_full()
            .w(px(24.0))
            .bg(theme.surface())
            .justify_center()
            .items_center()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(20.0))
                    .h(px(20.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_hover()))
                    .child(icon_sm(icon, theme.text_secondary()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.state.toggle_sidebar();
                            cx.notify();
                        }),
                    ),
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

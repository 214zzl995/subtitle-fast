use gpui::prelude::*;
use gpui::*;
use std::time::{Duration, Instant};

use crate::gui::components::*;
use crate::gui::icons::{Icon, icon_sm};
use crate::gui::state::AppState;
use crate::gui::theme::AppTheme;

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
    state: Entity<AppState>,
}

impl SubtitleFastApp {
    pub fn new(cx: &mut App) -> Self {
        let state = cx.new(|_| AppState::new());
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
                |_, cx| cx.new(|_| MainWindow::new(self.state.clone())),
            )
            .unwrap();

        window
    }
}

pub struct MainWindow {
    state: Entity<AppState>,
    appearance_subscription: Option<Subscription>,
    sidebar: Option<Entity<Sidebar>>,
    preview: Option<Entity<PreviewPanel>>,
    control_panel: Option<Entity<ControlPanel>>,
    status_panel: Option<Entity<StatusPanel>>,
    subtitle_list: Option<Entity<SubtitleList>>,
    playback_loop_started: bool,
    last_active_file_id: Option<crate::gui::state::FileId>,
}

impl MainWindow {
    pub fn new(state: Entity<AppState>) -> Self {
        Self {
            state,
            appearance_subscription: None,
            sidebar: None,
            preview: None,
            control_panel: None,
            status_panel: None,
            subtitle_list: None,
            playback_loop_started: false,
            last_active_file_id: None,
        }
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_theme_listener(window, cx);
        self.ensure_children(cx);
        self.ensure_playback_loop(cx);

        let current_active_file_id = self.state.read(cx).get_active_file_id();
        if self.last_active_file_id != current_active_file_id {
            self.last_active_file_id = current_active_file_id;
            if current_active_file_id.is_some() {
                if let Some(control_panel) = &self.control_panel {
                    control_panel.update(cx, |panel, cx| {
                        panel.init_decoder(cx);
                    });
                }
            }
        }

        let (
            theme,
            sidebar_panel_state,
            sidebar_collapsed,
            max_width,
            right_sidebar_width,
            is_resizing,
        ) = {
            let state = self.state.read(cx);
            let theme = state.get_theme();
            let sidebar_panel_state = state.left_sidebar_panel_state();
            let sidebar_collapsed = sidebar_panel_state.is_collapsed();
            let max_width = state.left_sidebar_width();
            let right_sidebar_width = state.right_sidebar_width();
            let is_resizing = state.is_resizing();
            (
                theme,
                sidebar_panel_state,
                sidebar_collapsed,
                max_width,
                right_sidebar_width,
                is_resizing,
            )
        };
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

        let full_size_style = StyleRefinement::default().w_full().h_full();
        let sidebar =
            AnyView::from(self.sidebar.as_ref().unwrap().clone()).cached(full_size_style.clone());
        let preview =
            AnyView::from(self.preview.as_ref().unwrap().clone()).cached(full_size_style.clone());
        let control_panel = AnyView::from(self.control_panel.as_ref().unwrap().clone());
        let status_panel = AnyView::from(self.status_panel.as_ref().unwrap().clone());
        let subtitle_list =
            AnyView::from(self.subtitle_list.as_ref().unwrap().clone()).cached(full_size_style);

        div()
            .relative()
            .flex()
            .flex_row()
            .w_full()
            .h_full()
            .bg(theme.background())
            .when(is_resizing, |d| d.cursor(CursorStyle::ResizeLeftRight))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                let mouse_x = f32::from(event.position.x);
                let state = this.state.clone();
                let changed = state.update(cx, |state, state_cx| {
                    if state.update_resize(mouse_x) {
                        state_cx.notify();
                        return true;
                    }
                    false
                });
                if changed {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    let state = this.state.clone();
                    let changed = state.update(cx, |state, state_cx| {
                        if state.is_resizing() {
                            state.finish_resize();
                            state_cx.notify();
                            return true;
                        }
                        false
                    });
                    if changed {
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
                                    .w(px(right_sidebar_width))
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

        let appearance = window.appearance();
        let state = self.state.clone();
        let changed = state.update(cx, |state, state_cx| {
            if state.update_theme_from_window_appearance(appearance) {
                state_cx.notify();
                return true;
            }
            false
        });
        if changed {
            cx.notify();
        }

        let this = cx.weak_entity();
        self.appearance_subscription =
            Some(cx.observe_window_appearance(window, move |_, window, cx| {
                let appearance = window.appearance();
                let updated = state.update(cx, |state, state_cx| {
                    if state.update_theme_from_window_appearance(appearance) {
                        state_cx.notify();
                        return true;
                    }
                    false
                });
                if updated {
                    let _ = this.update(cx, |_, cx| {
                        cx.notify();
                    });
                }
            }));
    }

    fn ensure_children(&mut self, cx: &mut Context<Self>) {
        if self.sidebar.is_none() {
            let state = self.state.clone();
            self.sidebar = Some(cx.new(|_| Sidebar::new(state)));
        }
        if self.preview.is_none() {
            let state = self.state.clone();
            self.preview = Some(cx.new(|_| PreviewPanel::new(state)));
        }
        if self.control_panel.is_none() {
            let state = self.state.clone();
            self.control_panel = Some(cx.new(|_| ControlPanel::new(state)));
        }
        if self.status_panel.is_none() {
            let state = self.state.clone();
            self.status_panel = Some(cx.new(|_| StatusPanel::new(state)));
        }
        if self.subtitle_list.is_none() {
            let state = self.state.clone();
            self.subtitle_list = Some(cx.new(|_| SubtitleList::new(state)));
        }
    }

    fn ensure_playback_loop(&mut self, cx: &mut Context<Self>) {
        if self.playback_loop_started {
            return;
        }
        self.playback_loop_started = true;
        let state = self.state.clone();
        cx.spawn(|_this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut async_app = (*cx).clone();
            async move {
                let mut last_tick = Instant::now();
                loop {
                    Timer::after(Duration::from_millis(16)).await;
                    let now = Instant::now();
                    let delta_ms = (now - last_tick).as_secs_f64() * 1000.0;
                    last_tick = now;
                    let updated = state.update(&mut async_app, |state, cx| {
                        let is_playing = state.is_playing();
                        if is_playing {
                            let next_time = state.playhead_ms() + delta_ms;
                            let duration = state.duration_ms();
                            if !state.playback_is_decoding() && next_time >= duration {
                                state.set_playhead_ms(duration);
                                state.set_playing(false);
                            } else if state.playback_is_decoding() {
                                state.set_playhead_ms_unclamped(next_time);
                            } else {
                                state.set_playhead_ms(next_time);
                            }
                        }
                        let advanced = state.advance_playback();
                        if is_playing || advanced {
                            cx.notify();
                        }
                    });
                    if updated.is_err() {
                        break;
                    }
                }
            }
        })
        .detach();
    }

    fn render_sidebar_titlebar(&self, theme: AppTheme, titlebar_height: Pixels) -> Div {
        div()
            .relative()
            .h(titlebar_height)
            .w_full()
            .bg(theme.titlebar_bg())
            .window_control_area(WindowControlArea::Drag)
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
            .bg(theme.titlebar_bg())
            .window_control_area(WindowControlArea::Drag);

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
                    let state = this.state.clone();
                    state.update(cx, |state, state_cx| {
                        state.toggle_sidebar();
                        state_cx.notify();
                    });
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
                cx.listener(|_this, _, _, cx| {
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
                                        let state = this.state.clone();
                                        state.update(cx, |state, cx| {
                                            for path in paths {
                                                state.add_file(path);
                                            }
                                            state.set_error_message(None);
                                            cx.notify();
                                        });
                                        if let Some(control_panel) = &this.control_panel {
                                            control_panel.update(cx, |panel, cx| {
                                                panel.init_decoder(cx);
                                            });
                                        }
                                    });
                                }
                                Ok(None) => {}
                                Err(err) => {
                                    let _ = this.update(&mut async_app, |this, cx| {
                                        let state = this.state.clone();
                                        state.update(cx, |state, cx| {
                                            state.set_error_message(Some(format!(
                                                "Failed to open file picker: {err}"
                                            )));
                                            cx.notify();
                                        });
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
        let is_resizing = self.state.read(cx).is_resizing_left();

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
                    let mouse_x = f32::from(event.position.x);
                    let state = this.state.clone();
                    state.update(cx, |state, state_cx| {
                        state.start_resize_left(mouse_x);
                        state_cx.notify();
                    });
                    cx.notify();
                }),
            )
    }

    fn render_resize_handle_right(&self, theme: AppTheme, cx: &mut Context<Self>) -> Div {
        let is_resizing = self.state.read(cx).is_resizing_right();

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
                    let mouse_x = f32::from(event.position.x);
                    let state = this.state.clone();
                    state.update(cx, |state, state_cx| {
                        state.start_resize_right(mouse_x);
                        state_cx.notify();
                    });
                    cx.notify();
                }),
            )
    }
}

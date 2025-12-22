use crate::gui::icons::{Icon, icon_sm};
use crate::gui::state::AppState;
use crate::gui::theme::AppTheme;
use gpui::prelude::*;
use gpui::*;
use gpui::{InteractiveElement, MouseButton};
use std::sync::Arc;

pub struct ControlPanel {
    state: Arc<AppState>,
    theme: AppTheme,
}

impl ControlPanel {
    pub fn new(state: Arc<AppState>, theme: AppTheme) -> Self {
        Self { state, theme }
    }
}

impl Render for ControlPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let threshold = self.state.get_threshold();
        let tolerance = self.state.get_tolerance();
        let roi = self
            .state
            .get_roi()
            .unwrap_or(crate::gui::state::RoiSelection {
                x: 0.15,
                y: 0.75,
                width: 0.70,
                height: 0.25,
            });
        let playhead = self.state.playhead_ms();
        let duration = self.state.duration_ms();
        let playing = self.state.is_playing();
        let highlight = self.state.highlight_enabled();

        div()
            .flex()
            .flex_col()
            .w_full()
            .bg(self.theme.surface())
            .px(px(10.0))
            .pt(px(6.0))
            .pb(px(24.0))
            .gap(px(12.0))
            .child(self.render_playback_bar(cx, playhead, duration, playing))
            .child(self.render_slider(
                cx,
                Icon::Sun,
                "Brightness Threshold",
                threshold,
                0.0,
                255.0,
                5.0,
                |state, value| state.set_threshold(value),
            ))
            .child(self.render_slider(
                cx,
                Icon::Gauge,
                "Tolerance",
                tolerance,
                0.0,
                50.0,
                2.0,
                |state, value| state.set_tolerance(value),
            ))
            .child(self.render_selection_section(cx, highlight))
            .child(self.render_selection_info(roi))
    }
}

impl ControlPanel {
    fn render_playback_bar(
        &self,
        cx: &mut Context<Self>,
        playhead: f64,
        duration: f64,
        playing: bool,
    ) -> Div {
        let progress = if duration > 0.0 {
            (playhead / duration).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let total_frames = 214918u32;
        let current_frame = (progress * total_frames as f64).round() as u32;

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .bg(self.theme.surface_elevated())
            .border_1()
            .border_color(self.theme.border())
            .rounded(px(10.0))
            .px(px(10.0))
            .py(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(30.0))
                            .h(px(30.0))
                            .rounded_full()
                            .bg(self.theme.surface_active())
                            .border_1()
                            .border_color(self.theme.border())
                            .cursor_pointer()
                            .hover(|s| s.bg(self.theme.surface_hover()))
                            .child(icon_sm(
                                if playing { Icon::Pause } else { Icon::Play },
                                self.theme.text_primary(),
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.state.toggle_playing();
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(self.render_progress_bar(cx, progress, duration).flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child(format!(
                                "{} / {}",
                                self.format_time(playhead),
                                self.format_time(duration)
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_tertiary())
                            .child(format!("Frame {}/{}", current_frame, total_frames)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(self.jump_button(cx, "-1f", -33.0))
                            .child(self.jump_button(cx, "-7f", -233.0))
                            .child(self.jump_button(cx, "-7s", -7000.0))
                            .child(self.jump_button(cx, "+7s", 7000.0)),
                    ),
            )
    }

    fn render_progress_bar(&self, cx: &mut Context<Self>, progress: f64, _duration: f64) -> Div {
        let _state = Arc::clone(&self.state);

        div()
            .relative()
            .w_full()
            .h(px(12.0))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                    let click_ratio = 0.5;
                    let new_time = click_ratio * this.state.duration_ms();
                    this.state.set_playhead_ms(new_time);
                    cx.notify();
                }),
            )
            .child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .right(px(0.0))
                    .top(px(4.0))
                    .h(px(4.0))
                    .rounded_full()
                    .bg(self.theme.border().opacity(0.6)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(4.0))
                    .h(px(4.0))
                    .rounded_full()
                    .bg(self.theme.accent())
                    .w(relative(progress as f32)),
            )
            .child(
                div()
                    .absolute()
                    .top(px(1.0))
                    .left(relative(progress as f32))
                    .ml(px(-5.0))
                    .w(px(10.0))
                    .h(px(10.0))
                    .rounded_full()
                    .bg(self.theme.surface())
                    .border_2()
                    .border_color(self.theme.accent())
                    .shadow_sm()
                    .hover(|s| s.top(px(0.0)).ml(px(-6.0)).w(px(12.0)).h(px(12.0))),
            )
    }

    fn jump_button(&self, cx: &mut Context<Self>, label: &str, delta_ms: f64) -> Div {
        div()
            .px(px(8.0))
            .py(px(3.0))
            .rounded_full()
            .bg(self.theme.surface_elevated())
            .border_1()
            .border_color(self.theme.border())
            .text_xs()
            .text_color(self.theme.text_secondary())
            .cursor_pointer()
            .hover(|s| s.bg(self.theme.surface_hover()))
            .child(label.to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    let current = this.state.playhead_ms();
                    this.state.set_playhead_ms(current + delta_ms);
                    cx.notify();
                }),
            )
    }

    fn render_selection_section(&self, cx: &mut Context<Self>, highlight_enabled: bool) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(icon_sm(Icon::MousePointer, self.theme.text_secondary()))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(self.theme.text_primary())
                            .child("Selection Overview"),
                    ),
            )
            .child(self.selection_item(
                cx,
                Icon::Sun,
                "Brightness Threshold",
                highlight_enabled,
                |state| {
                    state.toggle_highlight();
                },
            ))
    }

    fn render_selection_info(&self, roi: crate::gui::state::RoiSelection) -> Div {
        div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(icon_sm(Icon::Crosshair, self.theme.text_secondary()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child("Region"),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.text_secondary())
                    .child(format!(
                        "x{:.2} y{:.2} w{:.2} h{:.2}",
                        roi.x, roi.y, roi.width, roi.height
                    )),
            )
    }

    fn selection_item(
        &self,
        cx: &mut Context<Self>,
        icon: Icon,
        label: &str,
        active: bool,
        toggle: impl Fn(&AppState) + 'static,
    ) -> Div {
        let icon_color = if active {
            self.theme.accent()
        } else {
            self.theme.text_secondary()
        };
        let text_color = if active {
            self.theme.text_primary()
        } else {
            self.theme.text_secondary()
        };

        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .child(icon_sm(icon, icon_color))
            .child(
                div()
                    .text_xs()
                    .text_color(text_color)
                    .child(label.to_string()),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    toggle(&this.state);
                    cx.notify();
                }),
            )
    }

    fn render_slider(
        &self,
        _cx: &mut Context<Self>,
        icon: Icon,
        label: &str,
        value: f64,
        min: f64,
        max: f64,
        _step: f64,
        _update: fn(&AppState, f64),
    ) -> Div {
        let ratio = ((value - min) / (max - min)).clamp(0.0, 1.0) as f32;

        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .w(px(150.0))
                    .child(icon_sm(icon, self.theme.text_secondary()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child(label.to_string()),
                    ),
            )
            .child(
                div()
                    .w(px(36.0))
                    .text_right()
                    .text_xs()
                    .text_color(self.theme.text_secondary())
                    .child(format!("{:.0}", value)),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .h(px(12.0))
                    .child(
                        div()
                            .absolute()
                            .left(px(0.0))
                            .right(px(0.0))
                            .top(px(4.0))
                            .h(px(4.0))
                            .rounded_full()
                            .bg(self.theme.border().opacity(0.6)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(0.0))
                            .top(px(4.0))
                            .h(px(4.0))
                            .rounded_full()
                            .bg(self.theme.accent())
                            .w(relative(ratio)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(1.0))
                            .left(relative(ratio))
                            .ml(px(-5.0))
                            .w(px(10.0))
                            .h(px(10.0))
                            .rounded_full()
                            .bg(self.theme.surface())
                            .border_2()
                            .border_color(self.theme.accent())
                            .shadow_sm()
                            .hover(|s| s.top(px(0.0)).ml(px(-6.0)).w(px(12.0)).h(px(12.0))),
                    ),
            )
    }

    fn format_time(&self, ms: f64) -> String {
        let total_secs = (ms / 1000.0).round() as u64;
        let minutes = total_secs / 60;
        let seconds = total_secs % 60;
        format!("{:02}:{:02}", minutes, seconds)
    }
}

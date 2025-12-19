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
        let selection_visible = self.state.selection_visible();
        let highlight = self.state.highlight_enabled();

        div()
            .flex()
            .flex_col()
            .w_full()
            .bg(self.theme.surface())
            .p(px(12.0))
            .gap(px(12.0))
            .child(self.render_playback_bar(cx, playhead, duration, playing))
            .child(self.render_selection_section(cx, roi, selection_visible, highlight))
            .child(self.render_slider(
                cx,
                "Brightness Threshold",
                threshold,
                0.0,
                255.0,
                5.0,
                |state, value| state.set_threshold(value),
            ))
            .child(self.render_slider(
                cx,
                "Tolerance",
                tolerance,
                0.0,
                50.0,
                2.0,
                |state, value| state.set_tolerance(value),
            ))
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(32.0))
                            .h(px(32.0))
                            .rounded_full()
                            .bg(self.theme.accent())
                            .cursor_pointer()
                            .hover(|s| s.bg(self.theme.accent_hover()))
                            .child(icon_sm(
                                if playing { Icon::Pause } else { Icon::Play },
                                self.theme.background(),
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.state.toggle_playing();
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child(format!(
                                "{}  Frame {}/{}",
                                self.format_time(playhead),
                                current_frame,
                                total_frames
                            )),
                    ),
            )
            .child(self.render_progress_bar(cx, progress, duration))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(self.jump_button(cx, "-1f", -33.0))
                    .child(self.jump_button(cx, "-7f", -233.0))
                    .child(self.jump_button(cx, "-7s", -7000.0))
                    .child(self.jump_button(cx, "+7s", 7000.0))
                    .child(
                        div()
                            .flex_1()
                            .text_right()
                            .text_xs()
                            .text_color(self.theme.text_tertiary())
                            .child(format!(
                                "{} / {}",
                                self.format_time(playhead),
                                self.format_time(duration)
                            )),
                    ),
            )
    }

    fn render_progress_bar(&self, cx: &mut Context<Self>, progress: f64, _duration: f64) -> Div {
        let _state = Arc::clone(&self.state);

        div()
            .relative()
            .w_full()
            .h(px(8.0))
            .rounded_full()
            .bg(self.theme.border())
            .cursor_pointer()
            .overflow_hidden()
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
                    .h_full()
                    .rounded_full()
                    .bg(self.theme.accent())
                    .w(relative(progress as f32)),
            )
            .child(
                div()
                    .absolute()
                    .top(px(-2.0))
                    .left(relative(progress as f32))
                    .ml(px(-6.0))
                    .w(px(12.0))
                    .h(px(12.0))
                    .rounded_full()
                    .bg(self.theme.accent())
                    .border_2()
                    .border_color(self.theme.background())
                    .shadow_sm(),
            )
    }

    fn jump_button(&self, cx: &mut Context<Self>, label: &str, delta_ms: f64) -> Div {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(4.0))
            .bg(self.theme.surface_elevated())
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

    fn render_selection_section(
        &self,
        cx: &mut Context<Self>,
        roi: crate::gui::state::RoiSelection,
        selection_visible: bool,
        highlight_enabled: bool,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(icon_sm(Icon::MousePointer, self.theme.text_secondary()))
                    .child(
                        div()
                            .text_sm()
                            .text_color(self.theme.text_primary())
                            .child("Selection Overview"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(self.checkbox(cx, "Region", selection_visible, |state| {
                        state.toggle_selection_visibility();
                    }))
                    .child(
                        self.checkbox(cx, "Brightness Threshold", highlight_enabled, |state| {
                            state.toggle_highlight();
                        }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child(format!(
                                "x{:.2} y{:.2} w{:.2} h{:.2}",
                                roi.x, roi.y, roi.width, roi.height
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_tertiary())
                            .child("Bottom 25% default selection"),
                    ),
            )
    }

    fn checkbox(
        &self,
        cx: &mut Context<Self>,
        label: &str,
        checked: bool,
        toggle: impl Fn(&AppState) + 'static,
    ) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(16.0))
                    .h(px(16.0))
                    .rounded(px(3.0))
                    .border_1()
                    .border_color(if checked {
                        self.theme.accent()
                    } else {
                        self.theme.border()
                    })
                    .bg(if checked {
                        self.theme.accent()
                    } else {
                        gpui::transparent_black()
                    })
                    .when(checked, |d| {
                        d.child(icon_sm(Icon::Check, self.theme.background()))
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.text_secondary())
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
        cx: &mut Context<Self>,
        label: &str,
        value: f64,
        min: f64,
        max: f64,
        step: f64,
        update: fn(&AppState, f64),
    ) -> Div {
        let ratio = ((value - min) / (max - min)).clamp(0.0, 1.0) as f32;

        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child(format!("{:.0}", value)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(self.slider_button(cx, "-", value, min, max, -step, update))
                    .child(
                        div()
                            .flex_1()
                            .h(px(6.0))
                            .rounded_full()
                            .bg(self.theme.border())
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(self.theme.accent())
                                    .w(relative(ratio)),
                            ),
                    )
                    .child(self.slider_button(cx, "+", value, min, max, step, update)),
            )
    }

    fn slider_button(
        &self,
        cx: &mut Context<Self>,
        label: &str,
        current: f64,
        min: f64,
        max: f64,
        delta: f64,
        update: fn(&AppState, f64),
    ) -> Div {
        div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(24.0))
            .h(px(24.0))
            .rounded(px(4.0))
            .bg(self.theme.surface_elevated())
            .text_xs()
            .text_color(self.theme.text_secondary())
            .cursor_pointer()
            .hover(|s| s.bg(self.theme.surface_hover()))
            .child(label.to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    let new_value = (current + delta).clamp(min, max);
                    update(&this.state, new_value);
                    cx.notify();
                }),
            )
    }

    fn format_time(&self, ms: f64) -> String {
        let total_secs = (ms / 1000.0).round() as u64;
        let minutes = total_secs / 60;
        let seconds = total_secs % 60;
        format!("{:02}:{:02}", minutes, seconds)
    }
}

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
            .border_1()
            .border_color(self.theme.border())
            .rounded_md()
            .p(px(14.0))
            .gap(px(12.0))
            .child(self.render_playback_bar(cx, playhead, duration, playing))
            .child(self.render_selection_row(cx, roi, selection_visible, highlight))
            .child(self.render_slider(
                cx,
                "亮度阈值",
                threshold,
                0.0,
                255.0,
                5.0,
                |state, value| state.set_threshold(value),
            ))
            .child(
                self.render_slider(cx, "容差", tolerance, 0.0, 50.0, 2.0, |state, value| {
                    state.set_tolerance(value)
                }),
            )
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

        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(self.play_button(cx, playing)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child(format!(
                                "{} · Frame {}/{}",
                                self.format_time(playhead),
                                (progress * 180.0).round() as u32,
                                180
                            )),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .h(px(10.0))
                    .rounded_full()
                    .bg(self.theme.border())
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(self.theme.accent())
                            .w(relative(progress as f32)),
                    ),
            )
            .child(div().flex().items_center().gap(px(6.0)).children(vec![
                        self.jump_button(cx, "-1s", -1000.0),
                        self.jump_button(cx, "-5s", -5000.0),
                        self.jump_button(cx, "+1s", 1000.0),
                        self.jump_button(cx, "+5s", 5000.0),
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(self.theme.text_tertiary())
                            .child(format!(
                                "{} / {}",
                                self.format_time(playhead),
                                self.format_time(duration)
                            )),
                    ]))
    }

    fn play_button(&self, cx: &mut Context<Self>, playing: bool) -> Div {
        let label = if playing { "暂停" } else { "播放" };
        div()
            .px(px(10.0))
            .py(px(6.0))
            .rounded_full()
            .bg(self.theme.accent())
            .text_xs()
            .text_color(self.theme.background())
            .child(label.to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.state.toggle_playing();
                    cx.notify();
                }),
            )
    }

    fn jump_button(&self, cx: &mut Context<Self>, label: &str, delta_ms: f64) -> Div {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded_md()
            .bg(self.theme.surface_elevated())
            .border_1()
            .border_color(self.theme.border())
            .text_xs()
            .text_color(self.theme.text_secondary())
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

    fn render_selection_row(
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
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .text_color(self.theme.text_primary())
                            .child("选区概览"),
                    )
                    .child(div().flex().items_center().gap(px(6.0)).children(vec![
                        self.toggle_chip(cx, "显示选区", selection_visible, |state| {
                            state.toggle_selection_visibility();
                        }),
                        self.toggle_chip(cx, "亮度高亮", highlight_enabled, |state| {
                            state.toggle_highlight();
                        }),
                    ])),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child(format!(
                                "x {:.2} y {:.2} w {:.2} h {:.2}",
                                roi.x, roi.y, roi.width, roi.height
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_tertiary())
                            .child("底部 25% 默认选区"),
                    ),
            )
    }

    fn toggle_chip(
        &self,
        cx: &mut Context<Self>,
        label: &str,
        active: bool,
        toggle: impl Fn(&AppState) + 'static,
    ) -> Div {
        div()
            .px(px(10.0))
            .py(px(6.0))
            .rounded_md()
            .bg(if active {
                self.theme.accent_muted()
            } else {
                self.theme.surface_elevated()
            })
            .border_1()
            .border_color(if active {
                self.theme.border_focused()
            } else {
                self.theme.border()
            })
            .text_xs()
            .text_color(if active {
                self.theme.text_primary()
            } else {
                self.theme.text_secondary()
            })
            .child(label.to_string())
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
        let dec_value = value;
        let dec = cx.listener(move |this, _, _, cx| {
            let current = dec_value - step;
            update(&this.state, current.max(min));
            cx.notify();
        });

        let inc_value = value;
        let inc = cx.listener(move |this, _, _, cx| {
            let current = inc_value + step;
            update(&this.state, current.min(max));
            cx.notify();
        });

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
                    .child(
                        div()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded_md()
                            .bg(self.theme.surface_elevated())
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child("-")
                            .on_mouse_down(MouseButton::Left, dec),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h(px(8.0))
                            .rounded_full()
                            .bg(self.theme.border())
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(self.theme.accent())
                                    .w(relative(ratio)),
                            ),
                    )
                    .child(
                        div()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded_md()
                            .bg(self.theme.surface_elevated())
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child("+")
                            .on_mouse_down(MouseButton::Left, inc),
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

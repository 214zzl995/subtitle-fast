use crate::gui::icons::{Icon, icon_sm};
use crate::gui::state::{AppState, FileStatus};
use crate::gui::theme::AppTheme;
use gpui::prelude::*;
use gpui::*;
use gpui::{InteractiveElement, MouseButton};
use gpui_component::Icon as IconComponent;

pub struct StatusPanel {
    state: Entity<AppState>,
    theme: AppTheme,
    state_subscription: Option<Subscription>,
}

impl StatusPanel {
    pub fn new(state: Entity<AppState>) -> Self {
        Self {
            state,
            theme: AppTheme::dark(),
            state_subscription: None,
        }
    }
}

impl Render for StatusPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_state_subscription(cx);
        let state = self.state.read(cx);
        self.theme = state.get_theme();

        let metrics = state.get_metrics();
        let active_file = state.get_active_file();
        let error = state.get_error_message();

        let status = active_file
            .as_ref()
            .map(|f| f.status)
            .unwrap_or(FileStatus::Idle);
        let progress = active_file.as_ref().map(|f| f.progress).unwrap_or(0.0);

        div()
            .flex()
            .flex_col()
            .w_full()
            .bg(self.theme.surface())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(12.0))
                    .py(px(10.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(icon_sm(Icon::Gauge, self.theme.text_secondary()))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(self.theme.text_primary())
                                    .child("Detection Progress"),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.status_color(status))
                            .child(self.status_text(status)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .p(px(12.0))
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .w_full()
                                    .h(px(6.0))
                                    .rounded_full()
                                    .bg(self.theme.border())
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded_full()
                                            .bg(self.theme.accent())
                                            .w(relative(progress as f32)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(self.theme.text_tertiary())
                                    .child(format!("{:.0}%", progress * 100.0)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(self.metric_row(
                                Icon::Gauge,
                                "FPS",
                                format!("{:.1}", metrics.fps),
                                Some("fps"),
                            ))
                            .child(self.metric_row(
                                Icon::Activity,
                                "Detection",
                                format!("{:.1}", metrics.det_ms),
                                Some("ms"),
                            ))
                            .child(self.metric_row(
                                Icon::ScanText,
                                "OCR",
                                format!("{:.1}", metrics.ocr_ms),
                                Some("ms"),
                            ))
                            .child(self.metric_row(
                                Icon::MessageSquare,
                                "Subtitles",
                                format!("{}", metrics.cues),
                                Some("cues"),
                            ))
                            .child(self.metric_row(
                                Icon::Merge,
                                "Merged",
                                format!("{}", metrics.merged),
                                Some("cues"),
                            ))
                            .child(self.metric_row(
                                Icon::EyeOff,
                                "Empty OCR",
                                format!("{}", metrics.ocr_empty),
                                Some("frames"),
                            )),
                    )
                    .child(self.render_action_buttons(cx, status))
                    .when_some(error, |this, err| {
                        this.child(div().text_xs().text_color(self.theme.error()).child(err))
                    }),
            )
    }
}

impl StatusPanel {
    fn ensure_state_subscription(&mut self, cx: &mut Context<Self>) {
        if self.state_subscription.is_some() {
            return;
        }

        let state = self.state.clone();
        self.state_subscription = Some(cx.observe(&state, |_, _, cx| {
            cx.notify();
        }));
    }

    fn metric_row(&self, icon: Icon, label: &str, value: String, unit: Option<&str>) -> Div {
        let text_size = px(9.0);

        div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        IconComponent::new(icon)
                            .w(text_size)
                            .h(text_size)
                            .text_color(self.theme.text_secondary()),
                    )
                    .child(
                        div()
                            .text_size(text_size)
                            .text_color(self.theme.text_secondary())
                            .child(label.to_string()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(text_size)
                            .text_color(self.theme.text_primary())
                            .child(value),
                    )
                    .when_some(unit, |this, unit| {
                        this.child(
                            div()
                                .text_size(text_size)
                                .text_color(self.theme.text_secondary())
                                .child(unit.to_string()),
                        )
                    }),
            )
    }

    fn render_action_buttons(&self, cx: &mut Context<Self>, status: FileStatus) -> Div {
        let state = self.state.clone();
        let is_running = matches!(status, FileStatus::Detecting | FileStatus::Paused);

        let primary_label = match status {
            FileStatus::Detecting => "Pause",
            FileStatus::Paused => "Resume",
            _ => "Start Detection",
        };

        let primary_icon = match status {
            FileStatus::Detecting => Icon::Pause,
            FileStatus::Paused => Icon::Play,
            _ => Icon::Play,
        };

        let primary_action = cx.listener(move |_, _, _, cx| {
            state.update(cx, |state, cx| {
                if let Some(file) = state.get_active_file() {
                    let new_status = match file.status {
                        FileStatus::Detecting => FileStatus::Paused,
                        FileStatus::Paused => FileStatus::Detecting,
                        _ => FileStatus::Detecting,
                    };
                    state.update_file_status(file.id, new_status);
                    state.update_file_progress(file.id, (file.progress + 0.05).min(1.0));
                    state.set_error_message(None);
                } else {
                    state.set_error_message(Some(
                        "Please select a video before starting detection".to_string(),
                    ));
                }
                cx.notify();
            });
        });

        let state = self.state.clone();
        let cancel_action = cx.listener(move |_, _, _, cx| {
            state.update(cx, |state, cx| {
                if let Some(file) = state.get_active_file() {
                    state.update_file_status(file.id, FileStatus::Idle);
                    state.update_file_progress(file.id, 0.0);
                    state.set_error_message(None);
                }
                cx.notify();
            });
        });

        div()
            .flex()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap(px(4.0))
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(6.0))
                    .bg(self.theme.accent())
                    .cursor_pointer()
                    .hover(|s| s.bg(self.theme.accent_hover()))
                    .child(icon_sm(primary_icon, self.theme.background()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.background())
                            .child(primary_label),
                    )
                    .on_mouse_down(MouseButton::Left, primary_action),
            )
            .when(is_running, |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(4.0))
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(self.theme.danger())
                        .cursor_pointer()
                        .hover(|s| s.bg(self.theme.danger_hover()))
                        .child(icon_sm(Icon::Stop, self.theme.background()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(self.theme.background())
                                .child("Cancel"),
                        )
                        .on_mouse_down(MouseButton::Left, cancel_action),
                )
            })
    }

    fn status_text(&self, status: FileStatus) -> &'static str {
        match status {
            FileStatus::Idle => "Ready",
            FileStatus::Detecting => "Detecting",
            FileStatus::Paused => "Paused",
            FileStatus::Completed => "Completed",
            FileStatus::Failed => "Failed",
            FileStatus::Canceled => "Canceled",
        }
    }

    fn status_color(&self, status: FileStatus) -> Hsla {
        match status {
            FileStatus::Idle => self.theme.text_tertiary(),
            FileStatus::Detecting => self.theme.accent(),
            FileStatus::Paused => self.theme.warning(),
            FileStatus::Completed => self.theme.success(),
            FileStatus::Failed => self.theme.error(),
            FileStatus::Canceled => self.theme.text_tertiary(),
        }
    }
}

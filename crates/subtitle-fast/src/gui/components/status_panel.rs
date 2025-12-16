use crate::gui::state::{AppState, FileStatus};
use crate::gui::theme::AppTheme;
use gpui::prelude::*;
use gpui::*;
use std::sync::Arc;

pub struct StatusPanel {
    state: Arc<AppState>,
    theme: AppTheme,
}

impl StatusPanel {
    pub fn new(state: Arc<AppState>, theme: AppTheme) -> Self {
        Self { state, theme }
    }
}

impl Render for StatusPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let metrics = self.state.get_metrics();
        let active_file = self.state.get_active_file();
        let error = self.state.get_error_message();

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
            .border_1()
            .border_color(self.theme.border())
            .rounded_md()
            .p_4()
            .gap_4()
            .child(
                div().flex().justify_between().items_center().child(
                    div()
                        .text_sm()
                        .text_color(self.theme.text_primary())
                        .child("Detection Progress"),
                ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.text_secondary())
                    .child(self.status_text(status)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .w_full()
                            .h_2()
                            .bg(self.theme.border())
                            .rounded_sm()
                            .child(
                                div()
                                    .h_full()
                                    .bg(self.theme.accent())
                                    .rounded_sm()
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
                    .gap_2()
                    .child(self.render_metric("FPS".to_string(), format!("{:.1}", metrics.fps)))
                    .child(self.render_metric(
                        "Detection".to_string(),
                        format!("{:.1} ms", metrics.det_ms),
                    ))
                    .child(
                        self.render_metric("OCR".to_string(), format!("{:.1} ms", metrics.ocr_ms)),
                    )
                    .child(self.render_metric("Cues".to_string(), format!("{}", metrics.cues)))
                    .child(self.render_metric("Merged".to_string(), format!("{}", metrics.merged)))
                    .child(
                        self.render_metric(
                            "Empty OCR".to_string(),
                            format!("{}", metrics.ocr_empty),
                        ),
                    ),
            )
            .child(self.render_action_buttons(status))
            .when_some(error, |this, err| {
                this.child(div().text_xs().text_color(self.theme.error()).child(err))
            })
    }
}

impl StatusPanel {
    fn render_metric(&self, label: String, value: String) -> Div {
        div()
            .flex()
            .justify_between()
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.text_secondary())
                    .child(label),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.text_primary())
                    .child(value),
            )
    }

    fn render_action_buttons(&self, status: FileStatus) -> Div {
        let button_text = match status {
            FileStatus::Detecting => "Pause",
            FileStatus::Paused => "Resume",
            _ => "Start Detection",
        };

        div()
            .flex()
            .gap_2()
            .justify_center()
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(self.theme.accent())
                    .text_color(self.theme.background())
                    .rounded_md()
                    .text_xs()
                    .child(button_text),
            )
            .when(
                matches!(status, FileStatus::Detecting | FileStatus::Paused),
                |this| {
                    this.child(
                        div()
                            .px_4()
                            .py_2()
                            .bg(self.theme.error())
                            .text_color(self.theme.background())
                            .rounded_md()
                            .text_xs()
                            .child("Cancel"),
                    )
                },
            )
    }

    fn status_text(&self, status: FileStatus) -> &'static str {
        match status {
            FileStatus::Idle => "Ready to start",
            FileStatus::Detecting => "Detecting subtitles...",
            FileStatus::Paused => "Detection paused",
            FileStatus::Completed => "Detection completed",
            FileStatus::Failed => "Detection failed",
            FileStatus::Canceled => "Detection canceled",
        }
    }
}

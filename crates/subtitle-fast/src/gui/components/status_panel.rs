use crate::gui::state::{AppState, FileStatus};
use crate::gui::theme::AppTheme;
use gpui::prelude::*;
use gpui::*;
use gpui::{InteractiveElement, MouseButton};
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .p(px(14.0))
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .text_color(self.theme.text_primary())
                            .child("检测速度"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child(self.status_text(status)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .w_full()
                            .h(px(10.0))
                            .bg(self.theme.border())
                            .rounded_full()
                            .child(
                                div()
                                    .h_full()
                                    .bg(self.theme.accent())
                                    .rounded_full()
                                    .w(relative(progress as f32)),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(self.theme.text_tertiary())
                            .child(format!("{:.0}%", progress * 100.0)),
                    ),
            )
            .child(div().flex().flex_wrap().gap(px(10.0)).children(vec![
                self.render_metric("帧率".to_string(), format!("{:.1} fps", metrics.fps)),
                self.render_metric("检测".to_string(), format!("{:.1} ms", metrics.det_ms)),
                self.render_metric("OCR".to_string(), format!("{:.1} ms", metrics.ocr_ms)),
                self.render_metric("字幕".to_string(), format!("{}", metrics.cues)),
                self.render_metric("合并".to_string(), format!("{}", metrics.merged)),
                self.render_metric("空OCR".to_string(), format!("{}", metrics.ocr_empty)),
            ]))
            .child(self.render_action_buttons(cx, status))
            .when_some(error, |this, err| {
                this.child(div().text_xs().text_color(self.theme.error()).child(err))
            })
    }
}

impl StatusPanel {
    fn render_metric(&self, label: String, value: String) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .p(px(10.0))
            .rounded_md()
            .bg(self.theme.surface_elevated())
            .border_1()
            .border_color(self.theme.border())
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.text_secondary())
                    .child(label),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(self.theme.text_primary())
                    .child(value),
            )
    }

    fn render_action_buttons(&self, cx: &mut Context<Self>, status: FileStatus) -> Div {
        let button_text = match status {
            FileStatus::Detecting => "暂停检测",
            FileStatus::Paused => "继续检测",
            _ => "开始检测",
        };

        let primary_action = cx.listener(|this, _, _, cx| {
            if let Some(file) = this.state.get_active_file() {
                let new_status = match file.status {
                    FileStatus::Detecting => FileStatus::Paused,
                    FileStatus::Paused => FileStatus::Detecting,
                    _ => FileStatus::Detecting,
                };
                this.state.update_file_status(file.id, new_status);
                this.state
                    .update_file_progress(file.id, (file.progress + 0.05).min(1.0));
                this.state.set_error_message(None);
            } else {
                this.state
                    .set_error_message(Some("请先选择视频后再开始检测".to_string()));
            }
            cx.notify();
        });

        let cancel_action = cx.listener(|this, _, _, cx| {
            if let Some(file) = this.state.get_active_file() {
                this.state.update_file_status(file.id, FileStatus::Idle);
                this.state.update_file_progress(file.id, 0.0);
                this.state.set_error_message(None);
            }
            cx.notify();
        });

        div()
            .flex()
            .gap(px(8.0))
            .justify_between()
            .child(
                div()
                    .flex_1()
                    .px(px(12.0))
                    .py(px(10.0))
                    .bg(self.theme.accent())
                    .text_color(self.theme.background())
                    .rounded_md()
                    .text_xs()
                    .text_center()
                    .child(button_text)
                    .on_mouse_down(MouseButton::Left, primary_action),
            )
            .when(
                matches!(status, FileStatus::Detecting | FileStatus::Paused),
                |this| {
                    this.child(
                        div()
                            .px(px(12.0))
                            .py(px(10.0))
                            .bg(self.theme.error())
                            .text_color(self.theme.background())
                            .rounded_md()
                            .text_xs()
                            .text_center()
                            .child("取消")
                            .on_mouse_down(MouseButton::Left, cancel_action),
                    )
                },
            )
    }

    fn status_text(&self, status: FileStatus) -> &'static str {
        match status {
            FileStatus::Idle => "就绪",
            FileStatus::Detecting => "检测中",
            FileStatus::Paused => "已暂停",
            FileStatus::Completed => "完成",
            FileStatus::Failed => "失败",
            FileStatus::Canceled => "已取消",
        }
    }
}

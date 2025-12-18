use crate::gui::state::{AppState, FileStatus, TrackedFile};
use crate::gui::theme::AppTheme;
use gpui::prelude::*;
use gpui::*;
use gpui::{InteractiveElement, MouseButton};
use std::sync::Arc;

pub struct Sidebar {
    state: Arc<AppState>,
    theme: AppTheme,
}

impl Sidebar {
    pub fn new(state: Arc<AppState>, theme: AppTheme) -> Self {
        Self { state, theme }
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let files = self.state.get_files();
        let is_empty = files.is_empty();
        let active_id = self.state.get_active_file_id();

        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .bg(self.theme.surface())
            .border_r_1()
            .border_color(self.theme.border())
            .rounded_md()
            .p(px(14.0))
            .gap(px(12.0))
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
                            .child(
                                div()
                                    .w(px(10.0))
                                    .h(px(10.0))
                                    .rounded_full()
                                    .bg(self.theme.accent()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(self.theme.text_primary())
                                    .child("视频"),
                            ),
                    )
                    .child(
                        div()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded_md()
                            .bg(self.theme.surface_elevated())
                            .border_1()
                            .border_color(self.theme.border())
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child("导入"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .children(
                        files
                            .into_iter()
                            .map(|file| self.render_file_item(file, active_id, cx)),
                    )
                    .when(is_empty, |container| {
                        container.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .py(px(40.0))
                                .rounded_md()
                                .bg(self.theme.surface_elevated())
                                .border_1()
                                .border_color(self.theme.border())
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(self.theme.text_tertiary())
                                        .child("导入视频后会显示在这里"),
                                ),
                        )
                    }),
            )
    }
}

impl Sidebar {
    fn render_file_item(
        &self,
        file: TrackedFile,
        active_id: Option<crate::gui::state::FileId>,
        cx: &mut Context<Self>,
    ) -> Div {
        let is_active = active_id == Some(file.id);
        let file_name = file
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unknown")
            .to_string();

        div()
            .flex()
            .flex_col()
            .p(px(12.0))
            .rounded_md()
            .bg(if is_active {
                self.theme.accent_muted()
            } else {
                self.theme.surface_elevated()
            })
            .border_1()
            .border_color(if is_active {
                self.theme.border_focused()
            } else {
                self.theme.border()
            })
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_primary())
                            .child(file_name),
                    )
                    .child(div().flex().items_center().gap(px(6.0)).children(vec![
                            div()
                                .w(px(8.0))
                                .h(px(8.0))
                                .rounded_full()
                                .bg(if matches!(file.status, FileStatus::Detecting) {
                                    self.theme.accent()
                                } else {
                                    self.theme.border()
                                }),
                            div()
                                .text_xs()
                                .text_color(self.theme.text_secondary())
                                .child(format!("{:.0}%", file.progress * 100.0)),
                        ])),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.text_secondary())
                    .child(self.status_text(file.status)),
            )
            .child(
                div()
                    .w_full()
                    .h(px(6.0))
                    .rounded_full()
                    .bg(self.theme.border())
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(self.theme.accent())
                            .w(relative(file.progress as f32)),
                    ),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.state.set_active_file(file.id);
                    cx.notify();
                }),
            )
    }

    fn status_text(&self, status: FileStatus) -> &'static str {
        match status {
            FileStatus::Idle => "待命",
            FileStatus::Detecting => "检测中",
            FileStatus::Paused => "已暂停",
            FileStatus::Completed => "完成",
            FileStatus::Failed => "失败",
            FileStatus::Canceled => "已取消",
        }
    }
}

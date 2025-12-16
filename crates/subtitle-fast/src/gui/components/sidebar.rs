use crate::gui::state::{AppState, FileStatus, TrackedFile};
use crate::gui::theme::AppTheme;
use gpui::prelude::*;
use gpui::*;
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
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let files = self.state.get_files();
        let active_id = self.state.get_active_file_id();

        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .bg(self.theme.surface())
            .border_r_1()
            .border_color(self.theme.border())
            .p_3()
            .gap_2()
            .child(
                // Header
                div().flex().items_center().gap_2().child(
                    div()
                        .text_sm()
                        .text_color(self.theme.text_primary())
                        .child("Files"),
                ),
            )
            .child(
                div().flex().flex_col().gap_2().children(
                    files
                        .into_iter()
                        .map(|file| self.render_file_item(file, active_id)),
                ),
            )
    }
}

impl Sidebar {
    fn render_file_item(
        &self,
        file: TrackedFile,
        active_id: Option<crate::gui::state::FileId>,
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
            .p_2()
            .rounded_md()
            .bg(if is_active {
                self.theme.surface_elevated()
            } else {
                self.theme.surface()
            })
            .border_1()
            .border_color(if is_active {
                self.theme.border_focused()
            } else {
                self.theme.border()
            })
            .gap_1()
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
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child(format!("{:.0}%", file.progress * 100.0)),
                    ),
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
                    .h_1()
                    .bg(self.theme.border())
                    .rounded_sm()
                    .child(
                        div()
                            .h_full()
                            .bg(self.theme.accent())
                            .rounded_sm()
                            .w(relative(file.progress as f32)),
                    ),
            )
    }

    fn status_text(&self, status: FileStatus) -> &'static str {
        match status {
            FileStatus::Idle => "Idle",
            FileStatus::Detecting => "Detecting...",
            FileStatus::Paused => "Paused",
            FileStatus::Completed => "Completed",
            FileStatus::Failed => "Failed",
            FileStatus::Canceled => "Canceled",
        }
    }
}

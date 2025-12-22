use crate::gui::icons::{Icon, icon_sm};
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(12.0))
                    .pt(px(14.0))
                    .pb(px(10.0))
                    .child(icon_sm(Icon::Film, self.theme.text_secondary()))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(self.theme.text_primary())
                            .child("Videos"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .px(px(8.0))
                    .pb(px(10.0))
                    .gap(px(6.0))
                    .overflow_hidden()
                    .children(
                        files
                            .into_iter()
                            .map(|file| self.render_file_item(file, active_id, cx)),
                    )
                    .when(is_empty, |container| {
                        container.child(
                            div()
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .py(px(32.0))
                                .gap(px(8.0))
                                .child(icon_sm(Icon::Film, self.theme.text_tertiary()))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(self.theme.text_tertiary())
                                        .child("Imported videos will appear here"),
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

        let progress = file.progress;
        let status = file.status;
        let file_icon = if is_active {
            Icon::PlaySquare
        } else {
            Icon::Film
        };

        let base_bg = if is_active {
            self.theme.accent_muted()
        } else {
            gpui::transparent_black()
        };

        div()
            .flex()
            .flex_col()
            .px(px(10.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .bg(base_bg)
            .gap(px(6.0))
            .cursor_pointer()
            .hover(|s| {
                if !is_active {
                    s.bg(self.theme.surface_hover())
                } else {
                    s.bg(self.theme.accent_muted().opacity(0.75))
                }
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(icon_sm(file_icon, self.theme.text_secondary()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(self.theme.text_primary())
                                    .max_w(px(100.0))
                                    .overflow_hidden()
                                    .child(file_name),
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
                    .text_xs()
                    .text_color(self.status_color(status))
                    .child(self.status_text(status)),
            )
            .child(
                div()
                    .w_full()
                    .h(px(4.0))
                    .rounded_full()
                    .bg(self.theme.border().opacity(0.7))
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(self.theme.accent())
                            .w(relative(progress as f32)),
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
            FileStatus::Idle => "Idle",
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

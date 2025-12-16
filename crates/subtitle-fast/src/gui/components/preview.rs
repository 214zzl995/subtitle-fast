use crate::gui::state::AppState;
use crate::gui::theme::AppTheme;
use gpui::prelude::*;
use gpui::*;
use std::sync::Arc;

pub struct PreviewPanel {
    state: Arc<AppState>,
    theme: AppTheme,
}

impl PreviewPanel {
    pub fn new(state: Arc<AppState>, theme: AppTheme) -> Self {
        Self { state, theme }
    }
}

impl Render for PreviewPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let file = self.state.get_active_file();

        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .bg(self.theme.surface())
            .border_1()
            .border_color(self.theme.border())
            .rounded_md()
            .overflow_hidden()
            .child(if let Some(file) = file {
                self.render_preview(&file)
            } else {
                self.render_empty_state()
            })
    }
}

impl PreviewPanel {
    fn render_preview(&self, file: &crate::gui::state::TrackedFile) -> Div {
        div()
            .flex()
            .items_center()
            .justify_center()
            .w_full()
            .h_full()
            .bg(hsla(0.0, 0.0, 0.08, 1.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(self.theme.text_secondary())
                            .child("Video Preview"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_tertiary())
                            .child(format!("{}", file.path.display())),
                    ),
            )
    }

    fn render_empty_state(&self) -> Div {
        div()
            .flex()
            .items_center()
            .justify_center()
            .w_full()
            .h_full()
            .child(
                div()
                    .text_sm()
                    .text_color(self.theme.text_tertiary())
                    .child("No video selected"),
            )
    }
}

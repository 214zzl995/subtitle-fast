use crate::gui::icons::{Icon, icon_sm};
use crate::gui::state::AppState;
use crate::gui::theme::AppTheme;
use gpui::InteractiveElement;
use gpui::prelude::*;
use gpui::*;
use std::sync::Arc;

pub struct SubtitleList {
    state: Arc<AppState>,
    theme: AppTheme,
}

impl SubtitleList {
    pub fn new(state: Arc<AppState>, theme: AppTheme) -> Self {
        Self { state, theme }
    }

    pub fn set_theme(&mut self, theme: AppTheme) {
        self.theme = theme;
    }
}

impl Render for SubtitleList {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let subtitles = self.state.get_subtitles();

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
                    .justify_between()
                    .px(px(12.0))
                    .py(px(10.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(icon_sm(Icon::MessageSquare, self.theme.text_secondary()))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(self.theme.text_primary())
                                    .child("Subtitles"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(6.0))
                            .bg(self.theme.surface_elevated())
                            .cursor_pointer()
                            .hover(|s| s.bg(self.theme.surface_hover()))
                            .child(icon_sm(Icon::Upload, self.theme.text_secondary())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .p(px(8.0))
                    .gap(px(6.0))
                    .overflow_hidden()
                    .when(subtitles.is_empty(), |div| {
                        div.child(self.render_empty_state())
                    })
                    .children(
                        subtitles
                            .into_iter()
                            .map(|cue| self.render_subtitle_item(cue)),
                    ),
            )
    }
}

impl SubtitleList {
    fn render_subtitle_item(&self, cue: crate::gui::state::SubtitleCue) -> Div {
        div()
            .flex()
            .flex_col()
            .p(px(10.0))
            .rounded(px(6.0))
            .bg(self.theme.surface_elevated())
            .gap(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(self.theme.surface_hover()))
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.text_tertiary())
                    .child(self.format_time_range(cue.start_ms, cue.end_ms)),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(self.theme.text_primary())
                    .child(cue.text),
            )
    }

    fn render_empty_state(&self) -> Div {
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .h_full()
            .py(px(40.0))
            .gap(px(12.0))
            .child(icon_sm(Icon::MessageSquare, self.theme.text_tertiary()))
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.text_tertiary())
                    .text_center()
                    .child("Subtitles will appear here after detection starts"),
            )
    }

    fn format_time_range(&self, start_ms: f64, end_ms: f64) -> String {
        fn ms_to_time(ms: f64) -> String {
            let total_secs = (ms / 1000.0) as u64;
            let hours = total_secs / 3600;
            let minutes = (total_secs % 3600) / 60;
            let seconds = total_secs % 60;
            let millis = (ms % 1000.0) as u64;
            format!("{:02}:{:02}:{:02}.{:03}", hours, minutes, seconds, millis)
        }

        format!("{} - {}", ms_to_time(start_ms), ms_to_time(end_ms))
    }
}

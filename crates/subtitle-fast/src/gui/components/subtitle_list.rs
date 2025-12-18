use crate::gui::state::AppState;
use crate::gui::theme::AppTheme;
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
            .border_1()
            .border_color(self.theme.border())
            .rounded_md()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .p(px(12.0))
                    .border_b_1()
                    .border_color(self.theme.border())
                    .child(
                        div()
                            .text_sm()
                            .text_color(self.theme.text_primary())
                            .child("字幕列表"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(self.theme.text_secondary())
                                    .child(format!("{} 条", subtitles.len())),
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
                                    .child("导出"),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .p(px(12.0))
                    .gap(px(8.0))
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
            .p(px(12.0))
            .bg(self.theme.surface_elevated())
            .border_1()
            .border_color(self.theme.border())
            .rounded_md()
            .gap(px(6.0))
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
        div().flex().items_center().justify_center().h_full().child(
            div()
                .text_sm()
                .text_color(self.theme.text_tertiary())
                .child("开始检测后，字幕会出现在这里"),
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

        format!("{} → {}", ms_to_time(start_ms), ms_to_time(end_ms))
    }
}

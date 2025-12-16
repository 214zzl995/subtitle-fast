use crate::gui::state::AppState;
use crate::gui::theme::AppTheme;
use gpui::prelude::*;
use gpui::*;
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
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let threshold = self.state.get_threshold();
        let tolerance = self.state.get_tolerance();
        let roi = self.state.get_roi();

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
                div()
                    .text_sm()
                    .text_color(self.theme.text_primary())
                    .child("Detection Settings"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child("Region of Interest"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_tertiary())
                            .child(if let Some(r) = roi {
                                format!(
                                    "x:{:.2} y:{:.2} w:{:.2} h:{:.2}",
                                    r.x, r.y, r.width, r.height
                                )
                            } else {
                                "None (full frame)".to_string()
                            }),
                    ),
            )
            .child(self.render_slider("Threshold".to_string(), threshold, 0.0, 255.0))
            .child(self.render_slider("Tolerance".to_string(), tolerance, 0.0, 50.0))
    }
}

impl ControlPanel {
    fn render_slider(&self, label: String, value: f64, _min: f64, _max: f64) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
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
                            .text_color(self.theme.text_tertiary())
                            .child(format!("{:.0}", value)),
                    ),
            )
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
                            .w(relative(0.5)),
                    ),
            )
    }
}

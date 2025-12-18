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
        let roi = self.state.get_roi();
        let selection_visible = self.state.selection_visible();
        let highlight_enabled = self.state.highlight_enabled();

        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .bg(self.theme.surface())
            .border_1()
            .border_color(self.theme.border())
            .rounded_md()
            .p(px(12.0))
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .text_color(self.theme.text_primary())
                            .child("预览"),
                    )
                    .child(div().flex().items_center().gap(px(8.0)).children(vec![
                        self.chip("字幕", true),
                        self.chip("亮色区域", highlight_enabled),
                        self.chip("叠加", selection_visible),
                    ])),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .relative()
                    .bg(self.theme.translucent_panel())
                    .rounded_md()
                    .overflow_hidden()
                    .child(if let Some(file) = file {
                        self.render_preview(&file, roi, selection_visible, highlight_enabled)
                    } else {
                        self.render_empty_state()
                    }),
            )
    }
}

impl PreviewPanel {
    fn render_preview(
        &self,
        file: &crate::gui::state::TrackedFile,
        roi: Option<crate::gui::state::RoiSelection>,
        selection_visible: bool,
        highlight_enabled: bool,
    ) -> Div {
        let overlay = roi.unwrap_or(crate::gui::state::RoiSelection {
            x: 0.15,
            y: 0.75,
            width: 0.70,
            height: 0.25,
        });

        div()
            .relative()
            .w_full()
            .h_full()
            .bg(self.theme.surface_elevated())
            .child(
                div().absolute().inset_0().child(
                    div()
                        .w_full()
                        .h_full()
                        .bg(hsla(215.0, 0.18, 0.08, 1.0))
                        .child(div().absolute().inset_0().bg(self.theme.overlay()))
                        .child(
                            div()
                                .absolute()
                                .left(px(22.0))
                                .top(px(22.0))
                                .text_xs()
                                .text_color(self.theme.text_secondary())
                                .child(format!("{}", file.path.display())),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(22.0))
                                .bottom(px(22.0))
                                .text_sm()
                                .text_color(self.theme.text_primary())
                                .child("我再问一次\n请你解释安东生先生魔术的原理"),
                        ),
                ),
            )
            .child(self.render_overlay_toolbar(selection_visible, highlight_enabled))
            .when(selection_visible, |div| {
                div.child(self.render_selection_overlay(overlay, highlight_enabled))
            })
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

    fn render_selection_overlay(
        &self,
        roi: crate::gui::state::RoiSelection,
        highlight: bool,
    ) -> Div {
        let (x, y, w, h) = (roi.x, roi.y, roi.width, roi.height);

        div().absolute().inset_0().child(
            div()
                .flex()
                .items_end()
                .justify_center()
                .w_full()
                .h_full()
                .child(
                    div()
                        .relative()
                        .w(relative(w))
                        .h(relative(h))
                        .bg(if highlight {
                            self.theme.accent_muted()
                        } else {
                            hsla(215.0, 0.1, 0.08, 0.4)
                        })
                        .border_1()
                        .border_color(self.theme.overlay_dashed())
                        .border_dashed()
                        .rounded_md()
                        .child(
                            div()
                                .absolute()
                                .left(px(6.0))
                                .top(px(6.0))
                                .text_xs()
                                .text_color(self.theme.text_secondary())
                                .child(format!("x {:.2} y {:.2} w {:.2} h {:.2}", x, y, w, h)),
                        )
                        .children(self.render_handles()),
                ),
        )
    }

    fn render_handles(&self) -> Vec<Div> {
        vec![
            div()
                .absolute()
                .left(px(-6.0))
                .top(px(-6.0))
                .w(px(12.0))
                .h(px(12.0))
                .rounded_full()
                .bg(self.theme.accent())
                .border_1()
                .border_color(self.theme.border_focused()),
            div()
                .absolute()
                .right(px(-6.0))
                .top(px(-6.0))
                .w(px(12.0))
                .h(px(12.0))
                .rounded_full()
                .bg(self.theme.accent())
                .border_1()
                .border_color(self.theme.border_focused()),
            div()
                .absolute()
                .left(px(-6.0))
                .bottom(px(-6.0))
                .w(px(12.0))
                .h(px(12.0))
                .rounded_full()
                .bg(self.theme.accent())
                .border_1()
                .border_color(self.theme.border_focused()),
            div()
                .absolute()
                .right(px(-6.0))
                .bottom(px(-6.0))
                .w(px(12.0))
                .h(px(12.0))
                .rounded_full()
                .bg(self.theme.accent())
                .border_1()
                .border_color(self.theme.border_focused()),
        ]
    }

    fn render_overlay_toolbar(&self, selection_visible: bool, highlight_enabled: bool) -> Div {
        div()
            .absolute()
            .top(px(14.0))
            .right(px(14.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .children(vec![
                self.rounded_icon(selection_visible, "选区"),
                self.rounded_icon(highlight_enabled, "亮度"),
                self.rounded_icon(true, "色彩"),
            ])
    }

    fn rounded_icon(&self, active: bool, label: &str) -> Div {
        div()
            .px(px(12.0))
            .py(px(8.0))
            .rounded_full()
            .bg(if active {
                self.theme.accent()
            } else {
                self.theme.surface_elevated()
            })
            .border_1()
            .border_color(if active {
                self.theme.border_focused()
            } else {
                self.theme.border()
            })
            .text_xs()
            .text_color(if active {
                self.theme.background()
            } else {
                self.theme.text_secondary()
            })
            .child(label.to_string())
    }

    fn chip(&self, label: &str, active: bool) -> Div {
        div()
            .px(px(10.0))
            .py(px(6.0))
            .rounded_md()
            .bg(if active {
                self.theme.accent_muted()
            } else {
                self.theme.surface_elevated()
            })
            .border_1()
            .border_color(if active {
                self.theme.border_focused()
            } else {
                self.theme.border()
            })
            .text_xs()
            .text_color(self.theme.text_secondary())
            .child(label.to_string())
    }
}

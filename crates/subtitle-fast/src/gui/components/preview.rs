use crate::gui::icons::{Icon, icon_sm};
use crate::gui::state::AppState;
use crate::gui::theme::AppTheme;
use gpui::prelude::*;
use gpui::*;
use gpui::{InteractiveElement, MouseButton};
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(8.0))
                    .py(px(6.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(icon_sm(
                                Icon::GalleryThumbnails,
                                self.theme.text_secondary(),
                            ))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(self.theme.text_primary())
                                    .child("Preview"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(self.icon_toggle(
                                cx,
                                if selection_visible {
                                    Icon::Eye
                                } else {
                                    Icon::EyeOff
                                },
                                selection_visible,
                                |state| {
                                    state.toggle_selection_visibility();
                                },
                            ))
                            .child(self.icon_toggle(cx, Icon::RotateCcw, false, |_| {}))
                            .child(self.icon_toggle(cx, Icon::Crosshair, false, |_| {}))
                            .child(self.icon_toggle(cx, Icon::Sun, highlight_enabled, |state| {
                                state.toggle_highlight();
                            }))
                            .child(
                                div()
                                    .w(px(1.0))
                                    .h(px(16.0))
                                    .bg(self.theme.border())
                                    .mx(px(4.0)),
                            )
                            .child(self.color_picker_button())
                            .child(self.y_plane_toggle()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .pt(px(0.0))
                    .px(px(8.0))
                    .pb(px(6.0))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .rounded(px(12.0))
                            .border_1()
                            .border_color(self.theme.border())
                            .bg(hsla(0.0, 0.0, 0.04, 1.0))
                            .overflow_hidden()
                            .child(if let Some(file) = file {
                                self.render_preview(
                                    &file,
                                    roi,
                                    selection_visible,
                                    highlight_enabled,
                                )
                            } else {
                                self.render_empty_state()
                            }),
                    ),
            )
    }
}

impl PreviewPanel {
    fn icon_toggle(
        &self,
        cx: &mut Context<Self>,
        icon: Icon,
        active: bool,
        toggle: impl Fn(&AppState) + 'static,
    ) -> Div {
        div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(24.0))
            .h(px(24.0))
            .rounded(px(5.0))
            .bg(if active {
                self.theme.accent_muted()
            } else {
                gpui::transparent_black()
            })
            .cursor_pointer()
            .hover(|s| s.bg(self.theme.surface_hover()))
            .child(icon_sm(
                icon,
                if active {
                    self.theme.accent()
                } else {
                    self.theme.text_secondary()
                },
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    toggle(&this.state);
                    cx.notify();
                }),
            )
    }

    fn color_picker_button(&self) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(6.0))
            .h(px(24.0))
            .rounded(px(5.0))
            .cursor_pointer()
            .hover(|s| s.bg(self.theme.surface_hover()))
            .child(
                div()
                    .w(px(12.0))
                    .h(px(12.0))
                    .rounded(px(2.0))
                    .bg(self.theme.accent())
                    .border_1()
                    .border_color(self.theme.border()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.text_secondary())
                    .child("Foreground"),
            )
            .child(icon_sm(Icon::ChevronDown, self.theme.text_tertiary()))
    }

    fn y_plane_toggle(&self) -> Div {
        div()
            .flex()
            .items_center()
            .px(px(8.0))
            .h(px(24.0))
            .rounded_full()
            .bg(self.theme.surface_elevated())
            .cursor_pointer()
            .hover(|s| s.bg(self.theme.surface_hover()))
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.text_secondary())
                    .child("Y Plane"),
            )
    }

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
            .bg(hsla(0.0, 0.0, 0.02, 1.0))
            .child(
                div()
                    .absolute()
                    .left(px(12.0))
                    .top(px(12.0))
                    .text_xs()
                    .text_color(self.theme.text_tertiary())
                    .child(format!("{}", file.path.display())),
            )
            .child(
                div()
                    .absolute()
                    .left(px(12.0))
                    .bottom(px(12.0))
                    .text_sm()
                    .text_color(self.theme.text_primary())
                    .child("Sample subtitle text\nfor demonstration purposes"),
            )
            .when(selection_visible, |div| {
                div.child(self.render_selection_overlay(overlay, highlight_enabled))
            })
    }

    fn render_empty_state(&self) -> Div {
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .w_full()
            .h_full()
            .gap(px(12.0))
            .child(icon_sm(Icon::Film, self.theme.text_tertiary()))
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
                            hsla(210.0 / 360.0, 0.15, 0.12, 0.5)
                        })
                        .border_1()
                        .border_color(self.theme.overlay_dashed())
                        .rounded(px(4.0))
                        .child(
                            div()
                                .absolute()
                                .left(px(8.0))
                                .top(px(8.0))
                                .text_xs()
                                .text_color(self.theme.text_secondary())
                                .child(format!("x{:.2} y{:.2} w{:.2} h{:.2}", x, y, w, h)),
                        )
                        .children(self.render_handles()),
                ),
        )
    }

    fn render_handles(&self) -> Vec<Div> {
        let handle_size = px(10.0);
        let offset = px(-5.0);

        vec![
            div()
                .absolute()
                .left(offset)
                .top(offset)
                .size(handle_size)
                .rounded_full()
                .bg(self.theme.accent())
                .border_2()
                .border_color(self.theme.background())
                .cursor(CursorStyle::ResizeUpLeftDownRight),
            div()
                .absolute()
                .right(offset)
                .top(offset)
                .size(handle_size)
                .rounded_full()
                .bg(self.theme.accent())
                .border_2()
                .border_color(self.theme.background())
                .cursor(CursorStyle::ResizeUpRightDownLeft),
            div()
                .absolute()
                .left(offset)
                .bottom(offset)
                .size(handle_size)
                .rounded_full()
                .bg(self.theme.accent())
                .border_2()
                .border_color(self.theme.background())
                .cursor(CursorStyle::ResizeUpRightDownLeft),
            div()
                .absolute()
                .right(offset)
                .bottom(offset)
                .size(handle_size)
                .rounded_full()
                .bg(self.theme.accent())
                .border_2()
                .border_color(self.theme.background())
                .cursor(CursorStyle::ResizeUpLeftDownRight),
        ]
    }
}

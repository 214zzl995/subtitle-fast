use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, Context, Entity, Render, Window, div, ease_out_quint, hsla, px,
};

use crate::gui::components::{FramePreprocessor, VideoPlayerControlHandle, VideoRoiOverlay};
use crate::gui::icons::{Icon, icon_sm};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoViewMode {
    FullColor,
    YPlane,
}

pub struct VideoToolbar {
    controls: Option<VideoPlayerControlHandle>,
    roi_overlay: Option<Entity<VideoRoiOverlay>>,
    view: VideoViewMode,
    slide_from: VideoViewMode,
    slide_token: u64,
}

impl VideoToolbar {
    pub fn new() -> Self {
        Self {
            controls: None,
            roi_overlay: None,
            view: VideoViewMode::FullColor,
            slide_from: VideoViewMode::FullColor,
            slide_token: 0,
        }
    }

    pub fn set_controls(&mut self, controls: Option<VideoPlayerControlHandle>) {
        self.controls = controls;
        self.apply_view();
    }

    pub fn set_roi_overlay(&mut self, overlay: Option<Entity<VideoRoiOverlay>>) {
        self.roi_overlay = overlay;
    }

    fn set_view(&mut self, view: VideoViewMode, cx: &mut Context<Self>) {
        if self.view == view {
            return;
        }
        self.slide_from = self.view;
        self.slide_token = self.slide_token.wrapping_add(1);
        self.view = view;
        self.apply_view();
        cx.notify();
    }

    fn apply_view(&self) {
        let Some(controls) = self.controls.as_ref() else {
            return;
        };
        match self.view {
            VideoViewMode::FullColor => controls.clear_preprocessor(),
            VideoViewMode::YPlane => controls.set_preprocessor(y_plane_preprocessor()),
        }
    }
}

impl Render for VideoToolbar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.controls.is_some();
        let text_color = if enabled {
            hsla(0.0, 0.0, 1.0, 0.8)
        } else {
            hsla(0.0, 0.0, 1.0, 0.35)
        };
        let active_text_color = if enabled {
            hsla(0.0, 0.0, 1.0, 0.92)
        } else {
            hsla(0.0, 0.0, 1.0, 0.4)
        };
        let inactive_text_color = if enabled {
            hsla(0.0, 0.0, 1.0, 0.62)
        } else {
            hsla(0.0, 0.0, 1.0, 0.35)
        };

        let toggle_height = px(24.0);
        let segment_width = px(42.0);
        let segment_radius = px(7.0);
        let toggle_width = segment_width * 2.0;

        let reset_button = {
            let mut view = div()
                .id(("video-view-reset-roi", cx.entity_id()))
                .flex()
                .items_center()
                .justify_center()
                .h(toggle_height)
                .w(toggle_height)
                .rounded(segment_radius)
                .border_1()
                .border_color(hsla(0.0, 0.0, 1.0, if enabled { 0.32 } else { 0.2 }))
                .child(
                    icon_sm(
                        Icon::RotateCcw,
                        if enabled {
                            active_text_color
                        } else {
                            inactive_text_color
                        },
                    )
                    .w(px(12.0))
                    .h(px(12.0)),
                );

            if enabled {
                if let Some(roi_overlay) = self.roi_overlay.clone() {
                    view = view
                        .cursor_pointer()
                        .hover(|style| style.bg(hsla(0.0, 0.0, 1.0, 0.08)))
                        .on_click(cx.listener(move |_, _event, _window, cx| {
                            let _ = roi_overlay.update(cx, |overlay, cx| {
                                overlay.reset_roi(cx);
                            });
                        }));
                }
            }

            view
        };

        let button = |label: &'static str, active: bool, mode: VideoViewMode| {
            let id = match mode {
                VideoViewMode::FullColor => "video-view-full",
                VideoViewMode::YPlane => "video-view-y",
            };
            let mut view = div()
                .id((id, cx.entity_id()))
                .flex()
                .items_center()
                .justify_center()
                .h_full()
                .w(segment_width)
                .text_size(px(12.0))
                .line_height(px(12.0))
                .text_color(if active {
                    active_text_color
                } else {
                    inactive_text_color
                })
                .child(label);

            if enabled {
                view = view.cursor_pointer().on_click(cx.listener(
                    move |this, _event, _window, cx| {
                        this.set_view(mode, cx);
                    },
                ));
            }

            view
        };

        let slider_start = match self.slide_from {
            VideoViewMode::FullColor => px(0.0),
            VideoViewMode::YPlane => segment_width,
        };
        let slider_end = match self.view {
            VideoViewMode::FullColor => px(0.0),
            VideoViewMode::YPlane => segment_width,
        };

        let slider = div()
            .id(("video-view-slider", cx.entity_id()))
            .absolute()
            .top(px(0.0))
            .left(slider_start)
            .w(segment_width)
            .h_full()
            .rounded(segment_radius)
            .bg(hsla(0.0, 0.0, 1.0, if enabled { 0.16 } else { 0.08 }))
            .border_5()
            .border_color(hsla(0.0, 0.0, 0.0, 0.0))
            .with_animation(
                ("video-view-slider-anim", self.slide_token),
                Animation::new(Duration::from_millis(160)).with_easing(ease_out_quint()),
                move |slider, delta| {
                    let left = slider_start + (slider_end - slider_start) * delta;
                    slider.left(left)
                },
            );

        let view_group = div()
            .id(("video-view-toggle", cx.entity_id()))
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .h(toggle_height)
            .w(toggle_width)
            .border_1()
            .border_color(hsla(0.0, 0.0, 1.0, if enabled { 0.32 } else { 0.2 }))
            .rounded(segment_radius)
            .overflow_hidden()
            .bg(hsla(0.0, 0.0, 1.0, 0.04))
            .child(slider)
            .child(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .h_full()
                    .w_full()
                    .child(button(
                        "YUV",
                        self.view == VideoViewMode::FullColor,
                        VideoViewMode::FullColor,
                    ))
                    .child(button(
                        "Y",
                        self.view == VideoViewMode::YPlane,
                        VideoViewMode::YPlane,
                    )),
            );

        div()
            .id(("video-toolbar", cx.entity_id()))
            .flex()
            .items_center()
            .justify_between()
            .w_full()
            .h(px(29.0))
            .p(px(0.0))
            .text_xs()
            .text_color(text_color)
            .child(div().child("View"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(reset_button)
                    .child(view_group),
            )
    }
}

fn y_plane_preprocessor() -> FramePreprocessor {
    Arc::new(|_y_plane, uv_plane, _info| {
        uv_plane.fill(128);
        true
    })
}

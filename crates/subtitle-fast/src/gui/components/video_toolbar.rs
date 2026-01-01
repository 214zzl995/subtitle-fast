use std::sync::Arc;

use gpui::prelude::*;
use gpui::{Context, Render, Window, div, hsla, px};

use crate::gui::components::{FramePreprocessor, VideoPlayerControlHandle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoViewMode {
    FullColor,
    YPlane,
}

pub struct VideoToolbar {
    controls: Option<VideoPlayerControlHandle>,
    view: VideoViewMode,
}

impl VideoToolbar {
    pub fn new() -> Self {
        Self {
            controls: None,
            view: VideoViewMode::FullColor,
        }
    }

    pub fn set_controls(&mut self, controls: Option<VideoPlayerControlHandle>) {
        self.controls = controls;
        self.apply_view();
    }

    fn set_view(&mut self, view: VideoViewMode, cx: &mut Context<Self>) {
        if self.view == view {
            return;
        }
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
                .px(px(2.0))
                .py(px(3.0))
                .rounded(px(999.0))
                .border_1()
                .border_color(hsla(0.0, 0.0, 1.0, if active { 0.45 } else { 0.25 }))
                .text_xs()
                .text_color(text_color)
                .child(label);

            if active {
                view = view.bg(hsla(0.0, 0.0, 1.0, 0.12));
            }

            if enabled {
                view = view
                    .cursor_pointer()
                    .hover(|style| style.bg(hsla(0.0, 0.0, 1.0, 0.08)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.set_view(mode, cx);
                    }));
            }

            view
        };

        let view_group = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(button(
                "Full Color",
                self.view == VideoViewMode::FullColor,
                VideoViewMode::FullColor,
            ))
            .child(button(
                "Y Plane",
                self.view == VideoViewMode::YPlane,
                VideoViewMode::YPlane,
            ));

        div()
            .id(("video-toolbar", cx.entity_id()))
            .flex()
            .items_center()
            .justify_between()
            .w_full()
            .p(px(0.0))
            .text_sm()
            .text_color(text_color)
            .child(div().child("View"))
            .child(view_group)
    }
}

fn y_plane_preprocessor() -> FramePreprocessor {
    Arc::new(|_y_plane, uv_plane, _info| {
        uv_plane.fill(128);
        true
    })
}

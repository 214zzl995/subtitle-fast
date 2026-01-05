use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, BoxShadow, Context, Entity, FontWeight, InteractiveElement,
    Render, StatefulInteractiveElement, Window, div, ease_out_quint, hsla, px, rgb,
};

use crate::gui::components::{
    FramePreprocessor, VideoLumaHandle, VideoPlayerControlHandle, VideoRoiHandle, VideoRoiOverlay,
};
use crate::gui::icons::{Icon, icon_sm};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoViewMode {
    Yuv,
    Y,
}

pub struct VideoToolbar {
    controls: Option<VideoPlayerControlHandle>,
    roi_overlay: Option<Entity<VideoRoiOverlay>>,
    roi_handle: Option<VideoRoiHandle>,
    roi_visible: bool,
    luma_handle: Option<VideoLumaHandle>,
    view: VideoViewMode,
    slide_from: VideoViewMode,
    slide_token: u64,
}

impl VideoToolbar {
    pub fn new() -> Self {
        Self {
            controls: None,
            roi_overlay: None,
            roi_handle: None,
            roi_visible: true,
            luma_handle: None,
            view: VideoViewMode::Yuv,
            slide_from: VideoViewMode::Yuv,
            slide_token: 0,
        }
    }

    pub fn set_controls(&mut self, controls: Option<VideoPlayerControlHandle>) {
        self.controls = controls;
        self.apply_view();
    }

    pub fn set_roi_overlay(
        &mut self,
        overlay: Option<Entity<VideoRoiOverlay>>,
        cx: &mut Context<Self>,
    ) {
        self.roi_overlay = overlay;
        if let Some(roi_overlay) = self.roi_overlay.clone() {
            let visible = self.roi_visible;
            let _ = roi_overlay.update(cx, |overlay, cx| {
                overlay.set_visible(visible, cx);
            });
        }
    }

    pub fn set_luma_handle(&mut self, handle: Option<VideoLumaHandle>) {
        self.luma_handle = handle;
    }

    pub fn set_roi_handle(&mut self, handle: Option<VideoRoiHandle>) {
        self.roi_handle = handle;
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
            VideoViewMode::Yuv => controls.clear_preprocessor(),
            VideoViewMode::Y => controls.set_preprocessor(y_plane_preprocessor()),
        }
    }

    fn set_roi_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.roi_visible == visible {
            return;
        }
        self.roi_visible = visible;
        if let Some(roi_overlay) = self.roi_overlay.clone() {
            let _ = roi_overlay.update(cx, |overlay, cx| {
                overlay.set_visible(visible, cx);
            });
        }
        cx.notify();
    }

    fn toggle_roi_visible(&mut self, cx: &mut Context<Self>) {
        let visible = !self.roi_visible;
        self.set_roi_visible(visible, cx);
    }
}

impl Render for VideoToolbar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.controls.is_some();

        let container_bg = rgb(0x2b2b2b);
        let container_border = rgb(0x3a3a3a);
        let glider_bg = rgb(0x3f3f3f);
        let text_active_y = rgb(0xE0E0E0);
        let text_active_yuv = rgb(0xFFE259);
        let text_inactive = rgb(0x666666);
        let text_hover = rgb(0x888888);
        let hover_bg = rgb(0x3f3f3f);
        let info_text = rgb(0x9a9a9a);

        let luma_values = self.luma_handle.as_ref().map(|handle| handle.latest());
        let (luma_target, luma_delta) = if let Some(values) = luma_values {
            (values.target.to_string(), values.delta.to_string())
        } else {
            ("--".to_string(), "--".to_string())
        };
        let roi_text = self
            .roi_handle
            .as_ref()
            .map(|handle| handle.latest())
            .map(|roi| {
                format!(
                    "ROI: x{:.2} y{:.2} w{:.2} h{:.2}",
                    roi.x, roi.y, roi.width, roi.height
                )
            })
            .unwrap_or_else(|| "ROI: --".to_string());

        let info_group = div()
            .id(("video-toolbar-info", cx.entity_id()))
            .flex()
            .items_center()
            .gap(px(12.0))
            .text_color(info_text)
            .child(
                div()
                    .id(("video-toolbar-info-y", cx.entity_id()))
                    .child(format!("Y: {luma_target}")),
            )
            .child(
                div()
                    .id(("video-toolbar-info-tol", cx.entity_id()))
                    .child(format!("Tol: {luma_delta}")),
            )
            .child(
                div()
                    .id(("video-toolbar-info-roi", cx.entity_id()))
                    .child(roi_text),
            );

        let roi_visible = self.roi_visible;
        let roi_icon_color = if enabled {
            if roi_visible {
                text_active_y.into()
            } else {
                text_hover.into()
            }
        } else {
            text_inactive.into()
        };
        let roi_icon = if roi_visible { Icon::Eye } else { Icon::EyeOff };
        let roi_toggle_button = {
            let mut view = div()
                .id(("video-view-toggle-roi", cx.entity_id()))
                .flex()
                .items_center()
                .justify_center()
                .h(px(26.0))
                .w(px(26.0))
                .rounded(px(6.0))
                .bg(container_bg)
                .border_1()
                .border_color(container_border)
                .child(icon_sm(roi_icon, roi_icon_color).w(px(12.0)).h(px(12.0)));

            if enabled && self.roi_overlay.is_some() {
                view = view
                    .cursor_pointer()
                    .hover(|style| style.bg(hover_bg))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.toggle_roi_visible(cx);
                    }));
            }

            view
        };

        let reset_button = {
            let mut view = div()
                .id(("video-view-reset-roi", cx.entity_id()))
                .flex()
                .items_center()
                .justify_center()
                .h(px(26.0))
                .w(px(26.0))
                .rounded(px(6.0))
                .bg(container_bg)
                .border_1()
                .border_color(container_border)
                .child(
                    icon_sm(
                        Icon::RotateCcw,
                        if enabled {
                            text_active_y.into()
                        } else {
                            text_inactive.into()
                        },
                    )
                    .w(px(12.0))
                    .h(px(12.0)),
                );

            if enabled {
                if let Some(roi_overlay) = self.roi_overlay.clone() {
                    view = view
                        .cursor_pointer()
                        .hover(|style| style.bg(hover_bg))
                        .on_click(cx.listener(move |_, _event, _window, cx| {
                            let _ = roi_overlay.update(cx, |overlay, cx| {
                                overlay.reset_roi(cx);
                            });
                        }));
                }
            }

            view
        };

        let button_width = px(40.0);
        let button_height = px(20.0);
        let padding = px(2.0);

        let start_x = padding;
        let end_x = padding + button_width;

        let slider_start = match self.slide_from {
            VideoViewMode::Yuv => start_x,
            VideoViewMode::Y => end_x,
        };
        let slider_end = match self.view {
            VideoViewMode::Yuv => start_x,
            VideoViewMode::Y => end_x,
        };

        let slider = div()
            .id(("video-view-slider", cx.entity_id()))
            .absolute()
            .top(padding)
            .left(slider_start)
            .w(button_width)
            .h(button_height)
            .rounded(px(4.0))
            .bg(glider_bg)
            .shadow(vec![BoxShadow {
                color: hsla(0.0, 0.0, 0.0, 0.3),
                offset: gpui::point(px(0.0), px(1.0)),
                blur_radius: px(2.0),
                spread_radius: px(0.0),
            }])
            .with_animation(
                ("video-view-slider-anim", self.slide_token),
                Animation::new(Duration::from_millis(200)).with_easing(ease_out_quint()),
                move |slider, delta| {
                    let left = slider_start + (slider_end - slider_start) * delta;
                    slider.left(left)
                },
            );

        let toggle_label = |label: &'static str, mode: VideoViewMode, cx: &mut Context<Self>| {
            let is_active = self.view == mode;
            let target_color = if is_active {
                match mode {
                    VideoViewMode::Yuv => text_active_yuv,
                    VideoViewMode::Y => text_active_y,
                }
            } else {
                text_inactive
            };

            let mut el = div()
                .id(label)
                .flex()
                .items_center()
                .justify_center()
                .w(button_width)
                .h(button_height)
                .text_size(px(11.0))
                .font_weight(FontWeight::BOLD)
                .text_color(target_color)
                .child(label);

            if enabled && !is_active {
                el = el
                    .cursor_pointer()
                    .hover(|s| s.text_color(text_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_view(mode, cx);
                    }));
            } else if enabled && is_active {
                el = el.cursor_default();
            }

            el
        };

        // Container
        let toggle_container = div()
            .flex()
            .relative()
            .bg(container_bg)
            .border_1()
            .border_color(container_border)
            .rounded(px(6.0))
            .p(padding)
            .child(slider)
            .child(
                div()
                    .flex()
                    .relative()
                    .child(toggle_label("YUV", VideoViewMode::Yuv, cx))
                    .child(toggle_label("Y", VideoViewMode::Y, cx)),
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
            .child(info_group)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(roi_toggle_button)
                    .child(reset_button)
                    .child(
                        div()
                            .id(("video-toolbar-divider", cx.entity_id()))
                            .w(px(1.0))
                            .h(px(18.0))
                            .bg(container_border),
                    )
                    .child(toggle_container),
            )
    }
}

fn y_plane_preprocessor() -> FramePreprocessor {
    Arc::new(|_y_plane, uv_plane, _info| {
        uv_plane.fill(128);
        true
    })
}

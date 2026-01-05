use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, BoxShadow, Context, Entity, FontWeight, InteractiveElement,
    Render, Rgba, StatefulInteractiveElement, Subscription, Window, div, ease_out_quint, hsla, px,
    rgb,
};

use crate::gui::components::{
    ColorPicker, ColorPickerHandle, FramePreprocessor, VideoLumaControls, VideoLumaHandle,
    VideoPlayerControlHandle, VideoRoiHandle, VideoRoiOverlay,
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
    roi_subscription: Option<Subscription>,
    roi_visible: bool,
    luma_handle: Option<VideoLumaHandle>,
    luma_subscription: Option<Subscription>,
    color_picker: Option<Entity<ColorPicker>>,
    color_handle: Option<ColorPickerHandle>,
    color_subscription: Option<Subscription>,
    view: VideoViewMode,
    slide_from: VideoViewMode,
    slide_token: u64,
    highlight_visible: bool,
}

impl VideoToolbar {
    pub fn new() -> Self {
        Self {
            controls: None,
            roi_overlay: None,
            roi_handle: None,
            roi_subscription: None,
            roi_visible: true,
            luma_handle: None,
            luma_subscription: None,
            color_picker: None,
            color_handle: None,
            color_subscription: None,
            view: VideoViewMode::Yuv,
            slide_from: VideoViewMode::Yuv,
            slide_token: 0,
            highlight_visible: false,
        }
    }

    pub fn set_controls(
        &mut self,
        controls: Option<VideoPlayerControlHandle>,
        cx: &mut Context<Self>,
    ) {
        self.controls = controls;
        self.sync_frame_preprocessor();
        if let Some(color_picker) = self.color_picker.clone() {
            let enabled = self.controls.is_some();
            let _ = color_picker.update(cx, |picker, cx| {
                picker.set_enabled(enabled, cx);
            });
        }
    }

    pub fn set_roi_overlay(
        &mut self,
        overlay: Option<Entity<VideoRoiOverlay>>,
        cx: &mut Context<Self>,
    ) {
        self.roi_overlay = overlay;
        self.roi_subscription = None;
        if let Some(roi_overlay) = self.roi_overlay.clone() {
            self.roi_subscription = Some(cx.observe(&roi_overlay, |this, _, cx| {
                this.handle_roi_update(cx);
            }));
            let visible = self.roi_visible;
            let _ = roi_overlay.update(cx, |overlay, cx| {
                overlay.set_visible(visible, cx);
            });
        }
    }

    pub fn set_luma_controls(
        &mut self,
        handle: Option<VideoLumaHandle>,
        controls: Option<Entity<VideoLumaControls>>,
        cx: &mut Context<Self>,
    ) {
        self.luma_handle = handle;
        self.luma_subscription = None;
        if let Some(controls) = controls {
            self.luma_subscription = Some(cx.observe(&controls, |this, _, cx| {
                this.handle_luma_update(cx);
            }));
        }
        self.sync_frame_preprocessor();
    }

    pub fn set_color_picker(
        &mut self,
        picker: Option<Entity<ColorPicker>>,
        handle: Option<ColorPickerHandle>,
        cx: &mut Context<Self>,
    ) {
        self.color_picker = picker;
        self.color_handle = handle;
        self.color_subscription = None;
        if let Some(color_picker) = self.color_picker.clone() {
            self.color_subscription = Some(cx.observe(&color_picker, |this, _, cx| {
                this.handle_color_update(cx);
            }));
            let enabled = self.controls.is_some();
            let _ = color_picker.update(cx, |picker, cx| {
                picker.set_enabled(enabled, cx);
            });
        }
        self.sync_frame_preprocessor();
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
        self.sync_frame_preprocessor();
        cx.notify();
    }

    fn sync_frame_preprocessor(&self) {
        let Some(controls) = self.controls.as_ref() else {
            return;
        };
        if self.highlight_visible {
            if let (Some(luma_handle), Some(color_handle)) =
                (self.luma_handle.clone(), self.color_handle.clone())
            {
                let grayscale = self.view == VideoViewMode::Y;
                controls.set_preprocessor(luma_highlight_preprocessor(
                    luma_handle,
                    color_handle,
                    self.roi_handle.clone(),
                    grayscale,
                ));
                return;
            }
        }

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

    fn set_highlight_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.highlight_visible == visible {
            return;
        }
        self.highlight_visible = visible;
        self.sync_frame_preprocessor();
        cx.notify();
    }

    fn toggle_highlight_visible(&mut self, cx: &mut Context<Self>) {
        let visible = !self.highlight_visible;
        self.set_highlight_visible(visible, cx);
    }

    fn handle_luma_update(&mut self, cx: &mut Context<Self>) {
        if self.highlight_visible {
            self.sync_frame_preprocessor();
        }
        cx.notify();
    }

    fn handle_color_update(&mut self, _cx: &mut Context<Self>) {
        if self.highlight_visible {
            self.sync_frame_preprocessor();
        }
    }

    fn handle_roi_update(&mut self, cx: &mut Context<Self>) {
        if self.roi_handle.is_none() {
            return;
        }
        if self.highlight_visible {
            self.sync_frame_preprocessor();
        }
        cx.notify();
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
        let info_text = hsla(0.0, 0.0, 100.0, 0.3);

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
                    "x{:.3} y{:.3} w{:.3} h{:.3}",
                    roi.x, roi.y, roi.width, roi.height
                )
            })
            .unwrap_or_else(|| "--".to_string());

        let info_group = div()
            .id(("video-toolbar-info", cx.entity_id()))
            .flex()
            .flex_col()
            .justify_center()
            .items_start()
            .gap(px(1.0))
            .text_size(px(10.0))
            .line_height(px(10.0))
            .text_color(info_text)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(icon_sm(Icon::Sun, info_text.into()).w(px(10.0)).h(px(10.0)))
                    .child(format!("Y: {luma_target}  Tol: {luma_delta}")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        icon_sm(Icon::Crosshair, info_text.into())
                            .w(px(10.0))
                            .h(px(10.0)),
                    )
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

        let highlight_visible = self.highlight_visible;
        let highlight_icon_color = if enabled {
            if highlight_visible {
                text_active_y.into()
            } else {
                text_hover.into()
            }
        } else {
            text_inactive.into()
        };
        let highlight_toggle_button = {
            let mut view = div()
                .id(("video-view-toggle-highlight", cx.entity_id()))
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
                    icon_sm(Icon::Sparkles, highlight_icon_color)
                        .w(px(12.0))
                        .h(px(12.0)),
                );

            if enabled && self.luma_handle.is_some() && self.color_handle.is_some() {
                view = view
                    .cursor_pointer()
                    .hover(|style| style.bg(hover_bg))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.toggle_highlight_visible(cx);
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

        let mut control_group = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(roi_toggle_button)
            .child(highlight_toggle_button)
            .child(reset_button);

        if let Some(color_picker) = self.color_picker.clone() {
            control_group = control_group.child(color_picker);
        }

        control_group = control_group
            .child(
                div()
                    .id(("video-toolbar-divider", cx.entity_id()))
                    .w(px(1.0))
                    .h(px(18.0))
                    .bg(container_border),
            )
            .child(toggle_container);

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
            .child(control_group)
    }
}

fn y_plane_preprocessor() -> FramePreprocessor {
    Arc::new(|_y_plane, uv_plane, _info| {
        uv_plane.fill(128);
        true
    })
}

fn luma_highlight_preprocessor(
    luma_handle: VideoLumaHandle,
    color_handle: ColorPickerHandle,
    roi_handle: Option<VideoRoiHandle>,
    grayscale: bool,
) -> FramePreprocessor {
    Arc::new(move |y_plane, uv_plane, info| {
        let values = luma_handle.latest();
        let target_min = values.target.saturating_sub(values.delta);
        let target_max = values.target.saturating_add(values.delta);
        let (target_y, target_u, target_v) = rgb_to_nv12(color_handle.latest());

        if grayscale {
            uv_plane.fill(128);
        }

        let width = info.width as usize;
        let height = info.height as usize;
        if width == 0 || height == 0 {
            return true;
        }

        let (roi_left, roi_top, roi_right, roi_bottom) = if let Some(handle) = roi_handle.as_ref() {
            let roi = handle.latest();
            let left = roi.x.clamp(0.0, 1.0);
            let top = roi.y.clamp(0.0, 1.0);
            let right = (roi.x + roi.width).clamp(left, 1.0);
            let bottom = (roi.y + roi.height).clamp(top, 1.0);
            let left_px = (left * width as f32).ceil() as usize;
            let right_px = (right * width as f32).floor() as usize;
            let top_px = (top * height as f32).ceil() as usize;
            let bottom_px = (bottom * height as f32).floor() as usize;
            (
                left_px.min(width),
                top_px.min(height),
                right_px.min(width),
                bottom_px.min(height),
            )
        } else {
            (0, 0, width, height)
        };

        if roi_right <= roi_left || roi_bottom <= roi_top {
            return true;
        }

        let blocks_w = (width + 1) / 2;
        let blocks_h = (height + 1) / 2;

        for by in 0..blocks_h {
            let y0 = by * 2;
            let y1 = y0 + 1;
            let row0 = y0 * info.y_stride;
            let row1 = y1 * info.y_stride;
            let uv_row = by * info.uv_stride;
            for bx in 0..blocks_w {
                let x0 = bx * 2;
                let x1 = x0 + 1;
                let mut block_inside = true;
                for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
                    if x < width && y < height {
                        if x < roi_left || x >= roi_right || y < roi_top || y >= roi_bottom {
                            block_inside = false;
                            break;
                        }
                    }
                }
                if !block_inside {
                    continue;
                }
                let mut hit = false;

                if y0 < height && x0 < width {
                    let idx = row0 + x0;
                    let value = y_plane[idx];
                    if value >= target_min && value <= target_max {
                        y_plane[idx] = target_y;
                        hit = true;
                    }
                }
                if y0 < height && x1 < width {
                    let idx = row0 + x1;
                    let value = y_plane[idx];
                    if value >= target_min && value <= target_max {
                        y_plane[idx] = target_y;
                        hit = true;
                    }
                }
                if y1 < height && x0 < width {
                    let idx = row1 + x0;
                    let value = y_plane[idx];
                    if value >= target_min && value <= target_max {
                        y_plane[idx] = target_y;
                        hit = true;
                    }
                }
                if y1 < height && x1 < width {
                    let idx = row1 + x1;
                    let value = y_plane[idx];
                    if value >= target_min && value <= target_max {
                        y_plane[idx] = target_y;
                        hit = true;
                    }
                }

                if hit {
                    let uv_index = uv_row + bx * 2;
                    if uv_index + 1 < uv_plane.len() {
                        uv_plane[uv_index] = target_u;
                        uv_plane[uv_index + 1] = target_v;
                    }
                }
            }
        }

        true
    })
}

fn rgb_to_nv12(color: Rgba) -> (u8, u8, u8) {
    let r = (color.r.clamp(0.0, 1.0) * 255.0).round();
    let g = (color.g.clamp(0.0, 1.0) * 255.0).round();
    let b = (color.b.clamp(0.0, 1.0) * 255.0).round();

    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let u = -0.168_736 * r - 0.331_264 * g + 0.5 * b + 128.0;
    let v = 0.5 * r - 0.418_688 * g - 0.081_312 * b + 128.0;

    (
        y.round().clamp(0.0, 255.0) as u8,
        u.round().clamp(0.0, 255.0) as u8,
        v.round().clamp(0.0, 255.0) as u8,
    )
}

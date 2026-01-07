use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, BoxShadow, Context, Entity, FontWeight, InteractiveElement,
    Render, Rgba, StatefulInteractiveElement, Subscription, Window, div, ease_out_quint, hsla, px,
    rgb,
};

use crate::gui::components::{
    ColorPicker, ColorPickerHandle, FramePreprocessor, Nv12FrameInfo, VideoLumaControls,
    VideoLumaHandle, VideoLumaValues, VideoPlayerControlHandle, VideoRoiHandle, VideoRoiOverlay,
};
use crate::gui::icons::{Icon, icon_sm};
use subtitle_fast_types::VideoFrame;
use subtitle_fast_validator::subtitle_detection::{
    LumaBandConfig, SubtitleDetectionConfig, SubtitleDetectionResult, SubtitleDetector,
    SubtitleDetectorKind, build_detector,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoViewMode {
    Yuv,
    Y,
}

const VIEW_PREPROCESSOR_KEY: &str = "video-view";

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
    validator_overlay_visible: bool,
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
            validator_overlay_visible: false,
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
        self.sync_frame_preprocessor();
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
        let overlay_enabled = self.highlight_visible || self.validator_overlay_visible;
        if overlay_enabled {
            if let (Some(luma_handle), Some(color_handle), Some(roi_handle)) = (
                self.luma_handle.clone(),
                self.color_handle.clone(),
                self.roi_handle.clone(),
            ) {
                let grayscale = self.view == VideoViewMode::Y;
                controls.set_preprocessor(
                    VIEW_PREPROCESSOR_KEY,
                    frame_overlay_preprocessor(
                        luma_handle,
                        color_handle,
                        roi_handle,
                        OverlayOptions {
                            highlight: self.highlight_visible,
                            validator: self.validator_overlay_visible,
                            grayscale,
                        },
                    ),
                );
                return;
            }
        }

        match self.view {
            VideoViewMode::Yuv => controls.remove_preprocessor(VIEW_PREPROCESSOR_KEY),
            VideoViewMode::Y => {
                controls.set_preprocessor(VIEW_PREPROCESSOR_KEY, y_plane_preprocessor());
            }
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

    fn set_validator_overlay_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.validator_overlay_visible == visible {
            return;
        }
        self.validator_overlay_visible = visible;
        self.sync_frame_preprocessor();
        cx.notify();
    }

    fn toggle_validator_overlay_visible(&mut self, cx: &mut Context<Self>) {
        let visible = !self.validator_overlay_visible;
        self.set_validator_overlay_visible(visible, cx);
    }

    fn handle_luma_update(&mut self, cx: &mut Context<Self>) {
        if self.highlight_visible || self.validator_overlay_visible {
            self.sync_frame_preprocessor();
        }
        cx.notify();
    }

    fn handle_color_update(&mut self, _cx: &mut Context<Self>) {
        if self.highlight_visible || self.validator_overlay_visible {
            self.sync_frame_preprocessor();
        }
    }

    fn handle_roi_update(&mut self, cx: &mut Context<Self>) {
        if self.roi_handle.is_none() {
            return;
        }
        if self.highlight_visible || self.validator_overlay_visible {
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

        let validator_visible = self.validator_overlay_visible;
        let validator_icon_color = if enabled {
            if validator_visible {
                text_active_y.into()
            } else {
                text_hover.into()
            }
        } else {
            text_inactive.into()
        };
        let validator_toggle_button = {
            let mut view = div()
                .id(("video-view-toggle-validator", cx.entity_id()))
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
                    icon_sm(Icon::Frame, validator_icon_color)
                        .w(px(12.0))
                        .h(px(12.0)),
                );

            if enabled
                && self.luma_handle.is_some()
                && self.color_handle.is_some()
                && self.roi_handle.is_some()
            {
                view = view
                    .cursor_pointer()
                    .hover(|style| style.bg(hover_bg))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.toggle_validator_overlay_visible(cx);
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
            .child(validator_toggle_button)
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

#[derive(Clone, Copy)]
struct OverlayOptions {
    highlight: bool,
    validator: bool,
    grayscale: bool,
}

#[derive(Clone, Copy)]
struct RoiBounds {
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
}

struct ValidatorOverlayState {
    detector: Option<Box<dyn SubtitleDetector>>,
    dims: Option<(usize, usize, usize)>,
    roi: Option<subtitle_fast_types::RoiConfig>,
    luma_band: Option<(u8, u8)>,
    init_error_logged: bool,
}

impl ValidatorOverlayState {
    fn new() -> Self {
        Self {
            detector: None,
            dims: None,
            roi: None,
            luma_band: None,
            init_error_logged: false,
        }
    }

    fn detect(
        &mut self,
        frame: &VideoFrame,
        roi: subtitle_fast_types::RoiConfig,
        luma: VideoLumaValues,
    ) -> Option<SubtitleDetectionResult> {
        let dims = (
            frame.width() as usize,
            frame.height() as usize,
            frame.stride(),
        );
        let luma_band = (luma.target, luma.delta);
        let needs_rebuild = self.detector.is_none()
            || self.dims != Some(dims)
            || self.roi != Some(roi)
            || self.luma_band != Some(luma_band);
        if needs_rebuild {
            self.dims = Some(dims);
            self.roi = Some(roi);
            self.luma_band = Some(luma_band);
            self.init_error_logged = false;
            let mut config = SubtitleDetectionConfig::for_frame(dims.0, dims.1, dims.2);
            config.roi = roi;
            config.luma_band = LumaBandConfig {
                target: luma.target,
                delta: luma.delta,
            };
            match build_detector(SubtitleDetectorKind::ProjectionBand, config) {
                Ok(detector) => {
                    self.detector = Some(detector);
                }
                Err(err) => {
                    self.detector = None;
                    if !self.init_error_logged {
                        eprintln!("validator overlay detector init failed: {err}");
                        self.init_error_logged = true;
                    }
                }
            }
        }

        let Some(detector) = self.detector.as_ref() else {
            return None;
        };
        match detector.detect(frame) {
            Ok(result) => Some(result),
            Err(err) => {
                if !self.init_error_logged {
                    eprintln!("validator overlay detection failed: {err}");
                    self.init_error_logged = true;
                }
                None
            }
        }
    }
}

fn frame_overlay_preprocessor(
    luma_handle: VideoLumaHandle,
    color_handle: ColorPickerHandle,
    roi_handle: VideoRoiHandle,
    options: OverlayOptions,
) -> FramePreprocessor {
    let validator_state = Arc::new(Mutex::new(ValidatorOverlayState::new()));
    Arc::new(move |y_plane, uv_plane, info| {
        let width = info.width as usize;
        let height = info.height as usize;
        if width == 0 || height == 0 {
            return true;
        }

        let roi = roi_handle.latest();
        let Some(roi_bounds) = roi_bounds(roi, width, height) else {
            if options.grayscale {
                uv_plane.fill(128);
            }
            return true;
        };

        let values = luma_handle.latest();
        let (target_y, target_u, target_v) = rgb_to_nv12(color_handle.latest());

        let detection = if options.validator {
            let frame = VideoFrame::from_nv12_owned(
                info.width,
                info.height,
                info.y_stride,
                info.uv_stride,
                None,
                None,
                y_plane.to_vec(),
                uv_plane.to_vec(),
            )
            .ok();
            if let Some(frame) = frame {
                let mut state = validator_state
                    .lock()
                    .expect("validator overlay mutex poisoned");
                state.detect(&frame, roi, values)
            } else {
                None
            }
        } else {
            None
        };

        if options.grayscale {
            uv_plane.fill(128);
        }

        if options.highlight {
            let target_min = values.target.saturating_sub(values.delta);
            let target_max = values.target.saturating_add(values.delta);
            apply_luma_highlight(
                y_plane, uv_plane, info, roi_bounds, target_min, target_max, target_y, target_u,
                target_v,
            );
        }

        if let Some(result) = detection {
            if let Some(bounds) = detection_bounds(&result, width, height) {
                if let Some(bounds) = clamp_bounds_to_roi(bounds, roi_bounds) {
                    draw_nv12_rect_outline(
                        y_plane, uv_plane, info, bounds, roi_bounds, target_y, target_u, target_v,
                    );
                }
            }
        }

        true
    })
}

fn roi_bounds(
    roi: subtitle_fast_types::RoiConfig,
    width: usize,
    height: usize,
) -> Option<RoiBounds> {
    if width == 0 || height == 0 {
        return None;
    }
    let left = roi.x.clamp(0.0, 1.0);
    let top = roi.y.clamp(0.0, 1.0);
    let right = (roi.x + roi.width).clamp(left, 1.0);
    let bottom = (roi.y + roi.height).clamp(top, 1.0);
    let left_px = (left * width as f32).ceil() as usize;
    let right_px = (right * width as f32).floor() as usize;
    let top_px = (top * height as f32).ceil() as usize;
    let bottom_px = (bottom * height as f32).floor() as usize;
    if right_px <= left_px || bottom_px <= top_px {
        return None;
    }
    Some(RoiBounds {
        left: left_px.min(width),
        top: top_px.min(height),
        right: right_px.min(width),
        bottom: bottom_px.min(height),
    })
}

fn detection_bounds(
    result: &SubtitleDetectionResult,
    width: usize,
    height: usize,
) -> Option<RoiBounds> {
    let mut left = f32::INFINITY;
    let mut top = f32::INFINITY;
    let mut right = 0.0_f32;
    let mut bottom = 0.0_f32;
    for region in &result.regions {
        left = left.min(region.x);
        top = top.min(region.y);
        right = right.max(region.x + region.width);
        bottom = bottom.max(region.y + region.height);
    }
    if !left.is_finite() || !top.is_finite() || right <= left || bottom <= top {
        return None;
    }
    let left_px = left.floor().max(0.0) as usize;
    let top_px = top.floor().max(0.0) as usize;
    let right_px = right.ceil().min(width as f32) as usize;
    let bottom_px = bottom.ceil().min(height as f32) as usize;
    if right_px <= left_px || bottom_px <= top_px {
        return None;
    }
    Some(RoiBounds {
        left: left_px,
        top: top_px,
        right: right_px,
        bottom: bottom_px,
    })
}

fn clamp_bounds_to_roi(bounds: RoiBounds, roi: RoiBounds) -> Option<RoiBounds> {
    let left = bounds.left.max(roi.left);
    let top = bounds.top.max(roi.top);
    let right = bounds.right.min(roi.right);
    let bottom = bounds.bottom.min(roi.bottom);
    if right <= left || bottom <= top {
        return None;
    }
    Some(RoiBounds {
        left,
        top,
        right,
        bottom,
    })
}

fn block_inside_roi(roi: RoiBounds, x0: usize, y0: usize, width: usize, height: usize) -> bool {
    if x0 + 1 >= width || y0 + 1 >= height {
        return false;
    }
    x0 >= roi.left && x0 + 1 < roi.right && y0 >= roi.top && y0 + 1 < roi.bottom
}

fn apply_luma_highlight(
    y_plane: &mut [u8],
    uv_plane: &mut [u8],
    info: Nv12FrameInfo,
    roi: RoiBounds,
    target_min: u8,
    target_max: u8,
    target_y: u8,
    target_u: u8,
    target_v: u8,
) {
    let width = info.width as usize;
    let height = info.height as usize;
    let blocks_w = (width + 1) / 2;
    let blocks_h = (height + 1) / 2;

    for by in 0..blocks_h {
        let y0 = by * 2;
        let y1 = y0 + 1;
        if y0 >= height {
            break;
        }
        let row0 = y0 * info.y_stride;
        let row1 = y1 * info.y_stride;
        let uv_row = by * info.uv_stride;
        for bx in 0..blocks_w {
            let x0 = bx * 2;
            let x1 = x0 + 1;
            if x0 >= width {
                break;
            }
            if !block_inside_roi(roi, x0, y0, width, height) {
                continue;
            }
            let mut hit = false;

            if y0 < height && x0 < width {
                let idx = row0 + x0;
                if idx < y_plane.len() {
                    let value = y_plane[idx];
                    if value >= target_min && value <= target_max {
                        y_plane[idx] = target_y;
                        hit = true;
                    }
                }
            }
            if y0 < height && x1 < width {
                let idx = row0 + x1;
                if idx < y_plane.len() {
                    let value = y_plane[idx];
                    if value >= target_min && value <= target_max {
                        y_plane[idx] = target_y;
                        hit = true;
                    }
                }
            }
            if y1 < height && x0 < width {
                let idx = row1 + x0;
                if idx < y_plane.len() {
                    let value = y_plane[idx];
                    if value >= target_min && value <= target_max {
                        y_plane[idx] = target_y;
                        hit = true;
                    }
                }
            }
            if y1 < height && x1 < width {
                let idx = row1 + x1;
                if idx < y_plane.len() {
                    let value = y_plane[idx];
                    if value >= target_min && value <= target_max {
                        y_plane[idx] = target_y;
                        hit = true;
                    }
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
}

fn draw_nv12_rect_outline(
    y_plane: &mut [u8],
    uv_plane: &mut [u8],
    info: Nv12FrameInfo,
    bounds: RoiBounds,
    roi: RoiBounds,
    target_y: u8,
    target_u: u8,
    target_v: u8,
) {
    let width = info.width as usize;
    let height = info.height as usize;
    let thickness = 2usize;
    let left = bounds.left.min(width);
    let right = bounds.right.min(width);
    let top = bounds.top.min(height);
    let bottom = bounds.bottom.min(height);
    if right <= left || bottom <= top {
        return;
    }

    let top_end = (top + thickness).min(bottom);
    let bottom_start = bottom.saturating_sub(thickness).max(top);
    let left_end = (left + thickness).min(right);
    let right_start = right.saturating_sub(thickness).max(left);

    let mut draw_stripe = |x_start: usize, x_end: usize, y_start: usize, y_end: usize| {
        let blocks_w = (width + 1) / 2;
        let blocks_h = (height + 1) / 2;
        for by in 0..blocks_h {
            let y0 = by * 2;
            if y0 >= height {
                break;
            }
            let y1 = y0 + 1;
            let block_top = y0;
            let block_bottom = (y1 + 1).min(height);
            if block_bottom <= y_start || block_top >= y_end {
                continue;
            }
            let row0 = y0 * info.y_stride;
            let row1 = y1 * info.y_stride;
            let uv_row = by * info.uv_stride;
            for bx in 0..blocks_w {
                let x0 = bx * 2;
                if x0 >= width {
                    break;
                }
                let x1 = x0 + 1;
                let block_left = x0;
                let block_right = (x1 + 1).min(width);
                if block_right <= x_start || block_left >= x_end {
                    continue;
                }
                if !block_inside_roi(roi, x0, y0, width, height) {
                    continue;
                }

                for (x, y, row) in [
                    (x0, y0, row0),
                    (x1, y0, row0),
                    (x0, y1, row1),
                    (x1, y1, row1),
                ] {
                    if x < width
                        && y < height
                        && x >= x_start
                        && x < x_end
                        && y >= y_start
                        && y < y_end
                    {
                        let idx = row + x;
                        if idx < y_plane.len() {
                            y_plane[idx] = target_y;
                        }
                    }
                }

                let uv_index = uv_row + bx * 2;
                if uv_index + 1 < uv_plane.len() {
                    uv_plane[uv_index] = target_u;
                    uv_plane[uv_index + 1] = target_v;
                }
            }
        }
    };

    if top_end > top {
        draw_stripe(left, right, top, top_end);
    }
    if bottom > bottom_start {
        draw_stripe(left, right, bottom_start, bottom);
    }
    if left_end > left {
        draw_stripe(left, left_end, top, bottom);
    }
    if right > right_start {
        draw_stripe(right_start, right, top, bottom);
    }
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

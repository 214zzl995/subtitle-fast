use gpui::prelude::*;
use gpui::{
    Bounds, Context, CursorStyle, DispatchPhase, Entity, IsZero, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point, Render, Rgba, Subscription, Window,
    canvas, div, hsla, point, px, size,
};
use subtitle_fast_types::RoiConfig;
use tokio::sync::watch;

use crate::gui::components::{ColorPicker, ColorPickerHandle, VideoPlayerInfoHandle};

const DEFAULT_LEFT_GAP: f32 = 0.20;
const DEFAULT_RIGHT_GAP: f32 = 0.20;
const DEFAULT_BOTTOM_GAP: f32 = 0.00;
const DEFAULT_HEIGHT: f32 = 0.20;
const BORDER_WIDTH: f32 = 1.5;
const DASH_LENGTH: f32 = 8.0;
const DASH_GAP: f32 = 3.0;
const HANDLE_SIZE: f32 = 12.0;
const MIN_ROI_HEIGHT_FRACTION: f32 = 0.05;
const MIN_ROI_WIDTH_FRACTION: f32 = 0.05;

#[derive(Clone)]
pub struct VideoRoiHandle {
    receiver: watch::Receiver<RoiConfig>,
}

impl VideoRoiHandle {
    pub fn subscribe(&self) -> watch::Receiver<RoiConfig> {
        self.receiver.clone()
    }

    pub fn latest(&self) -> RoiConfig {
        *self.receiver.borrow()
    }
}

#[derive(Clone, Copy, Debug)]
enum DragCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug)]
struct DragState {
    corner: DragCorner,
    origin: Point<Pixels>,
    roi: RoiConfig,
}

pub struct VideoRoiOverlay {
    info: Option<VideoPlayerInfoHandle>,
    container_bounds: Option<Bounds<Pixels>>,
    picture_bounds: Option<Bounds<Pixels>>,
    roi: RoiConfig,
    dragging: Option<DragState>,
    visible: bool,
    sender: watch::Sender<RoiConfig>,
    color: Rgba,
    color_handle: Option<ColorPickerHandle>,
    color_subscription: Option<Subscription>,
}

impl VideoRoiOverlay {
    pub fn new() -> (Self, VideoRoiHandle) {
        let roi = default_roi();
        let (sender, receiver) = watch::channel(roi);
        let color = Rgba::from(hsla(0.12, 0.95, 0.6, 0.95));
        (
            Self {
                info: None,
                container_bounds: None,
                picture_bounds: None,
                roi,
                dragging: None,
                visible: true,
                sender,
                color,
                color_handle: None,
                color_subscription: None,
            },
            VideoRoiHandle { receiver },
        )
    }

    pub fn reset_roi(&mut self, cx: &mut Context<Self>) {
        self.dragging = None;
        self.roi = default_roi();
        let _ = self.sender.send(self.roi);
        cx.notify();
    }

    pub fn set_roi(&mut self, roi: RoiConfig, cx: &mut Context<Self>) {
        self.dragging = None;
        self.roi = roi;
        let _ = self.sender.send(self.roi);
        cx.notify();
    }

    pub fn set_info_handle(
        &mut self,
        info: Option<VideoPlayerInfoHandle>,
        initial_roi: Option<RoiConfig>,
        cx: &mut Context<Self>,
    ) {
        self.info = info;
        self.container_bounds = None;
        self.picture_bounds = None;
        self.dragging = None;
        self.roi = initial_roi.unwrap_or_else(default_roi);
        let _ = self.sender.send(self.roi);
        cx.notify();
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if !self.visible {
            self.dragging = None;
        }
        cx.notify();
    }

    pub fn set_color_picker(
        &mut self,
        picker: Option<Entity<ColorPicker>>,
        handle: Option<ColorPickerHandle>,
        cx: &mut Context<Self>,
    ) {
        self.color_handle = handle;
        self.color_subscription = None;
        if let Some(picker) = picker {
            self.color_subscription = Some(cx.observe(&picker, |this, _, cx| {
                this.update_color(cx);
            }));
        }
        self.update_color(cx);
    }

    fn update_color(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.color_handle.as_ref() else {
            return;
        };
        let next = handle.latest();
        if self.color != next {
            self.color = next;
            cx.notify();
        }
    }

    fn update_container_bounds(&mut self, bounds: Option<Bounds<Pixels>>) -> bool {
        if self.container_bounds != bounds {
            self.container_bounds = bounds;
            return true;
        }
        false
    }

    fn update_picture_bounds(&mut self) {
        let Some(container) = self.container_bounds else {
            self.picture_bounds = None;
            return;
        };

        let Some(info) = self.info.as_ref() else {
            self.picture_bounds = None;
            return;
        };

        let snapshot = info.snapshot();
        let (Some(width), Some(height)) = (snapshot.metadata.width, snapshot.metadata.height)
        else {
            self.picture_bounds = None;
            return;
        };
        if width == 0 || height == 0 {
            self.picture_bounds = None;
            return;
        }

        let container_w: f32 = container.size.width.into();
        let container_h: f32 = container.size.height.into();
        if container_w <= 0.0 || container_h <= 0.0 {
            self.picture_bounds = None;
            return;
        }

        let video_aspect = width as f32 / height as f32;
        if !video_aspect.is_finite() || video_aspect <= 0.0 {
            self.picture_bounds = None;
            return;
        }

        let container_aspect = container_w / container_h;
        let picture = if container_aspect > video_aspect {
            let height_px = container_h;
            let width_px = height_px * video_aspect;
            let offset_x = (container_w - width_px) * 0.5;
            Bounds {
                origin: point(container.origin.x + px(offset_x), container.origin.y),
                size: size(px(width_px), px(height_px)),
            }
        } else {
            let width_px = container_w;
            let height_px = width_px / video_aspect;
            let offset_y = (container_h - height_px) * 0.5;
            Bounds {
                origin: point(container.origin.x, container.origin.y + px(offset_y)),
                size: size(px(width_px), px(height_px)),
            }
        };

        self.picture_bounds = Some(picture);
    }

    fn begin_drag(&mut self, corner: DragCorner, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.dragging = Some(DragState {
            corner,
            origin: position,
            roi: self.roi,
        });
        cx.notify();
    }

    fn update_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.dragging else {
            return;
        };
        let Some(picture) = self.picture_bounds else {
            return;
        };
        if picture.size.width.is_zero() || picture.size.height.is_zero() {
            return;
        }

        let dx = (position.x - drag.origin.x) / picture.size.width;
        let dy = (position.y - drag.origin.y) / picture.size.height;

        let (mut left, mut top, mut right, mut bottom) = roi_edges(drag.roi);
        let min_height = min_roi_height(picture);
        let min_width = min_roi_width(picture);

        match drag.corner {
            DragCorner::TopLeft => {
                let max_left = (right - min_width).max(0.0);
                left = (left + dx).clamp(0.0, max_left);
                let max_top = (bottom - min_height).max(0.0);
                top = (top + dy).clamp(0.0, max_top);
            }
            DragCorner::TopRight => {
                let min_right = (left + min_width).min(1.0);
                right = (right + dx).clamp(min_right, 1.0);
                let max_top = (bottom - min_height).max(0.0);
                top = (top + dy).clamp(0.0, max_top);
            }
            DragCorner::BottomLeft => {
                let max_left = (right - min_width).max(0.0);
                left = (left + dx).clamp(0.0, max_left);
                let min_bottom = (top + min_height).min(1.0);
                bottom = (bottom + dy).clamp(min_bottom, 1.0);
            }
            DragCorner::BottomRight => {
                let min_right = (left + min_width).min(1.0);
                right = (right + dx).clamp(min_right, 1.0);
                let min_bottom = (top + min_height).min(1.0);
                bottom = (bottom + dy).clamp(min_bottom, 1.0);
            }
        }

        let next = RoiConfig {
            x: left,
            y: top,
            width: (right - left).max(0.0),
            height: (bottom - top).max(0.0),
        };

        if next != self.roi {
            self.roi = next;
            let _ = self.sender.send(self.roi);
            cx.notify();
        }
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.dragging.is_some() {
            self.dragging = None;
            cx.notify();
        }
    }
}

impl Render for VideoRoiOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.visible {
            if let Some(dragging) = self.dragging {
                window.set_window_cursor_style(cursor_for_corner(dragging.corner));
                let handle = cx.entity();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let _ = handle.update(cx, |this, cx| {
                        this.update_drag(event.position, cx);
                    });
                    window.refresh();
                });

                let handle = cx.entity();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    if event.button == MouseButton::Left {
                        let _ = handle.update(cx, |this, cx| {
                            this.end_drag(cx);
                        });
                        window.refresh();
                    }
                });
            }
        }

        let handle = cx.entity();
        let mut root = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .on_children_prepainted(move |bounds, _window, cx| {
                let bounds = bounds.first().copied();
                let _ = handle.update(cx, |this, cx| {
                    if this.update_container_bounds(bounds) {
                        cx.notify();
                    }
                });
            })
            .id(("video-roi-overlay", cx.entity_id()))
            .child(
                div()
                    .size_full()
                    .id(("video-roi-overlay-bounds", cx.entity_id())),
            );

        if !self.visible {
            return root;
        }

        self.update_picture_bounds();

        let has_frame = self.has_frame();

        let Some(container) = self.container_bounds else {
            return root;
        };
        let Some(picture) = self.picture_bounds else {
            return root;
        };
        if !has_frame {
            return root;
        }

        let local_origin = point(
            picture.origin.x - container.origin.x,
            picture.origin.y - container.origin.y,
        );

        let (left, top, right, bottom) = roi_edges(self.roi);
        let left_px = local_origin.x + picture.size.width * left;
        let top_px = local_origin.y + picture.size.height * top;
        let width_px = picture.size.width * (right - left);
        let height_px = picture.size.height * (bottom - top);

        let border_color = self.color;

        let stroke_width = px(BORDER_WIDTH);
        let stroke_inset = stroke_width * 0.5;
        let handle_radius = px(HANDLE_SIZE / 2.0);
        let handle_gap = handle_radius;
        if width_px <= stroke_inset * 2.0 || height_px <= stroke_inset * 2.0 {
            return root;
        }

        let dash_length = px(DASH_LENGTH);
        let dash_gap = px(DASH_GAP);
        let roi_outline = div()
            .id(("video-roi-rect", cx.entity_id()))
            .absolute()
            .left(left_px)
            .top(top_px)
            .w(width_px)
            .h(height_px)
            .child(
                canvas(
                    |_bounds, _window, _cx| (),
                    move |bounds, _, window, _cx| {
                        let left = bounds.origin.x + stroke_inset;
                        let top = bounds.origin.y + stroke_inset;
                        let right = bounds.origin.x + bounds.size.width - stroke_inset;
                        let bottom = bounds.origin.y + bounds.size.height - stroke_inset;
                        let x1 = left + handle_gap;
                        let x2 = right - handle_gap;
                        let y1 = top + handle_gap;
                        let y2 = bottom - handle_gap;
                        if x2 <= x1 || y2 <= y1 {
                            return;
                        }

                        let mut builder =
                            PathBuilder::stroke(stroke_width).dash_array(&[dash_length, dash_gap]);
                        builder.move_to(point(x1, top));
                        builder.line_to(point(x2, top));
                        builder.move_to(point(right, y1));
                        builder.line_to(point(right, y2));
                        builder.move_to(point(x2, bottom));
                        builder.line_to(point(x1, bottom));
                        builder.move_to(point(left, y2));
                        builder.line_to(point(left, y1));

                        if let Ok(path) = builder.build() {
                            window.paint_path(path, border_color);
                        }
                    },
                )
                .size_full(),
            );

        root = root.child(roi_outline);

        let handle_size = px(HANDLE_SIZE);
        let handle_positions = [
            (
                DragCorner::TopLeft,
                left_px + stroke_inset,
                top_px + stroke_inset,
            ),
            (
                DragCorner::TopRight,
                left_px + width_px - stroke_inset,
                top_px + stroke_inset,
            ),
            (
                DragCorner::BottomLeft,
                left_px + stroke_inset,
                top_px + height_px - stroke_inset,
            ),
            (
                DragCorner::BottomRight,
                left_px + width_px - stroke_inset,
                top_px + height_px - stroke_inset,
            ),
        ];

        for (corner, x, y) in handle_positions {
            let cursor = cursor_for_corner(corner);
            let id = match corner {
                DragCorner::TopLeft => "video-roi-handle-tl",
                DragCorner::TopRight => "video-roi-handle-tr",
                DragCorner::BottomLeft => "video-roi-handle-bl",
                DragCorner::BottomRight => "video-roi-handle-br",
            };

            let handle_view = div()
                .id((id, cx.entity_id()))
                .absolute()
                .left(x - handle_radius)
                .top(y - handle_radius)
                .w(handle_size)
                .h(handle_size)
                .rounded(handle_radius)
                .border_2()
                .border_color(border_color)
                .bg(hsla(0.0, 0.0, 0.0, 0.0))
                .map(|mut view| {
                    view.style().mouse_cursor = Some(cursor);
                    view
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                        this.begin_drag(corner, event.position, cx);
                    }),
                );

            root = root.child(handle_view);
        }

        root
    }
}

impl VideoRoiOverlay {
    fn has_frame(&self) -> bool {
        let Some(info) = self.info.as_ref() else {
            return false;
        };
        let snapshot = info.snapshot();
        snapshot.has_frame
    }
}

fn default_roi() -> RoiConfig {
    let width = 1.0 - DEFAULT_LEFT_GAP - DEFAULT_RIGHT_GAP;
    let height = DEFAULT_HEIGHT;
    let x = DEFAULT_LEFT_GAP;
    let y = (1.0 - DEFAULT_BOTTOM_GAP - height).max(0.0);
    RoiConfig {
        x,
        y,
        width: width.max(0.0),
        height: height.max(0.0),
    }
}

fn roi_edges(roi: RoiConfig) -> (f32, f32, f32, f32) {
    let left = roi.x.clamp(0.0, 1.0);
    let top = roi.y.clamp(0.0, 1.0);
    let right = (roi.x + roi.width).clamp(left, 1.0);
    let bottom = (roi.y + roi.height).clamp(top, 1.0);
    (left, top, right, bottom)
}

fn min_roi_height(picture: Bounds<Pixels>) -> f32 {
    let height_px: f32 = picture.size.height.into();
    if height_px <= 0.0 {
        return MIN_ROI_HEIGHT_FRACTION;
    }
    let min_from_handle = (HANDLE_SIZE / height_px).min(1.0);
    MIN_ROI_HEIGHT_FRACTION.max(min_from_handle).min(1.0)
}

fn min_roi_width(picture: Bounds<Pixels>) -> f32 {
    let width_px: f32 = picture.size.width.into();
    if width_px <= 0.0 {
        return MIN_ROI_WIDTH_FRACTION;
    }
    let min_from_handle = (HANDLE_SIZE / width_px).min(1.0);
    MIN_ROI_WIDTH_FRACTION.max(min_from_handle).min(1.0)
}

fn cursor_for_corner(corner: DragCorner) -> CursorStyle {
    #[cfg(target_os = "windows")]
    {
        let _ = corner;
        CursorStyle::default()
    }
    #[cfg(not(target_os = "windows"))]
    match corner {
        DragCorner::TopLeft | DragCorner::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        DragCorner::TopRight | DragCorner::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
    }
}

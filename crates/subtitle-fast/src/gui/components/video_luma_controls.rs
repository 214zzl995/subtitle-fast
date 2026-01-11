use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, BorderStyle, Bounds, Context, Corners, DispatchPhase, FontWeight,
    Half, Hsla, IsZero, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    Render, Window, canvas, div, ease_out_quint, hsla, point, px, quad, rgb, size,
    transparent_black,
};
use subtitle_fast_validator::subtitle_detection::{DEFAULT_DELTA, DEFAULT_TARGET};
use tokio::sync::watch;

use crate::gui::icons::{Icon, icon_sm};

#[derive(Clone, Copy, Debug, Default)]
pub struct VideoLumaValues {
    pub target: u8,
    pub delta: u8,
}

#[derive(Clone)]
pub struct VideoLumaHandle {
    receiver: watch::Receiver<VideoLumaValues>,
}

impl VideoLumaHandle {
    pub fn subscribe(&self) -> watch::Receiver<VideoLumaValues> {
        self.receiver.clone()
    }

    pub fn latest(&self) -> VideoLumaValues {
        *self.receiver.borrow()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LumaField {
    Target,
    Delta,
}

impl LumaField {
    fn id(self) -> &'static str {
        match self {
            Self::Target => "target",
            Self::Delta => "delta",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct SliderState {
    hovered: bool,
    hover_from: bool,
    hover_token: u64,
    bounds: Option<Bounds<Pixels>>,
}

impl SliderState {
    fn new() -> Self {
        Self {
            hovered: false,
            hover_from: false,
            hover_token: 0,
            bounds: None,
        }
    }
}

pub struct VideoLumaControls {
    target: u8,
    delta: u8,
    dragging: Option<LumaField>,
    enabled: bool,
    target_state: SliderState,
    delta_state: SliderState,
    sender: watch::Sender<VideoLumaValues>,
}

impl VideoLumaControls {
    pub fn new() -> (Self, VideoLumaHandle) {
        let values = VideoLumaValues {
            target: DEFAULT_TARGET,
            delta: DEFAULT_DELTA,
        };
        let (sender, receiver) = watch::channel(values);
        (
            Self {
                target: values.target,
                delta: values.delta,
                dragging: None,
                enabled: false,
                target_state: SliderState::new(),
                delta_state: SliderState::new(),
                sender,
            },
            VideoLumaHandle { receiver },
        )
    }

    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        self.dragging = None;
        self.target_state.hovered = false;
        self.target_state.hover_from = false;
        self.delta_state.hovered = false;
        self.delta_state.hover_from = false;
        cx.notify();
    }

    pub fn set_values(&mut self, target: u8, delta: u8, cx: &mut Context<Self>) {
        if self.target == target && self.delta == delta {
            return;
        }
        self.target = target;
        self.delta = delta;
        let _ = self.sender.send(VideoLumaValues {
            target: self.target,
            delta: self.delta,
        });
        cx.notify();
    }

    fn state(&self, field: LumaField) -> &SliderState {
        match field {
            LumaField::Target => &self.target_state,
            LumaField::Delta => &self.delta_state,
        }
    }

    fn state_mut(&mut self, field: LumaField) -> &mut SliderState {
        match field {
            LumaField::Target => &mut self.target_state,
            LumaField::Delta => &mut self.delta_state,
        }
    }

    fn value(&self, field: LumaField) -> u8 {
        match field {
            LumaField::Target => self.target,
            LumaField::Delta => self.delta,
        }
    }

    fn set_value(&mut self, field: LumaField, value: u8, cx: &mut Context<Self>) {
        let changed = match field {
            LumaField::Target => {
                if self.target == value {
                    false
                } else {
                    self.target = value;
                    true
                }
            }
            LumaField::Delta => {
                if self.delta == value {
                    false
                } else {
                    self.delta = value;
                    true
                }
            }
        };

        if changed {
            let _ = self.sender.send(VideoLumaValues {
                target: self.target,
                delta: self.delta,
            });
            cx.notify();
        }
    }

    fn update_bounds(&mut self, field: LumaField, bounds: Option<Bounds<Pixels>>) {
        self.state_mut(field).bounds = bounds;
    }

    fn bounds_contains(&self, field: LumaField, position: Point<Pixels>) -> bool {
        self.state(field)
            .bounds
            .map(|bounds| bounds.contains(&position))
            .unwrap_or(false)
    }

    fn value_from_position(&self, field: LumaField, position: Point<Pixels>) -> Option<u8> {
        let bounds = self.state(field).bounds?;
        if bounds.size.width.is_zero() {
            return None;
        }
        let mut ratio = (position.x - bounds.origin.x) / bounds.size.width;
        if !ratio.is_finite() {
            return None;
        }
        ratio = ratio.clamp(0.0, 1.0);
        let value = (ratio * 255.0).round().clamp(0.0, 255.0) as u8;
        Some(value)
    }

    fn set_hovered(&mut self, field: LumaField, hovered: bool, cx: &mut Context<Self>) {
        let state = self.state_mut(field);
        if state.hovered == hovered {
            return;
        }
        state.hover_from = state.hovered;
        state.hovered = hovered;
        state.hover_token = state.hover_token.wrapping_add(1);
        cx.notify();
    }

    fn begin_drag(&mut self, field: LumaField, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.dragging = Some(field);
        self.set_hovered(field, true, cx);
        if let Some(value) = self.value_from_position(field, position) {
            self.set_value(field, value, cx);
        }
    }

    fn update_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(field) = self.dragging else {
            return;
        };
        if let Some(value) = self.value_from_position(field, position) {
            self.set_value(field, value, cx);
        }
    }

    fn end_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(field) = self.dragging else {
            return;
        };
        if let Some(value) = self.value_from_position(field, position) {
            self.set_value(field, value, cx);
        }
        self.dragging = None;
        let hovered = self.bounds_contains(field, position);
        self.set_hovered(field, hovered, cx);
    }

    fn slider_row(
        &self,
        field: LumaField,
        label: &'static str,
        icon: Icon,
        accent: Hsla,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let value = self.value(field);
        let state = self.state(field);
        let enabled = self.enabled;
        let hover_from = if self.enabled && state.hover_from {
            1.0_f32
        } else {
            0.0_f32
        };
        let hover_to = if self.enabled && (state.hovered || self.dragging == Some(field)) {
            1.0_f32
        } else {
            0.0_f32
        };
        let slider_canvas = if (hover_from - hover_to).abs() < f32::EPSILON {
            build_luma_slider_canvas(value, hover_to, enabled).into_any_element()
        } else {
            let animation =
                Animation::new(Duration::from_millis(180)).with_easing(ease_out_quint());
            let token = state.hover_token;
            let base_id = gpui::ElementId::from(("luma-hover", cx.entity_id()));
            let animation_id = (
                gpui::ElementId::from((base_id, field.id())),
                token.to_string(),
            );
            build_luma_slider_canvas(value, hover_from, enabled)
                .with_animation(animation_id, animation, move |_track, delta| {
                    let mix = hover_from + (hover_to - hover_from) * delta;
                    build_luma_slider_canvas(value, mix, enabled)
                })
                .into_any_element()
        };

        let slider_handle = cx.entity();
        let slider_id = gpui::ElementId::from((
            gpui::ElementId::from(("video-luma-slider", cx.entity_id())),
            field.id(),
        ));
        let mut slider_track = div()
            .flex()
            .flex_1()
            .h(px(22.0))
            .items_center()
            .on_children_prepainted(move |bounds, _window, cx| {
                let bounds = bounds.first().copied();
                let _ = slider_handle.update(cx, |this, _| {
                    this.update_bounds(field, bounds);
                });
            })
            .child(slider_canvas)
            .id(slider_id);

        if self.enabled {
            slider_track = slider_track
                .cursor_pointer()
                .on_hover(cx.listener(move |this, hovered, _window, cx| {
                    if this.dragging.is_some() {
                        return;
                    }
                    this.set_hovered(field, *hovered, cx);
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                        this.begin_drag(field, event.position, cx);
                    }),
                );
        }

        let label_color = if self.enabled {
            hsla(0.0, 0.0, 1.0, 0.7)
        } else {
            hsla(0.0, 0.0, 1.0, 0.35)
        };

        let label_group = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(icon_sm(
                icon,
                if self.enabled {
                    accent
                } else {
                    hsla(0.0, 0.0, 1.0, 0.35)
                },
            ))
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(label_color)
                    .child(label),
            );

        let value_chip = div()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(10.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(label_color)
            .child(value.to_string());

        let head = div()
            .flex()
            .items_center()
            .justify_between()
            .w(px(140.0))
            .child(label_group)
            .child(value_chip);

        let row_id = gpui::ElementId::from((
            gpui::ElementId::from(("video-luma-row", cx.entity_id())),
            field.id(),
        ));

        div()
            .id(row_id)
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(head)
            .child(slider_track)
            .into_any_element()
    }
}

impl Render for VideoLumaControls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.dragging.is_some() && self.enabled {
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
                        this.end_drag(event.position, cx);
                    });
                    window.refresh();
                }
            });
        }

        let target_row = self.slider_row(
            LumaField::Target,
            "Y Brightness",
            Icon::Sun,
            hsla(0.0, 0.0, 1.0, 0.85),
            cx,
        );
        let delta_row = self.slider_row(
            LumaField::Delta,
            "Tolerance",
            Icon::Gauge,
            hsla(0.0, 0.0, 1.0, 0.65),
            cx,
        );

        div()
            .id(("video-luma-controls", cx.entity_id()))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(8.0))
            .rounded(px(10.0))
            .bg(rgb(0x151515))
            .border_1()
            .border_color(rgb(0x262626))
            .child(target_row)
            .child(delta_row)
    }
}

fn build_luma_slider_canvas(value: u8, mix: f32, enabled: bool) -> gpui::Canvas<()> {
    let progress = (value as f32 / 255.0).clamp(0.0, 1.0);
    let mix = mix.clamp(0.0, 1.0);
    let track_base = 4.0;
    let thumb_base = 10.0;
    let thumb_hover = 14.0;

    canvas(
        move |_, _, _| {},
        move |bounds, _, window, _| {
            let track_height = track_base;
            let track_height_px = px(track_height);
            let track_radius = track_height_px.half();
            let center_y = bounds.center().y;
            let track_top = center_y - track_height_px.half();
            let track_bounds = Bounds {
                origin: point(bounds.origin.x, track_top),
                size: size(bounds.size.width, track_height_px),
            };
            let track_corners =
                Corners::from(track_radius).clamp_radii_for_quad_size(track_bounds.size);

            let fill_width = (bounds.size.width * progress).clamp(px(0.0), bounds.size.width);
            let fill_bounds = Bounds {
                origin: track_bounds.origin,
                size: size(fill_width, track_bounds.size.height),
            };
            let fill_radius = fill_width.min(track_height_px).half();
            let fill_corners =
                Corners::from(fill_radius).clamp_radii_for_quad_size(fill_bounds.size);

            let track_bg = if enabled {
                rgb(0x2f2f2f)
            } else {
                rgb(0x232323)
            };
            let fill_bg = if enabled {
                rgb(0xd6d6d6)
            } else {
                rgb(0x5a5a5a)
            };

            window.paint_quad(quad(
                track_bounds,
                track_corners,
                track_bg,
                px(0.0),
                transparent_black(),
                BorderStyle::default(),
            ));

            if fill_width > px(0.0) {
                window.paint_quad(quad(
                    fill_bounds,
                    fill_corners,
                    fill_bg,
                    px(0.0),
                    transparent_black(),
                    BorderStyle::default(),
                ));
            }

            let thumb_size = thumb_base + (thumb_hover - thumb_base) * mix;
            let thumb_size_px = px(thumb_size);
            let thumb_radius = thumb_size_px.half();
            let thumb_center_x = bounds.origin.x + fill_width;
            let thumb_center = point(thumb_center_x, center_y);
            let thumb_bounds = Bounds {
                origin: point(thumb_center.x - thumb_radius, thumb_center.y - thumb_radius),
                size: size(thumb_size_px, thumb_size_px),
            };

            let thumb_bg = if enabled {
                rgb(0xf2f2f2)
            } else {
                rgb(0x6a6a6a)
            };
            window.paint_quad(quad(
                thumb_bounds,
                thumb_radius,
                thumb_bg,
                px(0.0),
                transparent_black(),
                BorderStyle::default(),
            ));
        },
    )
    .size_full()
}

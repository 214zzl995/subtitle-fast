use gpui::prelude::*;
use gpui::{
    Bounds, BoxShadow, Context, DispatchPhase, InteractiveElement, MouseButton, MouseDownEvent,
    PathBuilder, Pixels, Render, Rgba, SharedString, StatefulInteractiveElement, Window, canvas,
    deferred, div, hsla, point, px, rgb,
};
use tokio::sync::watch;

#[derive(Clone, Copy)]
struct ColorOption {
    name: &'static str,
    color: Rgba,
}

pub struct ColorPicker {
    open: bool,
    selected: usize,
    enabled: bool,
    button_bounds: Option<Bounds<Pixels>>,
    popup_bounds: Option<Bounds<Pixels>>,
    sender: watch::Sender<Rgba>,
}

impl ColorPicker {
    pub fn new() -> (Self, ColorPickerHandle) {
        let options = color_options();
        let selected = options[0].color;
        let (sender, receiver) = watch::channel(selected);
        (
            Self {
                open: false,
                selected: 0,
                enabled: false,
                button_bounds: None,
                popup_bounds: None,
                sender,
            },
            ColorPickerHandle { receiver },
        )
    }

    fn selected_color(&self) -> Rgba {
        let options = color_options();
        options
            .get(self.selected)
            .copied()
            .unwrap_or(options[0])
            .color
    }

    fn set_selected(&mut self, index: usize) -> bool {
        if self.selected == index {
            return false;
        }
        self.selected = index;
        let _ = self.sender.send(self.selected_color());
        true
    }

    fn toggle_open(&mut self, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        self.open = !self.open;
        if !self.open {
            self.popup_bounds = None;
        }
        cx.notify();
    }

    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        if !self.enabled {
            self.open = false;
            self.popup_bounds = None;
        }
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            self.popup_bounds = None;
            cx.notify();
        }
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        let _ = self.set_selected(index);
        self.close(cx);
    }
}

#[derive(Clone)]
pub struct ColorPickerHandle {
    receiver: watch::Receiver<Rgba>,
}

impl ColorPickerHandle {
    pub fn subscribe(&self) -> watch::Receiver<Rgba> {
        self.receiver.clone()
    }

    pub fn latest(&self) -> Rgba {
        *self.receiver.borrow()
    }
}

fn snap_to_device(value: Pixels, scale: f32) -> Pixels {
    let snapped = value.scale(scale).round();
    px((f64::from(snapped) as f32) / scale)
}

impl Render for ColorPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let container_bg = rgb(0x2b2b2b);
        let container_border = rgb(0x3a3a3a);
        let hover_bg = rgb(0x3f3f3f);
        let popup_bg = rgb(0x2f2f2f);
        let text_color = hsla(0.0, 0.0, 1.0, 0.85);
        let selected_bg = rgb(0x353535);

        let swatch_size = px(14.0);
        let swatch_radius = px(4.0);
        let button_size = px(26.0);
        let popup_width = px(140.0);
        let popup_radius = px(10.0);
        let tail_width = px(16.0);
        let tail_height = px(8.0);
        let tail_top_radius = px(1.0);
        let popup_offset = tail_height + px(3.0);
        let popup_left = (button_size - popup_width) * 0.5;
        let tail_left = (popup_width - tail_width) * 0.5;

        let options = color_options();
        let selected = options.get(self.selected).copied().unwrap_or(options[0]);

        let swatch_color = if self.enabled {
            selected.color
        } else {
            rgb(0x666666)
        };

        let button_bg = if self.enabled {
            container_bg
        } else {
            rgb(0x242424)
        };
        let button_border = if self.enabled {
            container_border
        } else {
            rgb(0x303030)
        };

        let swatch = div()
            .id(("color-picker-swatch", cx.entity_id()))
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .child(
                canvas(
                    |_bounds, _window, _cx| (),
                    move |bounds, _, window, _cx| {
                        let scale = window.scale_factor();
                        let center = bounds.center();
                        let center_x = snap_to_device(center.x, scale);
                        let center_y = snap_to_device(center.y, scale);
                        let swatch_bounds = Bounds::centered_at(
                            point(center_x, center_y),
                            gpui::size(swatch_size, swatch_size),
                        );
                        window.paint_quad(
                            gpui::fill(swatch_bounds, swatch_color).corner_radii(swatch_radius),
                        );
                    },
                )
                .size_full(),
            );

        let mut button = div()
            .id(("color-picker-button", cx.entity_id()))
            .relative()
            .w(button_size)
            .h(button_size)
            .rounded(px(6.0))
            .bg(button_bg)
            .border_1()
            .border_color(button_border)
            .child(swatch);

        if self.enabled {
            button = button
                .cursor_pointer()
                .hover(|style| style.bg(hover_bg))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.toggle_open(cx);
                }));
        }

        let handle = cx.entity();
        let button_wrapper = div()
            .on_children_prepainted(move |bounds, _window, cx| {
                let bounds = bounds.first().copied();
                let _ = handle.update(cx, |this, _cx| {
                    this.button_bounds = bounds;
                });
            })
            .child(button);

        let mut root = div()
            .id(("color-picker", cx.entity_id()))
            .relative()
            .child(button_wrapper);

        if self.open {
            let handle = cx.entity();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, _window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                if event.button != MouseButton::Left {
                    return;
                }
                let position = event.position;
                let _ = handle.update(cx, |this, cx| {
                    if !this.open {
                        return;
                    }
                    if let Some(bounds) = this.popup_bounds {
                        if !bounds.contains(&position) {
                            if let Some(button_bounds) = this.button_bounds {
                                if button_bounds.contains(&position) {
                                    return;
                                }
                            }
                            this.close(cx);
                        }
                    } else if let Some(button_bounds) = this.button_bounds {
                        if button_bounds.contains(&position) {
                            return;
                        }
                        this.close(cx);
                    }
                });
            });

            let mut popup = div()
                .id(("color-picker-popup", cx.entity_id()))
                .absolute()
                .top(popup_offset)
                .left(popup_left)
                .w(popup_width)
                .bg(popup_bg)
                .border_1()
                .border_color(container_border)
                .rounded(popup_radius)
                .shadow(vec![BoxShadow {
                    color: hsla(0.0, 0.0, 0.0, 0.35),
                    offset: gpui::point(px(0.0), px(4.0)),
                    blur_radius: px(8.0),
                    spread_radius: px(0.0),
                }])
                .occlude();

            let tail = div()
                .id(("color-picker-tail", cx.entity_id()))
                .absolute()
                .top(px(-7.0))
                .left(tail_left)
                .w(tail_width)
                .h(tail_height)
                .child(
                    canvas(
                        |_bounds, _window, _cx| (),
                        move |bounds, _, window, _cx| {
                            let left_f: f32 = bounds.origin.x.into();
                            let top_f: f32 = bounds.origin.y.into();
                            let width_f: f32 = bounds.size.width.into();
                            let height_f: f32 = bounds.size.height.into();
                            let mut radius_f: f32 = tail_top_radius.into();
                            radius_f = radius_f.min(height_f);

                            let apex_x = left_f + width_f * 0.5;
                            let apex_y = top_f;
                            let bottom_left_f = left_f;
                            let bottom_right_f = left_f + width_f;
                            let bottom_y_f = top_f + height_f;

                            let left_dx = bottom_left_f - apex_x;
                            let left_dy = bottom_y_f - apex_y;
                            let left_len = (left_dx * left_dx + left_dy * left_dy).sqrt();
                            let left_t = if left_len > 0.0 {
                                (radius_f / left_len).min(1.0)
                            } else {
                                0.0
                            };
                            let left_join_x = apex_x + left_dx * left_t;
                            let left_join_y = apex_y + left_dy * left_t;

                            let right_dx = bottom_right_f - apex_x;
                            let right_dy = bottom_y_f - apex_y;
                            let right_len = (right_dx * right_dx + right_dy * right_dy).sqrt();
                            let right_t = if right_len > 0.0 {
                                (radius_f / right_len).min(1.0)
                            } else {
                                0.0
                            };
                            let right_join_x = apex_x + right_dx * right_t;
                            let right_join_y = apex_y + right_dy * right_t;

                            let mut builder = PathBuilder::fill();
                            builder.move_to(point(px(bottom_left_f), px(bottom_y_f)));
                            builder.line_to(point(px(bottom_right_f), px(bottom_y_f)));
                            builder.line_to(point(px(right_join_x), px(right_join_y)));
                            builder.curve_to(
                                point(px(left_join_x), px(left_join_y)),
                                point(px(apex_x), px(apex_y)),
                            );
                            builder.line_to(point(px(bottom_left_f), px(bottom_y_f)));
                            if let Ok(path) = builder.build() {
                                window.paint_path(path, popup_bg);
                            }
                        },
                    )
                    .size_full(),
                );

            popup = popup.child(tail);

            let entity_id = cx.entity_id().as_u64();
            let option_base_id = SharedString::from(format!("color-picker-option-{entity_id}"));
            let swatch_base_id =
                SharedString::from(format!("color-picker-option-swatch-{entity_id}"));
            let divider_base_id = SharedString::from(format!("color-picker-divider-{entity_id}"));

            for (index, option) in options.iter().enumerate() {
                let mut row = div()
                    .id((option_base_id.clone(), index))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .py(px(6.0))
                    .text_size(px(11.0))
                    .text_color(text_color)
                    .cursor_pointer()
                    .hover(|style| style.bg(hover_bg))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.select(index, cx);
                    }))
                    .child(
                        div()
                            .id((swatch_base_id.clone(), index))
                            .w(swatch_size)
                            .h(swatch_size)
                            .rounded(swatch_radius)
                            .bg(option.color),
                    )
                    .child(option.name);

                if index == 0 {
                    row = row.rounded_tl(popup_radius).rounded_tr(popup_radius);
                }

                if index + 1 == options.len() {
                    row = row.rounded_bl(popup_radius).rounded_br(popup_radius);
                }

                if index == self.selected {
                    row = row.bg(selected_bg);
                }

                popup = popup.child(row);

                if index + 1 < options.len() {
                    popup = popup.child(
                        div()
                            .id((divider_base_id.clone(), index))
                            .w_full()
                            .h(px(1.0))
                            .bg(container_border),
                    );
                }
            }

            let handle = cx.entity();
            let popup_host = div()
                .on_children_prepainted(move |bounds, _window, cx| {
                    let bounds = bounds.first().copied();
                    let _ = handle.update(cx, |this, _cx| {
                        this.popup_bounds = bounds;
                    });
                })
                .id(("color-picker-popup-host", cx.entity_id()))
                .relative()
                .child(popup);

            root = root.child(deferred(popup_host).with_priority(10));
        }

        root
    }
}

fn color_options() -> [ColorOption; 9] {
    [
        ColorOption {
            name: "Crimson",
            color: rgb(0xE53935),
        },
        ColorOption {
            name: "Orange",
            color: rgb(0xFB8C00),
        },
        ColorOption {
            name: "Amber",
            color: rgb(0xFDD835),
        },
        ColorOption {
            name: "Lime",
            color: rgb(0xC0CA33),
        },
        ColorOption {
            name: "Emerald",
            color: rgb(0x43A047),
        },
        ColorOption {
            name: "Cyan",
            color: rgb(0x00ACC1),
        },
        ColorOption {
            name: "Azure",
            color: rgb(0x1E88E5),
        },
        ColorOption {
            name: "Violet",
            color: rgb(0x8E24AA),
        },
        ColorOption {
            name: "Magenta",
            color: rgb(0xD81B60),
        },
    ]
}

use gpui::prelude::*;
use gpui::{
    AnchoredPositionMode, BoxShadow, Context, Corner, InteractiveElement, Render, Rgba,
    SharedString, StatefulInteractiveElement, Window, anchored, deferred, div, hsla, point, px,
    rgb,
};

#[derive(Clone, Copy)]
struct ColorOption {
    name: &'static str,
    color: Rgba,
}

pub struct ColorPicker {
    open: bool,
    selected: usize,
}

impl ColorPicker {
    pub fn new() -> Self {
        Self {
            open: false,
            selected: 0,
        }
    }

    fn toggle_open(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            cx.notify();
        }
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.selected != index {
            self.selected = index;
        }
        self.close(cx);
    }
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
        let popup_width = px(180.0);
        let popup_offset = px(30.0);
        let window_bounds = window.bounds();

        let options = color_options();
        let selected = options.get(self.selected).copied().unwrap_or(options[0]);

        let swatch = div()
            .id(("color-picker-swatch", cx.entity_id()))
            .w(swatch_size)
            .h(swatch_size)
            .rounded(swatch_radius)
            .bg(selected.color);

        let button = div()
            .id(("color-picker-button", cx.entity_id()))
            .flex()
            .items_center()
            .justify_center()
            .w(button_size)
            .h(button_size)
            .rounded(px(6.0))
            .bg(container_bg)
            .border_1()
            .border_color(container_border)
            .cursor_pointer()
            .hover(|style| style.bg(hover_bg))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.toggle_open(cx);
            }))
            .child(swatch);

        let mut root = div()
            .id(("color-picker", cx.entity_id()))
            .relative()
            .child(button);

        if self.open {
            let overlay = anchored()
                .anchor(Corner::TopLeft)
                .position_mode(AnchoredPositionMode::Window)
                .position(point(px(0.0), px(0.0)))
                .child(
                    div()
                        .id(("color-picker-backdrop", cx.entity_id()))
                        .w(window_bounds.size.width)
                        .h(window_bounds.size.height)
                        .bg(hsla(0.0, 0.0, 0.0, 0.0))
                        .occlude()
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.close(cx);
                        })),
                );

            let mut popup = div()
                .id(("color-picker-popup", cx.entity_id()))
                .absolute()
                .top(popup_offset)
                .left(px(0.0))
                .w(popup_width)
                .bg(popup_bg)
                .border_1()
                .border_color(container_border)
                .rounded(px(8.0))
                .shadow(vec![BoxShadow {
                    color: hsla(0.0, 0.0, 0.0, 0.35),
                    offset: gpui::point(px(0.0), px(4.0)),
                    blur_radius: px(8.0),
                    spread_radius: px(0.0),
                }])
                .occlude();

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

            root = root
                .child(deferred(overlay).with_priority(5))
                .child(deferred(popup).with_priority(10));
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

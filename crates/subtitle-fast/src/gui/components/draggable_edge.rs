use gpui::prelude::*;
use gpui::*;

const HANDLE_WIDTH: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraggableEdge {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug)]
pub struct DragRange {
    pub min: Pixels,
    pub max: Pixels,
}

impl DragRange {
    pub fn new(min: Pixels, max: Pixels) -> Self {
        if min <= max {
            Self { min, max }
        } else {
            Self { min: max, max: min }
        }
    }

    fn clamp(&self, value: Pixels) -> Pixels {
        value.clamp(self.min, self.max)
    }
}

#[derive(Clone, Copy)]
struct DragOrigin {
    position: Point<Pixels>,
    width: Pixels,
}

pub struct DraggableEdgePanel {
    pub edge: DraggableEdge,
    pub range: DragRange,
    width: Pixels,
    drag_origin: Option<DragOrigin>,
}

impl DraggableEdgePanel {
    pub fn new(edge: DraggableEdge, range: DragRange) -> Self {
        Self {
            edge,
            range,
            width: range.min,
            drag_origin: None,
        }
    }

    fn begin_drag(&mut self, event: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.drag_origin = Some(DragOrigin {
            position: event.position,
            width: self.width,
        });
        cx.notify();
    }

    fn end_drag(&mut self, _event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.drag_origin = None;
        cx.notify();
    }

    fn update_drag_from_position(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(origin) = self.drag_origin else {
            return;
        };

        let delta = position.x - origin.position.x;
        let next = match self.edge {
            DraggableEdge::Left => origin.width - delta,
            DraggableEdge::Right => origin.width + delta,
        };
        let next = self.range.clamp(next);
        if next != self.width {
            self.width = next;
            cx.notify();
        }
    }

    fn handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(HANDLE_WIDTH))
            .h_full()
            .bg(rgb(0x2b2b2b))
            .cursor_ew_resize()
            .id(("draggable-edge-handle", cx.entity_id()))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::begin_drag))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::end_drag))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::end_drag))
    }
}

impl Render for DraggableEdgePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.drag_origin.is_some() {
            window.set_window_cursor_style(CursorStyle::ResizeLeftRight);
            let handle = cx.entity();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                let _ = handle.update(cx, |this, cx| {
                    this.update_drag_from_position(event.position, cx);
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
                        this.drag_origin = None;
                        cx.notify();
                    });
                    window.refresh();
                }
            });
        }

        let content = div()
            .flex_grow()
            .h_full()
            .bg(rgb(0x1a1a1a))
            .items_center()
            .justify_center()
            .text_color(rgb(0xf0f0f0))
            .child("Resizable Panel");

        let panel = div()
            .flex()
            .flex_row()
            .h_full()
            .w(self.width)
            .min_w(self.width)
            .max_w(self.width)
            .flex_none()
            .id(("draggable-edge-panel", cx.entity_id()))
            .bg(rgb(0x1a1a1a));

        match self.edge {
            DraggableEdge::Left => panel.child(self.handle(cx)).child(content),
            DraggableEdge::Right => panel.child(content).child(self.handle(cx)),
        }
    }
}

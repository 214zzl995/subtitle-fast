use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::*;

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

struct DraggableEdgeState {
    width: Pixels,
    drag_origin: Option<DragOrigin>,
}

impl DraggableEdgeState {
    fn new(range: DragRange) -> Self {
        Self {
            width: range.min,
            drag_origin: None,
        }
    }

    fn width(&self) -> Pixels {
        self.width
    }

    fn set_width(&mut self, range: DragRange, width: Pixels) -> bool {
        let next = range.clamp(width);
        if next != self.width {
            self.width = next;
            return true;
        }
        false
    }

    fn is_dragging(&self) -> bool {
        self.drag_origin.is_some()
    }

    fn begin_drag(&mut self, position: Point<Pixels>) {
        self.drag_origin = Some(DragOrigin {
            position,
            width: self.width,
        });
    }

    fn end_drag(&mut self) {
        self.drag_origin = None;
    }

    fn update_drag_from_position(
        &mut self,
        edge: DraggableEdge,
        range: DragRange,
        position: Point<Pixels>,
    ) -> bool {
        let Some(origin) = self.drag_origin else {
            return false;
        };

        let delta = position.x - origin.position.x;
        let next = match edge {
            DraggableEdge::Left => origin.width - delta,
            DraggableEdge::Right => origin.width + delta,
        };
        self.set_width(range, next)
    }
}

fn draggable_edge_handle(
    id: impl Into<ElementId>,
    edge: DraggableEdge,
    thickness: Pixels,
) -> Stateful<Div> {
    let mut handle = div()
        .absolute()
        .top_0()
        .bottom_0()
        .w(thickness)
        .cursor_ew_resize()
        .id(id);

    handle = match edge {
        DraggableEdge::Left => handle.left_0(),
        DraggableEdge::Right => handle.right_0(),
    };

    handle
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollapseDirection {
    Left,
    Right,
}

#[derive(Clone)]
pub struct SidebarHandle {
    entity: WeakEntity<Sidebar>,
}

impl SidebarHandle {
    fn new(entity: WeakEntity<Sidebar>) -> Self {
        Self { entity }
    }

    pub fn close(&self, cx: &mut App) {
        let _ = self.entity.update(cx, |this, cx| {
            this.start_close(cx);
        });
    }

    pub fn open(&self, cx: &mut App) {
        let _ = self.entity.update(cx, |this, cx| {
            this.start_open(cx);
        });
    }

    pub fn toggle(&self, cx: &mut App) {
        let _ = self.entity.update(cx, |this, cx| {
            if this.collapsed {
                this.start_open(cx);
            } else {
                this.start_close(cx);
            }
        });
    }
}

pub struct Sidebar {
    edge: DraggableEdge,
    range: DragRange,
    collapse_direction: CollapseDirection,
    collapsed_width: Pixels,
    collapse_duration: Duration,
    drag_hit_thickness: Pixels,
    content: Box<dyn Fn() -> AnyElement>,
    drag_state: DraggableEdgeState,
    visible_width: Pixels,
    animation: Option<CollapseAnimation>,
    collapsed: bool,
}

impl Sidebar {
    pub fn create(
        edge: DraggableEdge,
        range: DragRange,
        collapse_direction: CollapseDirection,
        collapsed_width: Pixels,
        collapse_duration: Duration,
        drag_hit_thickness: Pixels,
        content: impl Fn() -> AnyElement + 'static,
        cx: &mut App,
    ) -> (Entity<Self>, SidebarHandle) {
        let entity = cx.new(|_| {
            Self::new(
                edge,
                range,
                collapse_direction,
                collapsed_width,
                collapse_duration,
                drag_hit_thickness,
                Box::new(content),
            )
        });
        let handle = SidebarHandle::new(entity.downgrade());
        (entity, handle)
    }

    fn new(
        edge: DraggableEdge,
        range: DragRange,
        collapse_direction: CollapseDirection,
        collapsed_width: Pixels,
        collapse_duration: Duration,
        drag_hit_thickness: Pixels,
        content: Box<dyn Fn() -> AnyElement>,
    ) -> Self {
        let drag_state = DraggableEdgeState::new(range);
        let visible_width = drag_state.width();
        let collapsed_width = collapsed_width.clamp(px(0.0), range.max);
        let drag_hit_thickness = drag_hit_thickness.max(px(0.0));
        Self {
            edge,
            range,
            collapse_direction,
            collapsed_width,
            collapse_duration,
            drag_hit_thickness,
            content,
            drag_state,
            visible_width,
            animation: None,
            collapsed: false,
        }
    }

    fn start_close(&mut self, cx: &mut Context<Self>) {
        if self.collapsed && self.animation.is_none() {
            return;
        }

        self.collapsed = true;
        self.drag_state.end_drag();
        let target = self.collapsed_width.min(self.drag_state.width());
        self.animation = Some(CollapseAnimation::new(self.visible_width, target));
        cx.notify();
    }

    fn start_open(&mut self, cx: &mut Context<Self>) {
        let target = self.drag_state.width();
        if !self.collapsed && self.animation.is_none() && target == self.visible_width {
            return;
        }

        self.collapsed = false;
        self.drag_state.end_drag();
        self.animation = Some(CollapseAnimation::new(self.visible_width, target));
        cx.notify();
    }

    fn begin_drag(&mut self, event: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.collapsed = false;
        self.animation = None;
        self.drag_state.begin_drag(event.position);
        self.visible_width = self.drag_state.width();
        cx.notify();
    }

    fn end_drag(&mut self, _event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.drag_state.end_drag();
        cx.notify();
    }

    fn update_drag_from_position(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self
            .drag_state
            .update_drag_from_position(self.edge, self.range, position)
        {
            self.collapsed = false;
            self.visible_width = self.drag_state.width();
            cx.notify();
        }
    }

    fn animate_width(&mut self, window: &mut Window) {
        let Some(animation) = self.animation.take() else {
            return;
        };

        let elapsed = Instant::now().saturating_duration_since(animation.started_at);
        let duration = self.collapse_duration.as_secs_f32();
        let mut progress = if duration <= f32::EPSILON {
            1.0
        } else {
            (elapsed.as_secs_f32() / duration).min(1.0)
        };
        progress = ease_out(progress);

        let delta = animation.target - animation.start;
        self.visible_width = animation.start + delta * progress;

        if progress < 1.0 {
            self.animation = Some(animation);
            window.request_animation_frame();
        }
    }

    fn handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        draggable_edge_handle(
            ("collapsible-sidebar-handle", cx.entity_id()),
            self.edge,
            self.drag_hit_thickness,
        )
        .on_mouse_down(MouseButton::Left, cx.listener(Self::begin_drag))
        .on_mouse_up(MouseButton::Left, cx.listener(Self::end_drag))
        .on_mouse_up_out(MouseButton::Left, cx.listener(Self::end_drag))
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.animate_width(window);

        if self.drag_state.is_dragging() {
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
                        this.drag_state.end_drag();
                        cx.notify();
                    });
                    window.refresh();
                }
            });
        }

        let current_width = self.visible_width;
        let anchor_width = self.drag_state.width();

        let content_body = (self.content)();
        let content = div()
            .relative()
            .size_full()
            .id(("collapsible-sidebar-content", cx.entity_id()))
            .child(content_body);

        let panel = div()
            .relative()
            .h_full()
            .w(anchor_width)
            .min_w(anchor_width)
            .max_w(anchor_width)
            .flex_none()
            .id(("collapsible-sidebar-panel", cx.entity_id()))
            .child(self.handle(cx))
            .child(content);

        let mut outer = div()
            .flex()
            .flex_row()
            .h_full()
            .w(current_width)
            .min_w(current_width)
            .max_w(current_width)
            .flex_none()
            .overflow_hidden()
            .id(("collapsible-sidebar", cx.entity_id()));

        outer = match self.collapse_direction {
            CollapseDirection::Left => outer.justify_start(),
            CollapseDirection::Right => outer.justify_end(),
        };

        outer.child(panel)
    }
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

struct CollapseAnimation {
    start: Pixels,
    target: Pixels,
    started_at: Instant,
}

impl CollapseAnimation {
    fn new(start: Pixels, target: Pixels) -> Self {
        Self {
            start,
            target,
            started_at: Instant::now(),
        }
    }
}

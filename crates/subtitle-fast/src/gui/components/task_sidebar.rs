use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{
    Bounds, Context, DispatchPhase, FontWeight, InteractiveElement, MouseButton, MouseDownEvent,
    Pixels, Point, Render, Task, Window, deferred, div, hsla, point, px, relative, rgb,
};
use tokio::time::MissedTickBehavior;

use crate::gui::icons::{Icon, icon_sm};
use crate::gui::session::{SessionHandle, SessionId, VideoSession};
use crate::stage::PipelineProgress;

use super::DetectionRunState;

const MENU_WIDTH: f32 = 160.0;
const MENU_ITEM_HEIGHT: f32 = 28.0;
const PROGRESS_STEP: f64 = 0.004;
const PROGRESS_THROTTLE: Duration = Duration::from_millis(350);

#[derive(Clone)]
pub struct TaskSidebarCallbacks {
    pub on_add: Arc<dyn Fn(&mut Window, &mut Context<TaskSidebar>) + Send + Sync>,
    pub on_select: Arc<dyn Fn(SessionId, &mut Window, &mut Context<TaskSidebar>) + Send + Sync>,
}

pub struct TaskSidebar {
    sessions: SessionHandle,
    callbacks: TaskSidebarCallbacks,
    container_bounds: Option<Bounds<Pixels>>,
    menu: Option<TaskMenuState>,
    menu_bounds: Option<Bounds<Pixels>>,
    progress_tasks: HashMap<SessionId, Task<()>>,
}

impl TaskSidebar {
    pub fn new(sessions: SessionHandle, callbacks: TaskSidebarCallbacks) -> Self {
        Self {
            sessions,
            callbacks,
            container_bounds: None,
            menu: None,
            menu_bounds: None,
            progress_tasks: HashMap::new(),
        }
    }

    pub fn set_callbacks(&mut self, callbacks: TaskSidebarCallbacks, cx: &mut Context<Self>) {
        self.callbacks = callbacks;
        cx.notify();
    }

    fn set_container_bounds(&mut self, bounds: Option<Bounds<Pixels>>) {
        self.container_bounds = bounds;
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            self.menu = None;
            self.menu_bounds = None;
            cx.notify();
        }
    }

    fn open_menu(
        &mut self,
        session_id: SessionId,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.menu = Some(TaskMenuState {
            session_id,
            position,
        });
        self.menu_bounds = None;
        cx.notify();
    }

    fn menu_position(&self, menu_width: Pixels, menu_height: Pixels) -> Option<Point<Pixels>> {
        let bounds = self.container_bounds?;
        let menu = self.menu.as_ref()?;
        let mut left = menu.position.x - bounds.origin.x;
        let mut top = menu.position.y - bounds.origin.y;

        let max_left = (bounds.size.width - menu_width - px(8.0)).max(px(4.0));
        let max_top = (bounds.size.height - menu_height - px(8.0)).max(px(4.0));
        left = left.clamp(px(4.0), max_left);
        top = top.clamp(px(4.0), max_top);
        Some(point(left, top))
    }

    fn ensure_progress_listener(
        &mut self,
        session: &VideoSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.progress_tasks.contains_key(&session.id) {
            return;
        }

        let handle = session.detection.clone();
        let entity_id = cx.entity_id();
        let task = window.spawn(cx, async move |cx| {
            let mut progress_rx = handle.subscribe_progress();
            let mut state_rx = handle.subscribe_state();
            let snapshot = progress_rx.borrow().clone();
            let mut last_progress = snapshot.progress;
            let mut last_seen_progress = snapshot.progress;
            let mut last_progress_change_at = Instant::now();
            let mut completed = snapshot.completed;
            let mut running = state_rx.borrow().is_running();

            let mut ticker = tokio::time::interval(PROGRESS_THROTTLE);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    changed = progress_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let snapshot = progress_rx.borrow().clone();
                        if snapshot.progress != last_seen_progress {
                            last_seen_progress = snapshot.progress;
                            last_progress_change_at = Instant::now();
                        }
                        let progress_delta = (snapshot.progress - last_progress).abs();
                        let completion_changed = snapshot.completed && !completed;
                        completed = snapshot.completed;

                        if completion_changed || progress_delta >= PROGRESS_STEP {
                            last_progress = snapshot.progress;
                            if cx.update(|_window, cx| cx.notify(entity_id)).is_err() {
                                break;
                            }
                        }
                    }
                    changed = state_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        running = state_rx.borrow().is_running();
                        let snapshot = progress_rx.borrow().clone();
                        last_progress = snapshot.progress;
                        last_seen_progress = snapshot.progress;
                        last_progress_change_at = Instant::now();
                        completed = snapshot.completed;
                        if cx.update(|_window, cx| cx.notify(entity_id)).is_err() {
                            break;
                        }
                    }
                    _ = ticker.tick() => {
                        if running
                            && !completed
                            && Instant::now().duration_since(last_progress_change_at) >= PROGRESS_THROTTLE
                        {
                            if cx.update(|_window, cx| cx.notify(entity_id)).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });

        self.progress_tasks.insert(session.id, task);
    }

    fn progress_snapshot(&self, session: &VideoSession) -> PipelineProgress {
        session.detection.progress_snapshot()
    }

    fn status_text(run_state: DetectionRunState, progress: &PipelineProgress) -> &'static str {
        if progress.completed {
            "Completed"
        } else {
            match run_state {
                DetectionRunState::Idle => "Idle",
                DetectionRunState::Running => "Running",
                DetectionRunState::Paused => "Paused",
            }
        }
    }

    fn progress_ratio(progress: &PipelineProgress) -> f32 {
        let mut ratio = progress.progress;
        if progress.completed && ratio <= 0.0 {
            ratio = 1.0;
        }
        ratio.clamp(0.0, 1.0) as f32
    }

    fn progress_label(progress: &PipelineProgress) -> String {
        match progress.total_frames {
            Some(total) if total > 0 => format!("{} / {}", progress.samples_seen, total),
            _ => progress.samples_seen.to_string(),
        }
    }

    fn menu_item(
        &self,
        label: &'static str,
        icon: Icon,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let text_color = if enabled {
            hsla(0.0, 0.0, 1.0, 0.92)
        } else {
            hsla(0.0, 0.0, 1.0, 0.35)
        };
        let hover_bg = hsla(0.0, 0.0, 1.0, 0.08);

        let mut row = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(px(MENU_ITEM_HEIGHT))
            .px(px(10.0))
            .text_size(px(12.0))
            .text_color(text_color)
            .child(icon_sm(icon, text_color).w(px(14.0)).h(px(14.0)))
            .child(label);

        if enabled {
            row = row
                .cursor_pointer()
                .hover(move |style| style.bg(hover_bg))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, _window, cx| {
                        on_click(this, cx);
                    }),
                );
        }

        row
    }

    fn apply_action(
        &mut self,
        session_id: SessionId,
        action: TaskAction,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.sessions.session(session_id) else {
            self.close_menu(cx);
            return;
        };
        match action {
            TaskAction::Start => {
                session.detection.start();
            }
            TaskAction::Pause => {
                session.detection.toggle_pause();
            }
            TaskAction::Cancel => {
                session.detection.cancel();
            }
        }
        self.close_menu(cx);
    }
}

#[derive(Clone, Copy)]
enum TaskAction {
    Start,
    Pause,
    Cancel,
}

#[derive(Clone)]
struct TaskMenuState {
    session_id: SessionId,
    position: Point<Pixels>,
}

impl Render for TaskSidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border_color = rgb(0x2b2b2b);
        let panel_bg = rgb(0x1a1a1a);
        let header_text = hsla(0.0, 0.0, 1.0, 0.8);
        let button_bg = rgb(0x232323);
        let button_hover = rgb(0x2c2c2c);
        let button_border = rgb(0x343434);
        let item_active_bg = rgb(0x242424);
        let item_hover_bg = rgb(0x202020);
        let item_text = hsla(0.0, 0.0, 1.0, 0.9);
        let item_subtle = hsla(0.0, 0.0, 1.0, 0.55);
        let progress_bg = rgb(0x2a2a2a);
        let progress_fill = rgb(0xd6d6d6);

        let sessions = self.sessions.sessions_snapshot();
        let active_id = self.sessions.active_id();
        for session in &sessions {
            self.ensure_progress_listener(session, window, cx);
        }

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(12.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(header_text)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(icon_sm(Icon::GalleryThumbnails, header_text))
                    .child("Task Queue"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .h(px(26.0))
                    .px(px(8.0))
                    .rounded(px(6.0))
                    .bg(button_bg)
                    .border_1()
                    .border_color(button_border)
                    .cursor_pointer()
                    .hover(move |style| style.bg(button_hover))
                    .child(icon_sm(Icon::Upload, header_text).w(px(14.0)).h(px(14.0)))
                    .child("Add File")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, window, cx| {
                            (this.callbacks.on_add)(window, cx);
                        }),
                    ),
            );

        let mut list = div().flex().flex_col().gap(px(6.0)).w_full();

        if sessions.is_empty() {
            list = list.child(
                div()
                    .text_size(px(12.0))
                    .text_color(item_subtle)
                    .py(px(8.0))
                    .child("No tasks yet"),
            );
        } else {
            for session in &sessions {
                let session_id = session.id;
                let session_label = session.label.clone();
                let progress = self.progress_snapshot(session);
                let run_state = session.detection.run_state();
                let status = Self::status_text(run_state, &progress);
                let ratio = Self::progress_ratio(&progress);
                let progress_label = Self::progress_label(&progress);

                let mut row = div()
                    .id(("task-sidebar-entry", session_id))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .px(px(8.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .bg(if Some(session.id) == active_id {
                        item_active_bg
                    } else {
                        panel_bg
                    })
                    .hover(move |style| style.bg(item_hover_bg))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, window, cx| {
                            (this.callbacks.on_select)(session_id, window, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            this.open_menu(session_id, event.position, cx);
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .min_w(px(0.0))
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(item_text)
                                            .child(session_label),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(item_subtle)
                                            .child(status),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.0))
                                    .text_color(item_subtle)
                                    .child(progress_label),
                            ),
                    )
                    .child(
                        div()
                            .relative()
                            .h(px(6.0))
                            .rounded(px(6.0))
                            .bg(progress_bg)
                            .child(
                                div()
                                    .absolute()
                                    .top(px(0.0))
                                    .bottom(px(0.0))
                                    .left(px(0.0))
                                    .rounded(px(6.0))
                                    .w(relative(ratio))
                                    .bg(progress_fill),
                            ),
                    );

                if Some(session.id) == active_id {
                    row = row.border_1().border_color(hsla(0.0, 0.0, 1.0, 0.08));
                }

                list = list.child(row);
            }
        }

        let handle = cx.entity();
        let body = div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .px(px(12.0))
            .py(px(16.0))
            .child(header)
            .child(list)
            .on_children_prepainted(move |bounds, _window, cx| {
                let bounds = bounds.first().copied();
                let _ = handle.update(cx, |this, _| {
                    this.set_container_bounds(bounds);
                });
            });

        let mut root = div()
            .id(("task-sidebar", cx.entity_id()))
            .flex()
            .flex_col()
            .size_full()
            .bg(panel_bg)
            .border_r(px(1.0))
            .border_color(border_color)
            .child(body);

        if self.menu.is_some() {
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
                    if let Some(bounds) = this.menu_bounds {
                        if !bounds.contains(&position) {
                            this.close_menu(cx);
                        }
                    } else {
                        this.close_menu(cx);
                    }
                });
            });
        }

        if let Some(menu) = self.menu.clone() {
            let menu_width = px(MENU_WIDTH);
            let menu_height = px(MENU_ITEM_HEIGHT * 3.0);
            let position = self.menu_position(menu_width, menu_height);
            let Some(position) = position else {
                return root;
            };

            let progress = self
                .sessions
                .session(menu.session_id)
                .map(|session| self.progress_snapshot(&session));
            let run_state = self
                .sessions
                .session(menu.session_id)
                .map(|session| session.detection.run_state())
                .unwrap_or(DetectionRunState::Idle);
            let is_idle = run_state == DetectionRunState::Idle;
            let is_running = run_state == DetectionRunState::Running;
            let is_paused = run_state == DetectionRunState::Paused;
            let completed = progress.map(|progress| progress.completed).unwrap_or(false);

            let start_enabled = is_idle && !completed;
            let pause_enabled = is_running || is_paused;
            let cancel_enabled = run_state.is_running();

            let menu_panel = div()
                .absolute()
                .left(position.x)
                .top(position.y)
                .w(menu_width)
                .bg(rgb(0x2f2f2f))
                .border_1()
                .border_color(rgb(0x3a3a3a))
                .rounded(px(8.0))
                .shadow(vec![gpui::BoxShadow {
                    color: hsla(0.0, 0.0, 0.0, 0.35),
                    offset: point(px(0.0), px(4.0)),
                    blur_radius: px(8.0),
                    spread_radius: px(0.0),
                }])
                .child(self.menu_item(
                    "Start",
                    Icon::Play,
                    start_enabled,
                    move |this, cx| {
                        this.apply_action(menu.session_id, TaskAction::Start, cx);
                    },
                    cx,
                ))
                .child(self.menu_item(
                    "Pause",
                    Icon::Pause,
                    pause_enabled,
                    move |this, cx| {
                        this.apply_action(menu.session_id, TaskAction::Pause, cx);
                    },
                    cx,
                ))
                .child(self.menu_item(
                    "Cancel",
                    Icon::Stop,
                    cancel_enabled,
                    move |this, cx| {
                        this.apply_action(menu.session_id, TaskAction::Cancel, cx);
                    },
                    cx,
                ))
                .occlude();

            let handle = cx.entity();
            let menu_host = div()
                .on_children_prepainted(move |bounds, _window, cx| {
                    let bounds = bounds.first().copied();
                    let _ = handle.update(cx, |this, _| {
                        this.menu_bounds = bounds;
                    });
                })
                .child(menu_panel);

            root = root.child(deferred(menu_host).with_priority(10));
        }

        root
    }
}
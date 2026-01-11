use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_channel::mpsc::unbounded;
use futures_util::StreamExt;
use gpui::prelude::*;
use gpui::{
    Bounds, Context, FontWeight, InteractiveElement, MouseButton, Pixels, Render, Task, Window,
    div, hsla, px, relative, rgb,
};
use tokio::time::MissedTickBehavior;

use crate::gui::icons::{Icon, icon_md, icon_sm};
use crate::gui::runtime;
use crate::gui::session::{SessionHandle, SessionId, VideoSession};
use crate::stage::PipelineProgress;

use super::DetectionRunState;

const PROGRESS_STEP: f64 = 0.001;
const PROGRESS_THROTTLE: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub struct TaskSidebarCallbacks {
    pub on_add: Arc<dyn Fn(&mut Window, &mut Context<TaskSidebar>) + Send + Sync>,
    pub on_select: Arc<dyn Fn(SessionId, &mut Window, &mut Context<TaskSidebar>) + Send + Sync>,
    pub on_remove: Arc<dyn Fn(SessionId, &mut Window, &mut Context<TaskSidebar>) + Send + Sync>,
}

pub struct TaskSidebar {
    sessions: SessionHandle,
    callbacks: TaskSidebarCallbacks,
    container_bounds: Option<Bounds<Pixels>>,
    progress_tasks: HashMap<SessionId, Task<()>>,
}

impl TaskSidebar {
    pub fn new(sessions: SessionHandle, callbacks: TaskSidebarCallbacks) -> Self {
        Self {
            sessions,
            callbacks,
            container_bounds: None,
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
        let (notify_tx, mut notify_rx) = unbounded::<()>();

        let task = window.spawn(cx, async move |cx| {
            while notify_rx.next().await.is_some() {
                if cx.update(|_window, cx| cx.notify(entity_id)).is_err() {
                    break;
                }
            }
        });

        let tokio_task = runtime::spawn(async move {
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
                            if notify_tx.unbounded_send(()).is_err() {
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
                        if notify_tx.unbounded_send(()).is_err() {
                            break;
                        }
                    }
                    _ = ticker.tick() => {

                        if running
                            && !completed
                            && Instant::now().duration_since(last_progress_change_at) >= PROGRESS_THROTTLE
                        {
                            if notify_tx.unbounded_send(()).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });

        if tokio_task.is_none() {
            eprintln!("task sidebar listener failed: tokio runtime not initialized");
        }
        self.progress_tasks.insert(session.id, task);
    }

    fn progress_snapshot(&self, session: &VideoSession) -> PipelineProgress {
        session.detection.progress_snapshot()
    }

    fn status_text(run_state: DetectionRunState, progress: &PipelineProgress) -> &'static str {
        if progress.completed {
            "Done"
        } else {
            match run_state {
                DetectionRunState::Idle => "Idle",
                DetectionRunState::Running => "Processing",
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

    fn apply_action(&mut self, session_id: SessionId, action: TaskAction, _cx: &mut Context<Self>) {
        let Some(session) = self.sessions.session(session_id) else {
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
    }
}

#[derive(Clone, Copy)]
enum TaskAction {
    Start,
    Pause,
    Cancel,
}

impl Render for TaskSidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border_color = rgb(0x2b2b2b);
        let panel_bg = rgb(0x1b1b1b);
        let header_text = hsla(0.0, 0.0, 1.0, 0.72);
        let item_bg = rgb(0x252525);
        let item_hover_bg = rgb(0x2f2f2f);
        let item_text = hsla(0.0, 0.0, 0.9, 1.0);
        let item_subtle = hsla(0.0, 0.0, 0.6, 1.0);
        let progress_fill = hsla(0.0, 0.0, 1.0, 0.1);
        let btn_icon_color = hsla(0.0, 0.0, 0.7, 1.0);
        let btn_hover_bg = hsla(0.0, 0.0, 1.0, 0.1);
        let btn_stop_hover_bg = hsla(0.0, 0.0, 1.0, 0.15);

        let sessions = self.sessions.sessions_snapshot();
        let active_id = self.sessions.active_id();
        for session in &sessions {
            self.ensure_progress_listener(session, window, cx);
        }

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .pl(px(12.0))
            .pr(px(4.0))
            .pb(px(12.0))
            .pt(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(header_text)
                    .child(
                        icon_sm(Icon::GalleryThumbnails, header_text)
                            .w(px(14.0))
                            .h(px(14.0)),
                    )
                    .child("Task"),
            )
            .child(
                div()
                    .id(("task-sidebar-add", cx.entity_id()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(28.0))
                    .w(px(28.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .child(
                        icon_sm(Icon::Upload, header_text)
                            .w(px(14.0))
                            .h(px(14.0)),
                    )
                    .hover(move |style| style.bg(hsla(0.0, 0.0, 1.0, 0.12)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, window, cx| {
                            (this.callbacks.on_add)(window, cx);
                        }),
                    ),
            );

        let mut list = div().flex().flex_col().size_full().gap(px(8.0)).px(px(8.0));

        if sessions.is_empty() {
            list = list
                .justify_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(8.0))
                        .text_size(px(13.0))
                        .text_color(item_subtle)
                        .child(icon_md(Icon::Inbox, item_subtle))
                        .child("No tasks"),
                );
        } else {
            for session in &sessions {
                let session_id = session.id;
                let is_active = Some(session_id) == active_id;
                let session_label = session.label.clone();
                let progress = self.progress_snapshot(session);
                let run_state = session.detection.run_state();
                let status_str = Self::status_text(run_state, &progress);
                let ratio = Self::progress_ratio(&progress);

                let is_idle = run_state == DetectionRunState::Idle;
                let is_running = run_state == DetectionRunState::Running;
                let is_paused = run_state == DetectionRunState::Paused;
                let completed = progress.completed;

                let start_enabled = is_idle && !completed;
                let pause_enabled = is_running || is_paused;
                let cancel_enabled =
                    run_state.is_running() || run_state == DetectionRunState::Paused;

                let icon_color = if is_active {
                    hsla(0.0, 0.0, 1.0, 1.0)
                } else if is_running || completed {
                    hsla(0.0, 0.0, 1.0, 1.0)
                } else {
                    item_subtle
                };

                let make_btn = |icon: Icon,
                                action: TaskAction,
                                is_stop: bool,
                                cx: &mut Context<Self>| {
                    let hover_bg = if is_stop {
                        btn_stop_hover_bg
                    } else {
                        btn_hover_bg
                    };
                    let hover_color = hsla(0.0, 0.0, 1.0, 1.0);
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.0))
                        .w(px(24.0))
                        .h(px(24.0))
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover_bg).text_color(hover_color))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.apply_action(session_id, action, cx);
                            }),
                        )
                        .child(icon_sm(icon, btn_icon_color).w(px(12.0)).h(px(12.0)))
                };

                let status_icon = if is_running {
                    icon_sm(Icon::Film, icon_color)
                } else if completed {
                    icon_sm(Icon::Check, icon_color)
                } else if is_paused {
                    icon_sm(Icon::Pause, icon_color)
                } else {
                    icon_sm(Icon::Film, icon_color)
                };

                let has_controls = start_enabled || pause_enabled || cancel_enabled;
                let mut controls_box = div().flex().items_center().gap(px(2.0));

                if has_controls {
                    controls_box = controls_box
                        .bg(hsla(0.0, 0.0, 0.0, 0.3))
                        .rounded(px(8.0))
                        .p(px(2.0));

                    if start_enabled {
                        controls_box =
                            controls_box.child(make_btn(Icon::Play, TaskAction::Start, false, cx));
                    }
                    if pause_enabled {
                        let icon = if is_paused { Icon::Play } else { Icon::Pause };
                        controls_box =
                            controls_box.child(make_btn(icon, TaskAction::Pause, false, cx));
                    }
                    if cancel_enabled {
                        controls_box =
                            controls_box.child(make_btn(Icon::Stop, TaskAction::Cancel, true, cx));
                    }
                }

                controls_box = controls_box.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.0))
                        .w(px(24.0))
                        .h(px(24.0))
                        .cursor_pointer()
                        .hover(move |s| {
                            s.bg(hsla(0.0, 0.0, 1.0, 0.15))
                                .text_color(hsla(0.0, 0.0, 1.0, 1.0))
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                (this.callbacks.on_remove)(session_id, window, cx);
                            }),
                        )
                        .child(icon_sm(Icon::Trash, btn_icon_color).w(px(12.0)).h(px(12.0))),
                );

                let progress_bg_layer = div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .w(relative(ratio))
                    .rounded(px(8.0))
                    .bg(if completed {
                        hsla(0.0, 0.0, 0.0, 0.0)
                    } else {
                        progress_fill
                    });

                let item_content = div()
                    .flex()
                    .items_center()
                    .w_full()
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(24.0))
                            .child(status_icon.w(px(14.0)).h(px(14.0))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if is_active {
                                        hsla(0.0, 0.0, 1.0, 1.0)
                                    } else {
                                        item_text
                                    })
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(session_label.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .h(px(28.0))
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(item_subtle)
                                            .child(status_str),
                                    )
                                    .child(controls_box),
                            ),
                    );

                let row = div()
                    .id(("task-sidebar-entry", session_id))
                    .relative()
                    .h(px(56.0))
                    .rounded(px(8.0))
                    .bg(if is_active {
                        rgb(0x323232)
                    } else {
                        item_bg
                    })
                    .pl(px(8.0))
                    .pr(px(4.0))
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, window, cx| {
                            (this.callbacks.on_select)(session_id, window, cx);
                        }),
                    )
                    .hover(move |s| {
                        if !is_active {
                            s.bg(item_hover_bg)
                        } else {
                            s
                        }
                    })
                    .child(progress_bg_layer)
                    .child(item_content.relative());

                list = list.child(row);
            }
        }

        let handle = cx.entity();
        let body = div()
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(list)
            .on_children_prepainted(move |bounds, _window, cx| {
                let bounds = bounds.first().copied();
                let _ = handle.update(cx, |this, _| {
                    this.set_container_bounds(bounds);
                });
            });

        div()
            .id(("task-sidebar", cx.entity_id()))
            .flex()
            .flex_col()
            .size_full()
            .bg(panel_bg)
            .border_r(px(1.1))
            .border_color(border_color)
            .child(body)
    }
}
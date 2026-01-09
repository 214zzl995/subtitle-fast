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

use crate::gui::icons::{Icon, icon_sm};
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
        let panel_bg = rgb(0x1a1a1a);
        let header_text = hsla(0.0, 0.0, 1.0, 0.8);
        let _hover_bg = hsla(0.0, 0.0, 1.0, 0.06);
        let item_active_bg = rgb(0x242424);
        let item_text = hsla(0.0, 0.0, 1.0, 0.9);
        let item_subtle = hsla(0.0, 0.0, 1.0, 0.5);
        let progress_bg = rgb(0x333333);
        let progress_fill = rgb(0xcccccc);
        let btn_icon_color = hsla(0.0, 0.0, 1.0, 0.6);
        let btn_hover_bg = hsla(0.0, 0.0, 1.0, 0.1);

        let sessions = self.sessions.sessions_snapshot();
        let active_id = self.sessions.active_id();
        for session in &sessions {
            self.ensure_progress_listener(session, window, cx);
        }

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(32.0))
            .px(px(8.0))
            .border_b_1()
            .border_color(border_color)
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(header_text)
                    .child("TASKS"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(20.0))
                    .h(px(20.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(btn_hover_bg))
                    .child(icon_sm(Icon::Upload, header_text).w(px(14.0)).h(px(14.0)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, window, cx| {
                            (this.callbacks.on_add)(window, cx);
                        }),
                    ),
            );

        let mut list = div().flex().flex_col().w_full().gap(px(1.0)).py(px(4.0));

        if sessions.is_empty() {
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(60.0))
                    .text_size(px(12.0))
                    .text_color(item_subtle)
                    .child("No tasks"),
            );
        } else {
            for session in &sessions {
                let session_id = session.id;
                let session_label = session.label.clone();
                let progress = self.progress_snapshot(session);
                let run_state = session.detection.run_state();
                let status_str = Self::status_text(run_state, &progress);
                let ratio = Self::progress_ratio(&progress);

                let is_active = Some(session.id) == active_id;
                let bg_color = if is_active { item_active_bg } else { panel_bg };

                let is_idle = run_state == DetectionRunState::Idle;
                let is_running = run_state == DetectionRunState::Running;
                let is_paused = run_state == DetectionRunState::Paused;
                let completed = progress.completed;

                let start_enabled = is_idle && !completed;
                let pause_enabled = is_running || is_paused;
                let cancel_enabled =
                    run_state.is_running() || run_state == DetectionRunState::Paused;

                let action_btn =
                    |icon: Icon, enabled: bool, action: TaskAction, cx: &mut Context<Self>| {
                        let mut btn = div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(20.0))
                            .h(px(20.0))
                            .rounded(px(3.0));

                        if enabled {
                            btn = btn
                                .cursor_pointer()
                                .hover(move |s| s.bg(btn_hover_bg))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.apply_action(session_id, action, cx);
                                    }),
                                )
                                .child(icon_sm(icon, btn_icon_color).w(px(12.0)).h(px(12.0)));
                        } else {
                            // Invisible placeholder to keep alignment or just don't render?
                            // Let's render disabled for consistent layout, or just empty if we want to save space?
                            // "Efficient" usually means consistent locations.
                            btn = btn.child(
                                icon_sm(icon, btn_icon_color.opacity(0.1))
                                    .w(px(12.0))
                                    .h(px(12.0)),
                            );
                        }
                        btn
                    };

                let icon_box = div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(28.0))
                    .h(px(28.0))
                    .rounded(px(4.0))
                    .bg(rgb(0x222222))
                    .child(icon_sm(Icon::Film, item_subtle).w(px(16.0)).h(px(16.0)));

                let info_box = div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(item_text)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(session_label),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(3.0))
                                    .rounded_full()
                                    .bg(progress_bg)
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded_full()
                                            .w(relative(ratio))
                                            .bg(progress_fill),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(item_subtle)
                                    .child(status_str),
                            ),
                    );

                let controls_box = div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(action_btn(Icon::Play, start_enabled, TaskAction::Start, cx))
                    .child(action_btn(
                        Icon::Pause,
                        pause_enabled,
                        TaskAction::Pause,
                        cx,
                    ))
                    .child(action_btn(
                        Icon::Stop,
                        cancel_enabled,
                        TaskAction::Cancel,
                        cx,
                    ));

                let row = div()
                    .id(("task-sidebar-entry", session_id))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .py(px(6.0))
                    .bg(bg_color)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, window, cx| {
                            (this.callbacks.on_select)(session_id, window, cx);
                        }),
                    )
                    .hover(move |s| s.bg(hsla(0.0, 0.0, 1.0, 0.04)))
                    .child(icon_box)
                    .child(info_box)
                    .child(controls_box);

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

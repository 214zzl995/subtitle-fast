use gpui::prelude::*;
use gpui::{
    Context, Div, FontWeight, Hsla, InteractiveElement, Render, Stateful, Task, Window, div, hsla,
    px,
};
use tokio::sync::watch;

use crate::gui::icons::{Icon, icon_sm};

use super::{DetectionHandle, DetectionRunState};

#[derive(Clone, Copy)]
struct ControlButtonStyle {
    bg: Hsla,
    hover_bg: Hsla,
    border: Hsla,
    text: Hsla,
    icon: Hsla,
}

pub struct DetectionControls {
    handle: DetectionHandle,
    run_state: DetectionRunState,
    state_rx: watch::Receiver<DetectionRunState>,
    state_task: Option<Task<()>>,
}

impl DetectionControls {
    pub fn new(handle: DetectionHandle) -> Self {
        let state_rx = handle.subscribe_state();
        let run_state = *state_rx.borrow();
        Self {
            handle,
            run_state,
            state_rx,
            state_task: None,
        }
    }

    pub fn run_state(&self) -> DetectionRunState {
        self.run_state
    }

    fn start_detection(&mut self, cx: &mut Context<Self>) {
        self.handle.start();
        self.sync_run_state();
        cx.notify();
    }

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        self.handle.toggle_pause();
        self.sync_run_state();
        cx.notify();
    }

    fn cancel_detection(&mut self, cx: &mut Context<Self>) {
        self.handle.cancel();
        self.sync_run_state();
        cx.notify();
    }

    fn control_button(
        &self,
        id: &'static str,
        label: &'static str,
        icon: Icon,
        style: ControlButtonStyle,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let icon_view = div()
            .flex_none()
            .w(px(16.0))
            .h(px(16.0))
            .items_center()
            .justify_center()
            .child(icon_sm(icon, style.icon).w(px(14.0)).h(px(14.0)));

        div()
            .id((id, cx.entity_id()))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(4.0))
            .h(px(30.0))
            .px(px(12.0))
            .min_w(px(0.0))
            .rounded(px(8.0))
            .bg(style.bg)
            .border_1()
            .border_color(style.border)
            .text_size(px(12.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(style.text)
            .hover(move |s| s.bg(style.hover_bg))
            .child(icon_view)
            .child(label)
    }

    fn sync_run_state(&mut self) {
        let next = *self.state_rx.borrow();
        if self.run_state != next {
            self.run_state = next;
        }
    }

    fn ensure_state_listener(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state_task.is_some() {
            return;
        }
        let mut state_rx = self.state_rx.clone();
        let entity_id = cx.entity_id();
        let task = window.spawn(cx, async move |cx| {
            loop {
                if state_rx.changed().await.is_err() {
                    break;
                }
                if cx
                    .update(|_window, cx| {
                        cx.notify(entity_id);
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        self.state_task = Some(task);
    }
}

impl Render for DetectionControls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let container_id = ("detection-controls", cx.entity_id());
        let base_bg = hsla(0.0, 0.0, 0.15, 1.0);
        let base_hover = hsla(0.0, 0.0, 0.19, 1.0);
        let base_border = hsla(0.0, 0.0, 0.23, 1.0);
        let base_text = hsla(0.0, 0.0, 1.0, 0.9);
        let base_icon = hsla(0.0, 0.0, 1.0, 0.85);

        let start_bg = hsla(0.0, 0.0, 0.86, 1.0);
        let start_hover = hsla(0.0, 0.0, 0.92, 1.0);
        let start_border = hsla(0.0, 0.0, 0.78, 1.0);
        let start_text = hsla(0.0, 0.0, 0.12, 1.0);
        let start_icon = hsla(0.0, 0.0, 0.18, 1.0);

        let cancel_bg = hsla(0.0, 0.5, 0.28, 1.0);
        let cancel_hover = hsla(0.0, 0.56, 0.34, 1.0);
        let cancel_border = hsla(0.0, 0.5, 0.38, 1.0);

        let start_style = ControlButtonStyle {
            bg: start_bg,
            hover_bg: start_hover,
            border: start_border,
            text: start_text,
            icon: start_icon,
        };
        let pause_style = ControlButtonStyle {
            bg: base_bg,
            hover_bg: base_hover,
            border: base_border,
            text: base_text,
            icon: base_icon,
        };
        let cancel_style = ControlButtonStyle {
            bg: cancel_bg,
            hover_bg: cancel_hover,
            border: cancel_border,
            text: base_text,
            icon: base_icon,
        };

        let mut row = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .w_full()
            .min_w(px(0.0))
            .max_w(px(200.0));

        self.ensure_state_listener(window, cx);
        self.sync_run_state();

        if self.run_state == DetectionRunState::Idle {
            let start_button = self
                .control_button(
                    "detection-control-start",
                    "Start Detection",
                    Icon::Scan,
                    start_style,
                    cx,
                )
                .w_full()
                .min_w(px(0.0))
                .cursor_pointer()
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.start_detection(cx);
                }));
            row = row.child(start_button);
        } else {
            let pause_icon = if self.run_state == DetectionRunState::Paused {
                Icon::Play
            } else {
                Icon::Pause
            };
            let pause_label = if self.run_state == DetectionRunState::Paused {
                "Resume"
            } else {
                "Pause"
            };

            let pause_button = self
                .control_button(
                    "detection-control-pause",
                    pause_label,
                    pause_icon,
                    pause_style,
                    cx,
                )
                .flex_1()
                .min_w(px(0.0))
                .cursor_pointer()
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.toggle_pause(cx);
                }));
            let cancel_button = self
                .control_button(
                    "detection-control-cancel",
                    "Stop",
                    Icon::Stop,
                    cancel_style,
                    cx,
                )
                .flex_1()
                .min_w(px(0.0))
                .cursor_pointer()
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.cancel_detection(cx);
                }));
            row = row.child(pause_button).child(cancel_button);
        }

        div()
            .id(container_id)
            .flex()
            .items_center()
            .justify_center()
            .w_full()
            .min_w(px(0.0))
            .child(row)
    }
}

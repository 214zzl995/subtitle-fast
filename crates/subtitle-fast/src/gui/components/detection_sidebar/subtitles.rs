use futures_channel::mpsc::unbounded;
use futures_util::StreamExt;
use gpui::prelude::*;
use gpui::{Context, Render, ScrollHandle, Task, Window, div, hsla, px};

use crate::gui::runtime;
use crate::stage::progress_gui::GuiSubtitleEvent;

use super::{DetectionHandle, DetectionRunState};

#[derive(Clone, Debug)]
struct DetectedSubtitleEntry {
    id: u64,
    start_ms: f64,
    end_ms: f64,
    text: String,
}

impl DetectedSubtitleEntry {
    fn new(id: u64, subtitle: GuiSubtitleEvent) -> Self {
        Self {
            id,
            start_ms: subtitle.start_ms,
            end_ms: subtitle.end_ms,
            text: subtitle.text,
        }
    }
}

pub struct DetectedSubtitlesList {
    handle: DetectionHandle,
    run_state: DetectionRunState,
    subtitles: Vec<DetectedSubtitleEntry>,
    last_subtitle: Option<GuiSubtitleEvent>,
    next_id: u64,
    scroll_handle: ScrollHandle,
    progress_task: Option<Task<()>>,
}

impl DetectedSubtitlesList {
    pub fn new(handle: DetectionHandle) -> Self {
        Self {
            run_state: handle.run_state(),
            handle,
            subtitles: Vec::new(),
            last_subtitle: None,
            next_id: 0,
            scroll_handle: ScrollHandle::new(),
            progress_task: None,
        }
    }

    fn ensure_progress_listener(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.progress_task.is_some() {
            return;
        }

        let handle = self.handle.clone();
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
            let mut last_subtitle = progress_rx.borrow().subtitle.clone();
            let mut last_state = *state_rx.borrow();

            loop {
                tokio::select! {
                    changed = progress_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let subtitle = progress_rx.borrow().subtitle.clone();
                        if subtitle != last_subtitle {
                            last_subtitle = subtitle;
                            if notify_tx.unbounded_send(()).is_err() {
                                break;
                            }
                        }
                    }
                    changed = state_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let state = *state_rx.borrow();
                        if state != last_state {
                            last_state = state;
                            if notify_tx.unbounded_send(()).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });

        if tokio_task.is_none() {
            eprintln!("detection subtitles listener failed: tokio runtime not initialized");
        }

        self.progress_task = Some(task);
    }

    fn sync_run_state(&mut self) {
        let next = self.handle.run_state();
        if self.run_state == DetectionRunState::Idle && next == DetectionRunState::Running {
            self.reset_list();
        }
        self.run_state = next;
    }

    fn sync_subtitles(&mut self) {
        self.sync_run_state();

        let snapshot = self.handle.progress_snapshot();
        let subtitle = snapshot.subtitle.clone();
        if subtitle != self.last_subtitle {
            self.last_subtitle = subtitle.clone();
            if let Some(event) = subtitle {
                self.push_subtitle(event);
            }
        }
    }

    fn reset_list(&mut self) {
        self.subtitles.clear();
        self.last_subtitle = None;
        self.next_id = 0;
    }

    fn push_subtitle(&mut self, subtitle: GuiSubtitleEvent) {
        let entry = DetectedSubtitleEntry::new(self.next_id, subtitle);
        self.next_id = self.next_id.saturating_add(1);
        self.subtitles.push(entry);
    }

    fn subtitle_row(&self, entry: &DetectedSubtitleEntry) -> impl IntoElement {
        let time_color = hsla(0.0, 0.0, 1.0, 0.55);
        let text_color = hsla(0.0, 0.0, 1.0, 0.88);
        let time_text = format!(
            "{} - {}",
            format_timestamp(entry.start_ms),
            format_timestamp(entry.end_ms)
        );

        div()
            .id(("detection-subtitles-row", entry.id))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .w_full()
            .min_w(px(0.0))
            .py(px(2.0))
            .px(px(2.0))
            .child(
                div()
                    .text_size(px(9.0))
                    .text_color(time_color)
                    .child(time_text),
            )
            .child(
                div()
                    .min_w(px(0.0))
                    .text_size(px(11.0))
                    .text_color(text_color)
                    .child(entry.text.clone()),
            )
    }

    fn empty_placeholder(&self, cx: &Context<Self>) -> impl IntoElement {
        let placeholder_color = hsla(0.0, 0.0, 1.0, 0.4);
        div()
            .id(("detection-subtitles-empty", cx.entity_id()))
            .flex()
            .items_center()
            .justify_center()
            .size_full()
            .text_size(px(12.0))
            .text_color(placeholder_color)
            .child("No subtitles detected yet")
    }
}

impl Render for DetectedSubtitlesList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_progress_listener(window, cx);
        self.sync_subtitles();

        let list_body = if self.subtitles.is_empty() {
            div()
                .flex_1()
                .min_h(px(0.0))
                .child(self.empty_placeholder(cx))
        } else {
            let mut rows = div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .w_full()
                .min_w(px(0.0))
                .px(px(2.0))
                .py(px(4.0));

            for entry in &self.subtitles {
                rows = rows.child(self.subtitle_row(entry));
            }
            rows
        };

        let scroll_area = div()
            .id(("detection-subtitles-scroll", cx.entity_id()))
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .scrollbar_width(px(6.0))
            .track_scroll(&self.scroll_handle)
            .child(list_body);

        div()
            .id(("detection-subtitles", cx.entity_id()))
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .child(scroll_area)
    }
}

fn format_timestamp(ms: f64) -> String {
    if !ms.is_finite() || ms <= 0.0 {
        return "0:00.000".to_string();
    }

    let total_ms = ms.round().max(0.0) as u64;
    let total_secs = total_ms / 1000;
    let hours = total_secs / 3600;
    let minutes = (total_secs / 60) % 60;
    let seconds = total_secs % 60;
    let millis = total_ms % 1000;

    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}.{millis:03}")
    } else {
        format!("{minutes}:{seconds:02}.{millis:03}")
    }
}

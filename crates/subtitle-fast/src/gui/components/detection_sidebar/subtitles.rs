use gpui::prelude::*;
use gpui::{Context, Render, ScrollHandle, Task, Window, div, hsla, px};
use tokio::sync::broadcast;

use crate::stage::progress_gui::GuiSubtitleEvent;

use super::{DetectionHandle, SubtitleMessage};

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
    subtitles: Vec<DetectedSubtitleEntry>,
    next_id: u64,
    scroll_handle: ScrollHandle,
    subtitle_task: Option<Task<()>>,
}

impl DetectedSubtitlesList {
    pub fn new(handle: DetectionHandle) -> Self {
        let snapshot = handle.subtitles_snapshot();
        let mut subtitles = Vec::with_capacity(snapshot.len());
        let mut next_id = 0;
        for subtitle in snapshot {
            subtitles.push(DetectedSubtitleEntry::new(next_id, subtitle));
            next_id = next_id.saturating_add(1);
        }

        Self {
            handle,
            subtitles,
            next_id,
            scroll_handle: ScrollHandle::new(),
            subtitle_task: None,
        }
    }

    fn ensure_subtitle_listener(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.subtitle_task.is_some() {
            return;
        }

        let handle = cx.entity();
        let mut subtitle_rx = self.handle.subscribe_subtitles();

        let task = window.spawn(cx, async move |cx| {
            loop {
                match subtitle_rx.recv().await {
                    Ok(message) => {
                        if cx
                            .update(|_window, cx| {
                                let _ = handle.update(cx, |this, cx| {
                                    this.apply_message(message);
                                    cx.notify();
                                });
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if cx
                            .update(|_window, cx| {
                                let _ = handle.update(cx, |this, cx| {
                                    this.resync_from_handle();
                                    cx.notify();
                                });
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        self.subtitle_task = Some(task);
    }

    fn reset_list(&mut self) {
        self.subtitles.clear();
        self.next_id = 0;
    }

    fn push_subtitle(&mut self, subtitle: GuiSubtitleEvent) {
        let entry = DetectedSubtitleEntry::new(self.next_id, subtitle);
        self.next_id = self.next_id.saturating_add(1);
        self.subtitles.push(entry);
    }

    fn apply_message(&mut self, message: SubtitleMessage) {
        match message {
            SubtitleMessage::Reset => self.reset_list(),
            SubtitleMessage::New(subtitle) => self.push_subtitle(subtitle),
        }
    }

    fn resync_from_handle(&mut self) {
        let snapshot = self.handle.subtitles_snapshot();
        self.reset_list();
        for subtitle in snapshot {
            self.push_subtitle(subtitle);
        }
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
        self.ensure_subtitle_listener(window, cx);

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

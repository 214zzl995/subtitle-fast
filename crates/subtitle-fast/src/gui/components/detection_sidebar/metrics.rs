use std::time::{Duration, Instant};

use futures_channel::mpsc::unbounded;
use futures_util::StreamExt;
use gpui::prelude::*;
use gpui::{Context, FontWeight, Hsla, Render, Task, Window, div, hsla, px, relative, rgb};
use tokio::time::MissedTickBehavior;

use crate::gui::icons::{Icon, icon_sm};
use crate::gui::runtime;
use crate::stage::progress_gui::GuiProgressUpdate;

use super::DetectionHandle;

const METRICS_THROTTLE: Duration = Duration::from_millis(500);
const PROGRESS_STEP: f64 = 0.001;

pub struct DetectionMetrics {
    progress: GuiProgressUpdate,
    progress_task: Option<Task<()>>,
    handle: DetectionHandle,
}

impl DetectionMetrics {
    pub fn new(handle: DetectionHandle) -> Self {
        let progress = handle.progress_snapshot();
        Self {
            progress,
            progress_task: None,
            handle,
        }
    }

    fn sync_progress(&mut self) {
        let next = self.handle.progress_snapshot();
        let run_state = self.handle.run_state();
        let effective = if run_state.is_running() || next.completed {
            next
        } else {
            GuiProgressUpdate::default()
        };
        if self.progress != effective {
            self.progress = effective;
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
            let snapshot = progress_rx.borrow().clone();
            let mut last_progress = snapshot.progress;
            let mut last_seen_progress = snapshot.progress;
            let mut last_progress_change_at = Instant::now();
            let mut completed = snapshot.completed;
            let mut running = state_rx.borrow().is_running();

            let mut ticker = tokio::time::interval(METRICS_THROTTLE);
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
                    }
                    _ = ticker.tick() => {
                        if running
                            && !completed
                            && Instant::now().duration_since(last_progress_change_at) >= METRICS_THROTTLE
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
            eprintln!("detection metrics listener failed: tokio runtime not initialized");
        }
        self.progress_task = Some(task);
    }

    fn progress_ratio(&self) -> f32 {
        let mut ratio = self.progress.progress;
        if self.progress.completed && ratio <= 0.0 {
            ratio = 1.0;
        }
        ratio.clamp(0.0, 1.0) as f32
    }

    fn format_frames(&self) -> String {
        match self.progress.total_frames {
            Some(total) if total > 0 => format!("{} / {}", self.progress.samples_seen, total),
            _ => self.progress.samples_seen.to_string(),
        }
    }

    fn format_rate(value: f64, unit: &str) -> String {
        if value <= 0.0 {
            "--".to_string()
        } else {
            format!("{value:.1} {unit}")
        }
    }

    fn metric_row(
        &self,
        id: &'static str,
        icon: Icon,
        label: &'static str,
        value: String,
        label_color: Hsla,
        value_color: Hsla,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let icon_view = icon_sm(icon, label_color).w(px(12.0)).h(px(12.0));

        let left = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .min_w(px(0.0))
            .child(icon_view)
            .child(label);

        div()
            .id((id, cx.entity_id()))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .w_full()
            .min_w(px(0.0))
            .text_size(px(10.0))
            .text_color(label_color)
            .child(left)
            .child(
                div()
                    .flex_none()
                    .text_color(value_color)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(value),
            )
    }
}

impl Render for DetectionMetrics {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_progress_listener(window, cx);
        self.sync_progress();

        let track_bg = rgb(0x2a2a2a);
        let fill_bg = rgb(0xd6d6d6);
        let border = rgb(0x343434);
        let label_color = hsla(0.0, 0.0, 1.0, 0.62);
        let value_color = hsla(0.0, 0.0, 1.0, 0.9);
        let progress_ratio = self.progress_ratio();

        let progress_fill = div()
            .id(("detection-metrics-progress-fill", cx.entity_id()))
            .absolute()
            .top(px(0.0))
            .bottom(px(0.0))
            .left(px(0.0))
            .w(relative(progress_ratio))
            .rounded(px(4.0))
            .bg(fill_bg);

        let progress_bar = div()
            .id(("detection-metrics-progress-bar", cx.entity_id()))
            .relative()
            .w_full()
            .h(px(6.0))
            .rounded(px(4.0))
            .bg(track_bg)
            .border_1()
            .border_color(border)
            .child(progress_fill);

        let rows = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(self.metric_row(
                "detection-metric-frames",
                Icon::Frame,
                "Frames",
                self.format_frames(),
                label_color,
                value_color,
                cx,
            ))
            .child(self.metric_row(
                "detection-metric-fps",
                Icon::Activity,
                "FPS",
                Self::format_rate(self.progress.fps, "fps"),
                label_color,
                value_color,
                cx,
            ))
            .child(self.metric_row(
                "detection-metric-detect",
                Icon::Scan,
                "Detect",
                Self::format_rate(self.progress.det_ms, "ms"),
                label_color,
                value_color,
                cx,
            ))
            .child(self.metric_row(
                "detection-metric-seg",
                Icon::Crosshair,
                "Region",
                Self::format_rate(self.progress.seg_ms, "ms"),
                label_color,
                value_color,
                cx,
            ))
            .child(self.metric_row(
                "detection-metric-ocr",
                Icon::Sparkles,
                "OCR",
                Self::format_rate(self.progress.ocr_ms, "ms"),
                label_color,
                value_color,
                cx,
            ))
            .child(self.metric_row(
                "detection-metric-write",
                Icon::Gauge,
                "Write",
                Self::format_rate(self.progress.writer_ms, "ms"),
                label_color,
                value_color,
                cx,
            ))
            .child(self.metric_row(
                "detection-metric-cues",
                Icon::MessageSquare,
                "Cues",
                self.progress.cues.to_string(),
                label_color,
                value_color,
                cx,
            ))
            .child(self.metric_row(
                "detection-metric-merged",
                Icon::Merge,
                "Merged",
                self.progress.merged.to_string(),
                label_color,
                value_color,
                cx,
            ))
            .child(self.metric_row(
                "detection-metric-empty-ocr",
                Icon::ScanText,
                "Empty OCR",
                self.progress.ocr_empty.to_string(),
                label_color,
                value_color,
                cx,
            ));

        div()
            .id(("detection-metrics", cx.entity_id()))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(progress_bar)
            .child(rows)
    }
}

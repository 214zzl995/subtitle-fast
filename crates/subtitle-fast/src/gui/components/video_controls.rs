use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    Bounds, Context, IsZero, MouseButton, MouseDownEvent, Pixels, Point, Render, Window, div, hsla,
    px, relative, rgb,
};

use crate::gui::components::{VideoPlayerControlHandle, VideoPlayerInfoHandle};
use crate::gui::icons::{Icon, icon_md};

pub struct VideoControls {
    controls: Option<VideoPlayerControlHandle>,
    info: Option<VideoPlayerInfoHandle>,
    paused: bool,
    progress_bounds: Option<Bounds<Pixels>>,
}

impl VideoControls {
    pub fn new() -> Self {
        Self {
            controls: None,
            info: None,
            paused: false,
            progress_bounds: None,
        }
    }

    pub fn set_handles(
        &mut self,
        controls: Option<VideoPlayerControlHandle>,
        info: Option<VideoPlayerInfoHandle>,
    ) {
        self.controls = controls;
        self.info = info;
        self.paused = false;
        self.progress_bounds = None;
    }

    fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        let Some(controls) = self.controls.as_ref() else {
            return;
        };
        controls.toggle_pause();
        self.paused = !self.paused;
        cx.notify();
    }

    fn update_progress_bounds(&mut self, bounds: Option<Bounds<Pixels>>) {
        self.progress_bounds = bounds;
    }

    fn seek_from_position(&mut self, position: Point<Pixels>) {
        let Some(controls) = self.controls.as_ref() else {
            return;
        };
        let Some(info) = self.info.as_ref() else {
            return;
        };
        let Some(bounds) = self.progress_bounds else {
            return;
        };
        if bounds.size.width.is_zero() {
            return;
        }
        let mut ratio = (position.x - bounds.origin.x) / bounds.size.width;
        if !ratio.is_finite() {
            return;
        }
        ratio = ratio.clamp(0.0, 1.0);

        let snapshot = info.snapshot();
        if snapshot.metadata.duration.is_some() {
            if let Some(duration) = snapshot.metadata.duration {
                if duration > Duration::ZERO {
                    let target = duration.as_secs_f64() * ratio as f64;
                    if target.is_finite() && target >= 0.0 {
                        controls.seek_to(Duration::from_secs_f64(target));
                        return;
                    }
                }
            }
        }

        let total_frames = snapshot.metadata.calculate_total_frames().unwrap_or(0);
        if total_frames > 0 {
            let max_index = total_frames.saturating_sub(1);
            let target = (ratio as f64 * max_index as f64).round();
            let frame = target.clamp(0.0, max_index as f64) as u64;
            controls.seek_to_frame(frame);
        }
    }
}

impl Render for VideoControls {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let playback_icon = if self.paused { Icon::Play } else { Icon::Pause };
        let mut current_time = Duration::ZERO;
        let mut total_time = Duration::ZERO;
        let mut current_frame_index = 0u64;
        let mut current_frame_display = 0u64;
        let mut total_frames = 0u64;

        if let Some(info) = self.info.as_ref() {
            let snapshot = info.snapshot();
            if let Some(timestamp) = snapshot.last_timestamp {
                current_time = timestamp;
            } else if let (Some(frame_index), Some(fps)) =
                (snapshot.last_frame_index, snapshot.metadata.fps)
            {
                if fps > 0.0 {
                    current_time = Duration::from_secs_f64(frame_index as f64 / fps);
                }
            }

            if let Some(duration) = snapshot.metadata.duration {
                total_time = duration;
            } else if let (Some(total), Some(fps)) = (
                snapshot.metadata.calculate_total_frames(),
                snapshot.metadata.fps,
            ) {
                if fps > 0.0 {
                    total_time = Duration::from_secs_f64(total as f64 / fps);
                }
            }

            if let Some(frame_index) = snapshot.last_frame_index {
                current_frame_index = frame_index;
                current_frame_display = frame_index.saturating_add(1);
            }
            total_frames = snapshot.metadata.calculate_total_frames().unwrap_or(0);
        }

        let progress = if total_time.as_secs_f64() > 0.0 {
            (current_time.as_secs_f64() / total_time.as_secs_f64()).clamp(0.0, 1.0) as f32
        } else if total_frames > 0 {
            (current_frame_index as f64 / total_frames as f64).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };

        let time_text = format!("{}-{}", format_time(current_time), format_time(total_time));
        let frame_text = format!("{current_frame_display}-{total_frames}");
        let info_width = px(160.0);

        let playback_button = div()
            .id(("toggle-playback", cx.entity_id()))
            .flex()
            .items_center()
            .justify_center()
            .w(px(40.0))
            .h(px(40.0))
            .rounded(px(999.0))
            .border_1()
            .border_color(hsla(0.0, 0.0, 1.0, 0.35))
            .cursor_pointer()
            .hover(|style| style.bg(hsla(0.0, 0.0, 1.0, 0.08)))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.toggle_playback(cx);
            }))
            .child(icon_md(playback_icon, hsla(0.0, 0.0, 1.0, 0.85)));

        let progress_bar = div()
            .flex()
            .flex_1()
            .h(px(8.0))
            .rounded(px(999.0))
            .bg(rgb(0x2a2a2a))
            .overflow_hidden()
            .child(div().h_full().w(relative(progress)).bg(rgb(0x4d9bf5)));
        let progress_bar = {
            let handle = cx.entity();
            div()
                .flex()
                .flex_1()
                .cursor_pointer()
                .on_children_prepainted(move |bounds, _window, cx| {
                    let bounds = bounds.first().copied();
                    let _ = handle.update(cx, |this, _| {
                        this.update_progress_bounds(bounds);
                    });
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _window, _cx| {
                        this.seek_from_position(event.position);
                    }),
                )
                .child(progress_bar)
        };

        let info_row = div()
            .flex()
            .w_full()
            .justify_between()
            .text_sm()
            .text_color(hsla(0.0, 0.0, 1.0, 0.6))
            .child(
                div()
                    .flex()
                    .justify_start()
                    .w(info_width)
                    .min_w(info_width)
                    .max_w(info_width)
                    .text_left()
                    .child(frame_text),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .w(info_width)
                    .min_w(info_width)
                    .max_w(info_width)
                    .text_right()
                    .child(time_text),
            );

        div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .w_full()
            .p(px(12.0))
            .rounded(px(12.0))
            .bg(rgb(0x111111))
            .id(("video-controls", cx.entity_id()))
            .child(playback_button)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap(px(6.0))
                    .child(progress_bar)
                    .child(info_row),
            )
    }
}

fn format_time(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    format!("{minutes}:{seconds:02}")
}

use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, Bounds, Context, DispatchPhase, IsZero, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, Window, div,
    ease_out_quint, hsla, px, relative, rgb,
};

use crate::gui::components::{VideoPlayerControlHandle, VideoPlayerInfoHandle};
use crate::gui::icons::{Icon, icon_md};
use subtitle_fast_decoder::VideoMetadata;

pub struct VideoControls {
    controls: Option<VideoPlayerControlHandle>,
    info: Option<VideoPlayerInfoHandle>,
    paused: bool,
    pending_paused: Option<bool>,
    seek: SeekDragState,
    progress_hovered: bool,
    progress_hover_from: bool,
    progress_hover_token: u64,
}

struct SeekDragState {
    progress_bounds: Option<Bounds<Pixels>>,
    dragging: bool,
    last_seek_at: Option<Instant>,
    drag_ratio: Option<f32>,
    last_seek_ratio: Option<f32>,
    pending_ratio: Option<f32>,
}

impl SeekDragState {
    fn new() -> Self {
        Self {
            progress_bounds: None,
            dragging: false,
            last_seek_at: None,
            drag_ratio: None,
            last_seek_ratio: None,
            pending_ratio: None,
        }
    }

    fn reset_all(&mut self) {
        *self = Self::new();
    }

    fn reset_dragging(&mut self) {
        self.dragging = false;
        self.last_seek_at = None;
        self.drag_ratio = None;
        self.last_seek_ratio = None;
    }
}

impl VideoControls {
    const SEEK_THROTTLE: Duration = Duration::from_millis(100);
    const RELEASE_EPSILON: f32 = 0.002;

    pub fn new() -> Self {
        Self {
            controls: None,
            info: None,
            paused: false,
            pending_paused: None,
            seek: SeekDragState::new(),
            progress_hovered: false,
            progress_hover_from: false,
            progress_hover_token: 0,
        }
    }

    pub fn set_handles(
        &mut self,
        controls: Option<VideoPlayerControlHandle>,
        info: Option<VideoPlayerInfoHandle>,
    ) {
        if let Some(previous) = self.controls.as_ref() {
            previous.end_scrub();
        }
        self.controls = controls;
        self.info = info;
        self.paused = false;
        self.pending_paused = None;
        self.seek.reset_all();
        self.progress_hovered = false;
        self.progress_hover_from = false;
        self.progress_hover_token = 0;
    }

    fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        let Some(controls) = self.controls.as_ref() else {
            return;
        };
        controls.toggle_pause();
        self.paused = !self.paused;
        self.pending_paused = Some(self.paused);
        cx.notify();
    }

    fn sync_paused(&mut self, paused: bool) {
        if let Some(pending) = self.pending_paused {
            if pending == paused {
                self.pending_paused = None;
                self.paused = paused;
            }
        } else if self.paused != paused {
            self.paused = paused;
        }
    }

    fn update_progress_bounds(&mut self, bounds: Option<Bounds<Pixels>>) {
        self.seek.progress_bounds = bounds;
    }

    fn progress_ratio_from_position(&self, position: Point<Pixels>) -> Option<f32> {
        let Some(bounds) = self.seek.progress_bounds else {
            return None;
        };
        if bounds.size.width.is_zero() {
            return None;
        }
        let mut ratio = (position.x - bounds.origin.x) / bounds.size.width;
        if !ratio.is_finite() {
            return None;
        }
        ratio = ratio.clamp(0.0, 1.0);
        Some(ratio)
    }

    fn seek_from_ratio(&mut self, ratio: f32) -> bool {
        let Some(controls) = self.controls.as_ref() else {
            return false;
        };
        let Some(info) = self.info.as_ref() else {
            return false;
        };

        let snapshot = info.snapshot();
        if snapshot.metadata.duration.is_some() {
            if let Some(duration) = snapshot.metadata.duration {
                if duration > Duration::ZERO {
                    let target = duration.as_secs_f64() * ratio as f64;
                    if target.is_finite() && target >= 0.0 {
                        controls.seek_to(Duration::from_secs_f64(target));
                        return true;
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
            return true;
        }
        false
    }

    fn update_drag_ratio(&mut self, position: Point<Pixels>) {
        self.seek.drag_ratio = self.progress_ratio_from_position(position);
    }

    fn seek_from_position_throttled(&mut self, position: Point<Pixels>, now: Instant, force: bool) {
        let ratio = self
            .seek
            .drag_ratio
            .or_else(|| self.progress_ratio_from_position(position));
        let Some(ratio) = ratio else {
            return;
        };
        if !force {
            if let Some(last) = self.seek.last_seek_at {
                if now.duration_since(last) < Self::SEEK_THROTTLE {
                    return;
                }
            }
        }
        if self.seek_from_ratio(ratio) {
            self.seek.last_seek_at = Some(now);
            self.seek.last_seek_ratio = Some(ratio);
            self.seek.pending_ratio = Some(ratio);
        }
    }

    fn begin_seek_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(controls) = self.controls.as_ref() {
            controls.begin_scrub();
        }
        self.seek.dragging = true;
        self.seek.last_seek_at = None;
        self.seek.last_seek_ratio = None;
        self.seek.pending_ratio = None;
        self.update_drag_ratio(position);
        self.seek_from_position_throttled(position, Instant::now(), true);
        cx.notify();
    }

    fn set_progress_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if self.progress_hovered == hovered {
            return;
        }
        self.progress_hover_from = self.progress_hovered;
        self.progress_hovered = hovered;
        self.progress_hover_token = self.progress_hover_token.wrapping_add(1);
        cx.notify();
    }

    fn update_seek_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.seek.dragging {
            return;
        }
        self.update_drag_ratio(position);
        self.seek_from_position_throttled(position, Instant::now(), false);
        cx.notify();
    }

    fn end_seek_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.seek.dragging {
            return;
        }
        self.update_drag_ratio(position);
        let should_seek = if self.paused {
            true
        } else {
            match (self.seek.drag_ratio, self.seek.last_seek_ratio) {
                (Some(current), Some(last)) => (current - last).abs() > Self::RELEASE_EPSILON,
                (Some(_), None) => true,
                _ => false,
            }
        };
        if should_seek {
            self.seek_from_position_throttled(position, Instant::now(), true);
        }
        self.seek.reset_dragging();
        if let Some(controls) = self.controls.as_ref() {
            controls.end_scrub();
        }
        cx.notify();
    }
}

impl Render for VideoControls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.seek.dragging {
            let handle = cx.entity();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                let _ = handle.update(cx, |this, cx| {
                    this.update_seek_drag(event.position, cx);
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
                        this.end_seek_drag(event.position, cx);
                    });
                    window.refresh();
                }
            });
        }

        let playback_icon = if self.paused { Icon::Play } else { Icon::Pause };
        let mut current_time = Duration::ZERO;
        let mut total_time = Duration::ZERO;
        let mut current_frame_index = 0u64;
        let mut current_frame_display = 0u64;
        let mut total_frames = 0u64;
        let snapshot = self.info.as_ref().map(|info| info.snapshot());

        if let Some(snapshot) = snapshot {
            self.sync_paused(snapshot.paused);
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

        let actual_progress = if total_time.as_secs_f64() > 0.0 {
            (current_time.as_secs_f64() / total_time.as_secs_f64()).clamp(0.0, 1.0) as f32
        } else if total_frames > 0 {
            let max_index = total_frames.saturating_sub(1).max(1);
            (current_frame_index as f64 / max_index as f64).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };

        let mut preview_ratio = self.seek.drag_ratio;
        if preview_ratio.is_none() {
            if let Some(pending) = self.seek.pending_ratio {
                if (actual_progress - pending).abs() <= Self::RELEASE_EPSILON {
                    self.seek.pending_ratio = None;
                } else {
                    preview_ratio = Some(pending);
                }
            }
        }

        if let (Some(ratio), Some(snapshot)) = (preview_ratio, snapshot) {
            let (preview_time, preview_frame) = preview_from_ratio(ratio, snapshot.metadata);
            if let Some(preview_time) = preview_time {
                current_time = preview_time;
            }
            if let Some(frame_index) = preview_frame {
                current_frame_display = frame_index.saturating_add(1);
            }
        }

        let progress = preview_ratio.unwrap_or(actual_progress);

        let time_text = format!("{}-{}", format_time(current_time), format_time(total_time));
        let frame_text = format!("{current_frame_display}-{total_frames}");

        let hover_from = if self.progress_hover_from {
            1.0_f32
        } else {
            0.0_f32
        };
        let hover_to = if self.progress_hovered {
            1.0_f32
        } else {
            0.0_f32
        };
        let progress_track = if (hover_from - hover_to).abs() < f32::EPSILON {
            build_progress_track(progress, hover_to).into_any_element()
        } else {
            let animation =
                Animation::new(Duration::from_millis(180)).with_easing(ease_out_quint());
            let token = self.progress_hover_token;
            let animation_id = (
                gpui::ElementId::from(("progress-hover", cx.entity_id())),
                token.to_string(),
            );
            build_progress_track(progress, hover_from)
                .with_animation(animation_id, animation, move |_track, delta| {
                    let mix = hover_from + (hover_to - hover_from) * delta;
                    build_progress_track(progress, mix)
                })
                .into_any_element()
        };

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

        let progress_bar = {
            let handle = cx.entity();
            div()
                .flex()
                .flex_1()
                .h(px(24.0))
                .items_center()
                .cursor_pointer()
                .on_children_prepainted(move |bounds, _window, cx| {
                    let bounds = bounds.first().copied();
                    let _ = handle.update(cx, |this, _| {
                        this.update_progress_bounds(bounds);
                    });
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                        this.begin_seek_drag(event.position, cx);
                    }),
                )
                .child(progress_track)
                .id(("progress-track", cx.entity_id()))
                .on_hover(cx.listener(|this, hovered, _window, cx| {
                    this.set_progress_hovered(*hovered, cx);
                }))
        };

        let info_row = div()
            .flex()
            .gap(px(24.0))
            .text_sm()
            .text_color(hsla(0.0, 0.0, 1.0, 0.6))
            .child(div().flex().child(frame_text))
            .child(div().flex().child(time_text));

        div()
            .flex()
            .flex_col()
            .w_full()
            .p(px(12.0))
            .rounded(px(12.0))
            .bg(rgb(0x111111))
            .id(("video-controls", cx.entity_id()))
            .child(progress_bar)
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .child(playback_button)
                    .child(info_row),
            )
    }
}

fn build_progress_track(progress: f32, mix: f32) -> gpui::Div {
    let track_base = 4.0;
    let track_hover = 6.0;
    let thumb_base = 4.0;
    let thumb_hover = 12.0;
    let thumb_opacity_base = 0.5;
    let thumb_opacity_hover = 1.0;

    let track_height = track_base + (track_hover - track_base) * mix;
    let track_radius = track_height / 2.0;
    let thumb_size = thumb_base + (thumb_hover - thumb_base) * mix;
    let thumb_radius = thumb_size / 2.0;
    let thumb_opacity = thumb_opacity_base + (thumb_opacity_hover - thumb_opacity_base) * mix;

    div()
        .w_full()
        .h(px(track_height))
        .rounded(px(track_radius))
        .bg(hsla(0.0, 0.0, 1.0, 0.15))
        .child(
            div()
                .h_full()
                .w(relative(progress))
                .bg(hsla(0.0, 0.0, 1.0, 1.0))
                .rounded(px(track_radius))
                .relative()
                .child(
                    div()
                        .absolute()
                        .right(px(-thumb_radius))
                        .h_full()
                        .w(px(thumb_size))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .bg(hsla(0.0, 0.0, 1.0, 1.0))
                                .shadow_sm()
                                .opacity(thumb_opacity)
                                .w(px(thumb_size))
                                .h(px(thumb_size))
                                .rounded(px(thumb_radius)),
                        ),
                ),
        )
}

fn format_time(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    format!("{minutes}:{seconds:02}")
}

fn preview_from_ratio(ratio: f32, metadata: VideoMetadata) -> (Option<Duration>, Option<u64>) {
    let ratio = ratio.clamp(0.0, 1.0) as f64;
    let total_frames = metadata.calculate_total_frames();

    if let Some(duration) = metadata.duration {
        if duration > Duration::ZERO {
            let seconds = duration.as_secs_f64() * ratio;
            if seconds.is_finite() && seconds >= 0.0 {
                let time = Duration::from_secs_f64(seconds);
                let frame = if let Some(fps) = metadata.fps {
                    if fps.is_finite() && fps > 0.0 {
                        let frame = seconds * fps;
                        if frame.is_finite() && frame >= 0.0 {
                            Some(frame.round() as u64)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    total_frames.and_then(|total| {
                        if total > 0 {
                            let max_index = total.saturating_sub(1);
                            let target = (ratio * max_index as f64).round();
                            Some(target.clamp(0.0, max_index as f64) as u64)
                        } else {
                            None
                        }
                    })
                };
                return (Some(time), frame);
            }
        }
    }

    if let Some(total) = total_frames {
        if total > 0 {
            let max_index = total.saturating_sub(1);
            let target = (ratio * max_index as f64).round();
            let frame = target.clamp(0.0, max_index as f64) as u64;
            let time = metadata.fps.and_then(|fps| {
                if fps.is_finite() && fps > 0.0 {
                    let seconds = frame as f64 / fps;
                    if seconds.is_finite() && seconds >= 0.0 {
                        Some(Duration::from_secs_f64(seconds))
                    } else {
                        None
                    }
                } else {
                    None
                }
            });
            return (time, Some(frame));
        }
    }

    (None, None)
}

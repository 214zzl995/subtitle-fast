use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, BorderStyle, Bounds, BoxShadow, Context, Corners, DispatchPhase,
    FontWeight, Half, IsZero, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, Render, Window, canvas, div, hsla, point, px, quad, rgb, size, transparent_black,
};

use crate::gui::components::video_player::VideoPlayerInfoSnapshot;
use crate::gui::components::{VideoPlayerControlHandle, VideoPlayerInfoHandle};
use crate::gui::icons::{Icon, icon_sm};
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
    playback_hovered: bool,
    jog_hovered: Option<JogButton>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JogButton {
    Minus1s,
    Plus1s,
    Minus7f,
    Plus7f,
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
            playback_hovered: false,
            jog_hovered: None,
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
        self.paused = true;
        self.pending_paused = None;
        self.seek.reset_all();
        self.progress_hovered = false;
        self.progress_hover_from = false;
        self.progress_hover_token = 0;
        self.playback_hovered = false;
        self.jog_hovered = None;
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

    fn seek_relative_time(&mut self, delta_seconds: f64) -> bool {
        let Some(controls) = self.controls.as_ref() else {
            return false;
        };
        let Some(info) = self.info.as_ref() else {
            return false;
        };
        let snapshot = info.snapshot();
        if snapshot.ended && delta_seconds > 0.0 {
            return false;
        }
        let Some(current_time) = current_time_from_snapshot(&snapshot) else {
            return false;
        };

        let mut target = current_time.as_secs_f64() + delta_seconds;
        if !target.is_finite() {
            return false;
        }
        if let Some(max_seconds) = max_time_seconds(&snapshot) {
            target = target.clamp(0.0, max_seconds);
        } else if target < 0.0 {
            target = 0.0;
        }

        controls.seek_to(Duration::from_secs_f64(target));
        true
    }

    fn seek_relative_frames(&mut self, delta_frames: i64) -> bool {
        let Some(controls) = self.controls.as_ref() else {
            return false;
        };
        let Some(info) = self.info.as_ref() else {
            return false;
        };
        let snapshot = info.snapshot();
        if snapshot.ended && delta_frames > 0 {
            return false;
        }
        let Some(current_frame) = current_frame_from_snapshot(&snapshot) else {
            return false;
        };

        let mut target = current_frame as i64 + delta_frames;
        if target < 0 {
            target = 0;
        }
        let total_frames = snapshot.metadata.calculate_total_frames().unwrap_or(0);
        if total_frames > 0 {
            let max_index = total_frames.saturating_sub(1) as i64;
            if target > max_index {
                target = max_index;
            }
        }

        controls.seek_to_frame(target as u64);
        true
    }

    fn update_progress_bounds(&mut self, bounds: Option<Bounds<Pixels>>) {
        self.seek.progress_bounds = bounds;
    }

    fn progress_bounds_contains(&self, position: Point<Pixels>) -> bool {
        self.seek
            .progress_bounds
            .map(|bounds| bounds.contains(&position))
            .unwrap_or(false)
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
        self.set_progress_hovered(true, cx);
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

    fn reset_progress_hover(&mut self, cx: &mut Context<Self>) {
        if !self.progress_hovered && !self.progress_hover_from {
            return;
        }
        self.progress_hovered = false;
        self.progress_hover_from = false;
        self.progress_hover_token = self.progress_hover_token.wrapping_add(1);
        cx.notify();
    }

    fn set_playback_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if self.playback_hovered == hovered {
            return;
        }
        self.playback_hovered = hovered;
        cx.notify();
    }

    fn set_jog_hovered(&mut self, hovered: bool, button: JogButton, cx: &mut Context<Self>) {
        let next = if hovered {
            Some(button)
        } else if self.jog_hovered == Some(button) {
            None
        } else {
            self.jog_hovered
        };
        if self.jog_hovered == next {
            return;
        }
        self.jog_hovered = next;
        cx.notify();
    }

    fn reset_jog_hover(&mut self, cx: &mut Context<Self>) {
        if self.jog_hovered.is_none() {
            return;
        }
        self.jog_hovered = None;
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
        let hovered = self.progress_bounds_contains(position);
        self.set_progress_hovered(hovered, cx);
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

        let time_value_text = format_time(current_time);
        let time_total_text = format!("/{}", format_time(total_time));
        let frame_value_text = current_frame_display.to_string();
        let frame_total_text = format!("/{total_frames}");

        let interaction_enabled = self.controls.is_some();
        if !interaction_enabled {
            self.reset_progress_hover(cx);
            self.reset_jog_hover(cx);
        }

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
        let (hover_from, hover_to) = if interaction_enabled {
            (hover_from, hover_to)
        } else {
            (0.0_f32, 0.0_f32)
        };
        let progress_canvas = if (hover_from - hover_to).abs() < f32::EPSILON {
            build_progress_canvas(progress, hover_to).into_any_element()
        } else {
            let animation = Animation::new(Duration::from_millis(230)).with_easing(css_ease);
            let token = self.progress_hover_token;
            let animation_id = (
                gpui::ElementId::from(("progress-hover", cx.entity_id())),
                token.to_string(),
            );
            build_progress_canvas(progress, hover_from)
                .with_animation(animation_id, animation, move |_track, delta| {
                    let mix = hover_from + (hover_to - hover_from) * delta;
                    build_progress_canvas(progress, mix)
                })
                .into_any_element()
        };

        let playback_bg = if interaction_enabled {
            hsla(0.0, 0.0, 1.0, 0.1)
        } else {
            hsla(0.0, 0.0, 1.0, 0.08)
        };
        let playback_icon_color = if interaction_enabled {
            if self.playback_hovered {
                hsla(0.0, 0.0, 0.0, 1.0)
            } else {
                hsla(0.0, 0.0, 1.0, 1.0)
            }
        } else {
            hsla(0.0, 0.0, 1.0, 0.45)
        };

        let mut playback_button = div()
            .id(("toggle-playback", cx.entity_id()))
            .flex()
            .items_center()
            .justify_center()
            .w(px(32.0))
            .h(px(32.0))
            .rounded(px(999.0))
            .bg(playback_bg)
            .child(
                icon_sm(playback_icon, playback_icon_color)
                    .map(|this| if !self.paused { this } else { this.ml(px(2.0)) }),
            );
        if interaction_enabled {
            playback_button = playback_button
                .hover(|style| style.bg(rgb(0xffffff)))
                .cursor_pointer()
                .on_hover(cx.listener(|this, hovered, _window, cx| {
                    this.set_playback_hovered(*hovered, cx);
                }))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.toggle_playback(cx);
                }));
        }

        let jog_text_color = if interaction_enabled {
            hsla(0.0, 0.0, 1.0, 0.55)
        } else {
            hsla(0.0, 0.0, 1.0, 0.35)
        };
        let jog_hover_text = hsla(0.0, 0.0, 1.0, 1.0);
        let jog_bg = hsla(0.0, 0.0, 1.0, 0.06);
        let jog_hover_bg = hsla(0.0, 0.0, 1.0, 0.1);

        let jog_button = |id: &'static str, label: &'static str, hovered: bool| {
            let text_color = if hovered {
                jog_hover_text
            } else {
                jog_text_color
            };
            let bg_color = if hovered { jog_hover_bg } else { jog_bg };
            div()
                .id((id, cx.entity_id()))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.0))
                .text_color(text_color)
                .bg(bg_color)
                .px(px(8.0))
                .py(px(4.0))
                .child(label)
        };

        let mut jog_minus_1s = jog_button(
            "jog-minus-1s",
            "-1s",
            self.jog_hovered == Some(JogButton::Minus1s),
        )
        .rounded_tl(px(4.0))
        .rounded_bl(px(4.0));
        let mut jog_plus_1s = jog_button(
            "jog-plus-1s",
            "+1s",
            self.jog_hovered == Some(JogButton::Plus1s),
        );
        let mut jog_minus_7f = jog_button(
            "jog-minus-7f",
            "-7f",
            self.jog_hovered == Some(JogButton::Minus7f),
        );
        let mut jog_plus_7f = jog_button(
            "jog-plus-7f",
            "+7f",
            self.jog_hovered == Some(JogButton::Plus7f),
        )
        .rounded_tr(px(4.0))
        .rounded_br(px(4.0));

        if interaction_enabled {
            jog_minus_1s = jog_minus_1s
                .cursor_pointer()
                .on_hover(cx.listener(|this, hovered, _window, cx| {
                    this.set_jog_hovered(*hovered, JogButton::Minus1s, cx);
                }))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    if this.seek_relative_time(-1.0) {
                        cx.notify();
                    }
                }));
            jog_plus_1s = jog_plus_1s
                .cursor_pointer()
                .on_hover(cx.listener(|this, hovered, _window, cx| {
                    this.set_jog_hovered(*hovered, JogButton::Plus1s, cx);
                }))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    if this.seek_relative_time(1.0) {
                        cx.notify();
                    }
                }));
            jog_minus_7f = jog_minus_7f
                .cursor_pointer()
                .on_hover(cx.listener(|this, hovered, _window, cx| {
                    this.set_jog_hovered(*hovered, JogButton::Minus7f, cx);
                }))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    if this.seek_relative_frames(-7) {
                        cx.notify();
                    }
                }));
            jog_plus_7f = jog_plus_7f
                .cursor_pointer()
                .on_hover(cx.listener(|this, hovered, _window, cx| {
                    this.set_jog_hovered(*hovered, JogButton::Plus7f, cx);
                }))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    if this.seek_relative_frames(7) {
                        cx.notify();
                    }
                }));
        }

        let jog_cluster = div()
            .id(("jog-cluster", cx.entity_id()))
            .flex()
            .items_center()
            .gap(px(0.0))
            .p(px(2.0))
            .rounded(px(6.0))
            .bg(hsla(0.0, 0.0, 0.0, 0.2))
            .overflow_hidden()
            .child(jog_minus_1s)
            .child(jog_plus_1s)
            .child(jog_minus_7f)
            .child(jog_plus_7f);

        let progress_bar = {
            let handle = cx.entity();
            let mut bar = div()
                .flex()
                .flex_1()
                .h(px(24.0))
                .items_center()
                .on_children_prepainted(move |bounds, _window, cx| {
                    let bounds = bounds.first().copied();
                    let _ = handle.update(cx, |this, _| {
                        this.update_progress_bounds(bounds);
                    });
                })
                .child(progress_canvas)
                .id(("progress-track", cx.entity_id()));

            if interaction_enabled {
                bar = bar
                    .cursor_pointer()
                    .on_hover(cx.listener(|this, hovered, _window, cx| {
                        if this.seek.dragging {
                            return;
                        }
                        this.set_progress_hovered(*hovered, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.begin_seek_drag(event.position, cx);
                        }),
                    );
            }

            bar
        };

        let info_row = div()
            .flex()
            .items_center()
            .gap(px(18.0))
            .text_size(px(12.0))
            .text_color(hsla(0.0, 0.0, 1.0, 0.55))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(0.0, 0.0, 1.0, 0.4))
                            .child("FRAME:"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(hsla(0.0, 0.0, 1.0, 0.92))
                            .child(frame_value_text)
                            .child(div().opacity(0.5).child(frame_total_text)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(0.0, 0.0, 1.0, 0.4))
                            .child("TIME:"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(hsla(0.0, 0.0, 1.0, 0.92))
                            .child(time_value_text)
                            .child(div().opacity(0.5).child(time_total_text)),
                    ),
            );

        let left_group = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(playback_button)
            .child(jog_cluster);

        div()
            .flex()
            .flex_col()
            .w_full()
            .p(px(10.0))
            .rounded(px(12.0))
            .bg(rgb(0x111111))
            .id(("video-controls", cx.entity_id()))
            .child(progress_bar)
            .gap(px(4.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .child(left_group)
                    .child(info_row),
            )
    }
}

fn build_progress_canvas(progress: f32, mix: f32) -> gpui::Canvas<()> {
    let progress = progress.clamp(0.0, 1.0);
    let mix = mix.clamp(0.0, 1.0);
    let track_base = 4.0;
    let track_hover = 6.0;
    let thumb_base = 6.0;
    let thumb_hover = 14.0;

    canvas(
        move |_, _, _| {},
        move |bounds, _, window, _| {
            let track_height = track_base + (track_hover - track_base) * mix;
            let track_height_px = px(track_height);
            let track_radius = track_height_px.half();
            let center_y = bounds.center().y;
            let track_top = center_y - track_height_px.half();
            let track_bounds = Bounds {
                origin: point(bounds.origin.x, track_top),
                size: size(bounds.size.width, track_height_px),
            };
            let track_corners =
                Corners::from(track_radius).clamp_radii_for_quad_size(track_bounds.size);

            let fill_width = (bounds.size.width * progress).clamp(px(0.0), bounds.size.width);
            let fill_bounds = Bounds {
                origin: track_bounds.origin,
                size: size(fill_width, track_bounds.size.height),
            };
            let fill_radius = fill_width.min(track_height_px).half();
            let fill_corners =
                Corners::from(fill_radius).clamp_radii_for_quad_size(fill_bounds.size);

            let track_bg = hsla(0.0, 0.0, 1.0, 0.15);
            let fill_bg = hsla(0.0, 0.0, 1.0, 1.0);

            window.paint_quad(quad(
                track_bounds,
                track_corners,
                track_bg,
                px(0.0),
                transparent_black(),
                BorderStyle::default(),
            ));

            if fill_width > px(0.0) {
                window.paint_quad(quad(
                    fill_bounds,
                    fill_corners,
                    fill_bg,
                    px(0.0),
                    transparent_black(),
                    BorderStyle::default(),
                ));
            }

            let thumb_size = thumb_base + (thumb_hover - thumb_base) * mix;
            let thumb_size_px = px(thumb_size);
            let thumb_radius = thumb_size_px.half();
            let thumb_center_x = bounds.origin.x + fill_width;
            let thumb_center = point(thumb_center_x, center_y);
            let thumb_bounds = Bounds {
                origin: point(thumb_center.x - thumb_radius, thumb_center.y - thumb_radius),
                size: size(thumb_size_px, thumb_size_px),
            };

            let shadow_alpha = 0.25 * mix;
            if shadow_alpha > 0.0 {
                let shadow = BoxShadow {
                    color: hsla(0.0, 0.0, 0.0, shadow_alpha),
                    offset: point(px(0.0), px(1.0)),
                    blur_radius: px(4.0),
                    spread_radius: px(0.0),
                };
                let thumb_corners =
                    Corners::from(thumb_radius).clamp_radii_for_quad_size(thumb_bounds.size);
                window.paint_shadows(thumb_bounds, thumb_corners, &[shadow]);
            }

            let thumb_opacity = 0.85 + 0.15 * mix;
            let thumb_bg = hsla(0.0, 0.0, 1.0, thumb_opacity);
            window.paint_quad(quad(
                thumb_bounds,
                thumb_radius,
                thumb_bg,
                px(0.0),
                transparent_black(),
                BorderStyle::default(),
            ));
        },
    )
    .size_full()
}

fn css_ease(delta: f32) -> f32 {
    cubic_bezier_ease(delta, 0.25, 0.1, 0.25, 1.0)
}

fn cubic_bezier_ease(delta: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    if delta <= 0.0 {
        return 0.0;
    }
    if delta >= 1.0 {
        return 1.0;
    }

    let sample_x = |t: f32| {
        let inv = 1.0 - t;
        3.0 * inv * inv * t * x1 + 3.0 * inv * t * t * x2 + t * t * t
    };
    let sample_y = |t: f32| {
        let inv = 1.0 - t;
        3.0 * inv * inv * t * y1 + 3.0 * inv * t * t * y2 + t * t * t
    };
    let sample_dx = |t: f32| {
        let inv = 1.0 - t;
        3.0 * inv * inv * x1 + 6.0 * inv * t * (x2 - x1) + 3.0 * t * t * (1.0 - x2)
    };

    let mut t = delta;
    for _ in 0..6 {
        let x = sample_x(t) - delta;
        let dx = sample_dx(t);
        if dx.abs() < 1e-4 {
            break;
        }
        t = (t - x / dx).clamp(0.0, 1.0);
    }

    let mut t0 = 0.0;
    let mut t1 = 1.0;
    for _ in 0..8 {
        let x = sample_x(t);
        if (x - delta).abs() < 1e-4 {
            break;
        }
        if x > delta {
            t1 = t;
        } else {
            t0 = t;
        }
        t = 0.5 * (t0 + t1);
    }

    sample_y(t).clamp(0.0, 1.0)
}

fn format_time(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    format!("{minutes}:{seconds:02}")
}

fn current_time_from_snapshot(snapshot: &VideoPlayerInfoSnapshot) -> Option<Duration> {
    if let Some(timestamp) = snapshot.last_timestamp {
        return Some(timestamp);
    }
    if let (Some(frame_index), Some(fps)) = (snapshot.last_frame_index, snapshot.metadata.fps) {
        if fps.is_finite() && fps > 0.0 {
            let seconds = frame_index as f64 / fps;
            if seconds.is_finite() && seconds >= 0.0 {
                return Some(Duration::from_secs_f64(seconds));
            }
        }
    }
    None
}

fn current_frame_from_snapshot(snapshot: &VideoPlayerInfoSnapshot) -> Option<u64> {
    if let Some(frame_index) = snapshot.last_frame_index {
        return Some(frame_index);
    }
    if let (Some(timestamp), Some(fps)) = (snapshot.last_timestamp, snapshot.metadata.fps) {
        if fps.is_finite() && fps > 0.0 {
            let frame = timestamp.as_secs_f64() * fps;
            if frame.is_finite() && frame >= 0.0 {
                return Some(frame.round() as u64);
            }
        }
    }
    None
}

fn max_time_seconds(snapshot: &VideoPlayerInfoSnapshot) -> Option<f64> {
    if let Some(duration) = snapshot.metadata.duration {
        if duration > Duration::ZERO {
            let seconds = duration.as_secs_f64();
            if seconds.is_finite() && seconds > 0.0 {
                return Some(seconds);
            }
        }
    }

    let total_frames = snapshot.metadata.calculate_total_frames()?;
    let fps = snapshot.metadata.fps?;
    if total_frames > 0 && fps.is_finite() && fps > 0.0 {
        let seconds = total_frames as f64 / fps;
        if seconds.is_finite() && seconds > 0.0 {
            return Some(seconds);
        }
    }
    None
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

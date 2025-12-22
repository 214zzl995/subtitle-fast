use crate::gui::icons::{Icon, icon_sm};
use crate::gui::state::{AppState, PLAYBACK_BUFFER_CAPACITY, PlaybackFrame, PlaybackSession};
use crate::gui::theme::AppTheme;
use futures_util::StreamExt;
use gpui::prelude::*;
use gpui::*;
use gpui::{InteractiveElement, MouseButton};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use subtitle_fast_decoder::{Configuration, FrameError, FrameResult, VideoFrame};
use tokio::sync::mpsc;

pub struct ControlPanel {
    state: Entity<AppState>,
    theme: AppTheme,
    state_subscription: Option<Subscription>,
}

enum DecoderMessage {
    TotalFrames(Option<u64>),
    Frame(FrameResult<VideoFrame>),
}

const DEFAULT_FRAME_MS: f64 = 33.333;
const BACKPRESSURE_WAIT_MS: u64 = 8;

impl ControlPanel {
    pub fn new(state: Entity<AppState>) -> Self {
        Self {
            state,
            theme: AppTheme::dark(),
            state_subscription: None,
        }
    }
}

impl Render for ControlPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_state_subscription(cx);
        let state = self.state.read(cx);
        self.theme = state.get_theme();

        let threshold = state.get_threshold();
        let tolerance = state.get_tolerance();
        let roi = state.get_roi().unwrap_or(crate::gui::state::RoiSelection {
            x: 0.15,
            y: 0.75,
            width: 0.70,
            height: 0.25,
        });
        let playhead = state.playhead_ms();
        let duration = state.duration_ms();
        let playing = state.is_playing();
        let highlight = state.highlight_enabled();

        div()
            .flex()
            .flex_col()
            .w_full()
            .bg(self.theme.surface())
            .px(px(10.0))
            .pt(px(6.0))
            .pb(px(24.0))
            .gap(px(12.0))
            .child(self.render_playback_bar(cx, playhead, duration, playing))
            .child(self.render_slider(
                cx,
                Icon::Sun,
                "Brightness Threshold",
                threshold,
                0.0,
                255.0,
                5.0,
                |state, value| state.set_threshold(value),
            ))
            .child(self.render_slider(
                cx,
                Icon::Gauge,
                "Tolerance",
                tolerance,
                0.0,
                50.0,
                2.0,
                |state, value| state.set_tolerance(value),
            ))
            .child(self.render_selection_section(cx, highlight))
            .child(self.render_selection_info(roi))
    }
}

impl ControlPanel {
    fn ensure_state_subscription(&mut self, cx: &mut Context<Self>) {
        if self.state_subscription.is_some() {
            return;
        }

        let state = self.state.clone();
        self.state_subscription = Some(cx.observe(&state, |_, _, cx| {
            cx.notify();
        }));
    }

    fn render_playback_bar(
        &self,
        cx: &mut Context<Self>,
        playhead: f64,
        duration: f64,
        playing: bool,
    ) -> Div {
        let state_snapshot = self.state.read(cx);
        let progress = if duration > 0.0 {
            (playhead / duration).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let total_frames = state_snapshot.playback_total_frames();
        let decoded_frames = state_snapshot.playback_decoded_frames();
        let current_frame = state_snapshot
            .playback_current_frame_index()
            .unwrap_or_else(|| decoded_frames.saturating_sub(1));
        let frame_label = match total_frames {
            Some(total) => format!("Frame {}/{}", current_frame, total),
            None => format!("Frame {}", current_frame),
        };

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .bg(self.theme.surface_elevated())
            .border_1()
            .border_color(self.theme.border())
            .rounded(px(10.0))
            .px(px(10.0))
            .py(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(30.0))
                            .h(px(30.0))
                            .rounded_full()
                            .bg(self.theme.surface_active())
                            .border_1()
                            .border_color(self.theme.border())
                            .cursor_pointer()
                            .hover(|s| s.bg(self.theme.surface_hover()))
                            .child(icon_sm(
                                if playing { Icon::Pause } else { Icon::Play },
                                self.theme.text_primary(),
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_playback(cx);
                                }),
                            ),
                    )
                    .child(self.render_progress_bar(progress).flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child(format!(
                                "{} / {}",
                                self.format_time(playhead),
                                self.format_time(duration)
                            )),
                    ),
            )
            .child(
                div().flex().items_center().justify_between().child(
                    div()
                        .text_xs()
                        .text_color(self.theme.text_tertiary())
                        .child(frame_label),
                ),
            )
    }

    fn render_progress_bar(&self, progress: f64) -> Div {
        div()
            .relative()
            .w_full()
            .h(px(12.0))
            .child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .right(px(0.0))
                    .top(px(4.0))
                    .h(px(4.0))
                    .rounded_full()
                    .bg(self.theme.border().opacity(0.6)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(4.0))
                    .h(px(4.0))
                    .rounded_full()
                    .bg(self.theme.accent())
                    .w(relative(progress as f32)),
            )
            .child(
                div()
                    .absolute()
                    .top(px(1.0))
                    .left(relative(progress as f32))
                    .ml(px(-5.0))
                    .w(px(10.0))
                    .h(px(10.0))
                    .rounded_full()
                    .bg(self.theme.surface())
                    .border_2()
                    .border_color(self.theme.accent())
                    .shadow_sm()
                    .hover(|s| s.top(px(0.0)).ml(px(-6.0)).w(px(12.0)).h(px(12.0))),
            )
    }

    fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        let state = self.state.clone();
        let session = state.update(cx, |state, cx| {
            if state.get_active_file().is_none() {
                state.set_error_message(Some(
                    "Please select a video before starting playback".to_string(),
                ));
                state.set_playing(false);
                cx.notify();
                return None;
            }

            let now_playing = !state.is_playing();
            state.set_playing(now_playing);
            let session = if now_playing {
                state.start_playback_session()
            } else {
                None
            };
            cx.notify();
            session
        });

        if let Some(session) = session {
            self.spawn_decoder(session, cx);
        }
    }

    pub fn init_decoder(&mut self, cx: &mut Context<Self>) {
        let state = self.state.clone();
        let session = state.update(cx, |state, cx| {
            let session = state.init_playback_for_file();
            cx.notify();
            session
        });

        if let Some(session) = session {
            self.spawn_decoder(session, cx);
        }
    }

    fn render_selection_section(&self, cx: &mut Context<Self>, highlight_enabled: bool) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(icon_sm(Icon::MousePointer, self.theme.text_secondary()))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(self.theme.text_primary())
                            .child("Selection Overview"),
                    ),
            )
            .child(self.selection_item(
                cx,
                Icon::Sun,
                "Brightness Threshold",
                highlight_enabled,
                |state| {
                    state.toggle_highlight();
                },
            ))
    }

    fn render_selection_info(&self, roi: crate::gui::state::RoiSelection) -> Div {
        div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(icon_sm(Icon::Crosshair, self.theme.text_secondary()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child("Region"),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.text_secondary())
                    .child(format!(
                        "x{:.2} y{:.2} w{:.2} h{:.2}",
                        roi.x, roi.y, roi.width, roi.height
                    )),
            )
    }

    fn selection_item(
        &self,
        cx: &mut Context<Self>,
        icon: Icon,
        label: &str,
        active: bool,
        toggle: impl Fn(&AppState) + 'static,
    ) -> Div {
        let state = self.state.clone();
        let icon_color = if active {
            self.theme.accent()
        } else {
            self.theme.text_secondary()
        };
        let text_color = if active {
            self.theme.text_primary()
        } else {
            self.theme.text_secondary()
        };

        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .child(icon_sm(icon, icon_color))
            .child(
                div()
                    .text_xs()
                    .text_color(text_color)
                    .child(label.to_string()),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, _, _, cx| {
                    state.update(cx, |state, cx| {
                        toggle(state);
                        cx.notify();
                    });
                }),
            )
    }

    fn render_slider(
        &self,
        _cx: &mut Context<Self>,
        icon: Icon,
        label: &str,
        value: f64,
        min: f64,
        max: f64,
        _step: f64,
        _update: fn(&AppState, f64),
    ) -> Div {
        let ratio = ((value - min) / (max - min)).clamp(0.0, 1.0) as f32;

        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .w(px(150.0))
                    .child(icon_sm(icon, self.theme.text_secondary()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child(label.to_string()),
                    ),
            )
            .child(
                div()
                    .w(px(36.0))
                    .text_right()
                    .text_xs()
                    .text_color(self.theme.text_secondary())
                    .child(format!("{:.0}", value)),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .h(px(12.0))
                    .child(
                        div()
                            .absolute()
                            .left(px(0.0))
                            .right(px(0.0))
                            .top(px(4.0))
                            .h(px(4.0))
                            .rounded_full()
                            .bg(self.theme.border().opacity(0.6)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(0.0))
                            .top(px(4.0))
                            .h(px(4.0))
                            .rounded_full()
                            .bg(self.theme.accent())
                            .w(relative(ratio)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(1.0))
                            .left(relative(ratio))
                            .ml(px(-5.0))
                            .w(px(10.0))
                            .h(px(10.0))
                            .rounded_full()
                            .bg(self.theme.surface())
                            .border_2()
                            .border_color(self.theme.accent())
                            .shadow_sm()
                            .hover(|s| s.top(px(0.0)).ml(px(-6.0)).w(px(12.0)).h(px(12.0))),
                    ),
            )
    }

    fn format_time(&self, ms: f64) -> String {
        let total_secs = (ms / 1000.0).round() as u64;
        let minutes = total_secs / 60;
        let seconds = total_secs % 60;
        format!("{:02}:{:02}", minutes, seconds)
    }

    fn spawn_decoder(&self, session: PlaybackSession, cx: &mut Context<Self>) {
        let state = self.state.clone();
        let (tx, rx) = mpsc::channel::<DecoderMessage>(PLAYBACK_BUFFER_CAPACITY);
        let path = session.path.clone();
        let session_id = session.session_id;

        thread::spawn(move || {
            let config = Configuration {
                input: Some(path),
                channel_capacity: NonZeroUsize::new(PLAYBACK_BUFFER_CAPACITY),
                ..Default::default()
            };
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    let _ =
                        tx.blocking_send(DecoderMessage::Frame(Err(FrameError::configuration(
                            format!("Failed to start decoder runtime: {err}"),
                        ))));
                    return;
                }
            };
            runtime.block_on(async move {
                let provider = match config.create_provider() {
                    Ok(provider) => provider,
                    Err(err) => {
                        let _ = tx.send(DecoderMessage::Frame(Err(err))).await;
                        return;
                    }
                };
                let total_frames = provider.total_frames();
                let _ = tx.send(DecoderMessage::TotalFrames(total_frames)).await;
                let mut stream = provider.into_stream();
                while let Some(frame) = stream.next().await {
                    if tx.send(DecoderMessage::Frame(frame)).await.is_err() {
                        break;
                    }
                }
            });
        });

        cx.spawn(move |_this, cx: &mut AsyncApp| {
            let mut async_app = (*cx).clone();
            async move {
                let mut receiver = rx;
                let mut last_timestamp_ms: Option<f64> = None;
                loop {
                    let active_session_id =
                        match state.read_with(&async_app, |state, _| state.playback_session_id()) {
                            Ok(active) => active,
                            Err(_) => break,
                        };
                    if active_session_id != session_id {
                        break;
                    }
                    let buffer_len =
                        match state.read_with(&async_app, |state, _| state.playback_buffer_len()) {
                            Ok(len) => len,
                            Err(_) => break,
                        };
                    if buffer_len >= PLAYBACK_BUFFER_CAPACITY {
                        Timer::after(Duration::from_millis(BACKPRESSURE_WAIT_MS)).await;
                        continue;
                    }

                    let message = receiver.recv().await;
                    let Some(message) = message else {
                        let _ = state.update(&mut async_app, |state, cx| {
                            state.mark_playback_finished(session_id);
                            cx.notify();
                        });
                        break;
                    };

                    match message {
                        DecoderMessage::TotalFrames(total) => {
                            let _ = state.update(&mut async_app, |state, cx| {
                                state.set_playback_total_frames(session_id, total);
                                cx.notify();
                            });
                        }
                        DecoderMessage::Frame(result) => match result {
                            Ok(frame) => {
                                let timestamp_ms = frame
                                    .timestamp()
                                    .map(|ts| ts.as_secs_f64() * 1000.0)
                                    .or_else(|| {
                                        frame
                                            .frame_index()
                                            .map(|index| index as f64 * DEFAULT_FRAME_MS)
                                    })
                                    .unwrap_or_else(|| {
                                        let next =
                                            last_timestamp_ms.unwrap_or(0.0) + DEFAULT_FRAME_MS;
                                        next
                                    });
                                last_timestamp_ms = Some(timestamp_ms);

                                let image = match RenderImage::from_nv12(
                                    frame.width(),
                                    frame.height(),
                                    frame.y_stride(),
                                    frame.uv_stride(),
                                    frame.y_plane().to_vec(),
                                    frame.uv_plane().to_vec(),
                                ) {
                                    Ok(image) => Arc::new(image),
                                    Err(err) => {
                                        let _ = state.update(&mut async_app, |state, cx| {
                                            let message =
                                                format!("Failed to render NV12 frame: {err}");
                                            state.set_playback_error(
                                                session_id,
                                                Some(message.clone()),
                                            );
                                            state.set_error_message(Some(message));
                                            cx.notify();
                                        });
                                        break;
                                    }
                                };

                                let playback_frame =
                                    PlaybackFrame::new(timestamp_ms, frame.frame_index(), image);
                                let _ = state.update(&mut async_app, |state, cx| {
                                    if state.push_playback_frame(session_id, playback_frame) {
                                        if timestamp_ms > state.duration_ms() {
                                            state.set_duration_ms(timestamp_ms);
                                        }
                                        cx.notify();
                                    }
                                });
                            }
                            Err(err) => {
                                let _ = state.update(&mut async_app, |state, cx| {
                                    let message = format!("Decoder error: {err}");
                                    state.set_playback_error(session_id, Some(message.clone()));
                                    state.set_error_message(Some(message));
                                    cx.notify();
                                });
                                break;
                            }
                        },
                    }
                }
            }
        })
        .detach();
    }
}

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use gpui::{
    Context, Frame, ObjectFit, Render, VideoHandle, Window, div, hsla, prelude::*, px, rgb, video,
};
use subtitle_fast_decoder::{
    Configuration, DecoderController, OutputFormat, SeekInfo, SeekMode, VideoFrame, VideoMetadata,
};
use tokio_stream::StreamExt;

#[derive(Clone)]
pub struct VideoPlayerControlHandle {
    inner: Arc<Mutex<ControlState>>,
}

struct ControlState {
    paused: bool,
    scrubbing: bool,
    decoder: Option<DecoderController>,
    seek_timing: Option<SeekTiming>,
    pending_seek: Option<SeekInfo>,
    restart_pending: bool,
    restart_request: Option<RestartRequest>,
}

impl ControlState {
    fn new() -> Self {
        Self {
            paused: false,
            scrubbing: false,
            decoder: None,
            seek_timing: None,
            pending_seek: None,
            restart_pending: false,
            restart_request: None,
        }
    }
}

impl VideoPlayerControlHandle {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ControlState::new())),
        }
    }

    fn set_decoder(&self, controller: DecoderController) {
        let pending = {
            let mut state = self.inner.lock().expect("control state mutex poisoned");
            state.decoder = Some(controller);
            state.restart_pending = false;
            state.restart_request = None;
            state.pending_seek.take()
        };
        if let Some(info) = pending {
            self.send_seek(info);
        }
    }

    fn clear_decoder(&self) {
        let mut state = self.inner.lock().expect("control state mutex poisoned");
        state.decoder = None;
    }

    fn clear_pending_seek(&self) {
        let mut state = self.inner.lock().expect("control state mutex poisoned");
        state.pending_seek = None;
    }

    fn request_restart(&self, request: RestartRequest) {
        let mut state = self.inner.lock().expect("control state mutex poisoned");
        if state.restart_pending {
            return;
        }
        state.restart_pending = true;
        state.restart_request = Some(request);
    }

    fn take_restart_request(&self) -> Option<RestartRequest> {
        let mut state = self.inner.lock().expect("control state mutex poisoned");
        state.restart_request.take()
    }

    fn is_restart_pending(&self) -> bool {
        let state = self.inner.lock().expect("control state mutex poisoned");
        state.restart_pending
    }

    fn reset_seek_timing(&self) {
        let mut state = self.inner.lock().expect("control state mutex poisoned");
        state.seek_timing = None;
    }

    pub fn pause(&self) {
        let mut state = self.inner.lock().expect("control state mutex poisoned");
        state.paused = true;
    }

    pub fn play(&self) {
        let mut state = self.inner.lock().expect("control state mutex poisoned");
        state.paused = false;
    }

    pub fn toggle_pause(&self) {
        let mut state = self.inner.lock().expect("control state mutex poisoned");
        state.paused = !state.paused;
    }

    pub fn begin_scrub(&self) {
        let mut state = self.inner.lock().expect("control state mutex poisoned");
        state.scrubbing = true;
    }

    pub fn end_scrub(&self) {
        let pending = {
            let mut state = self.inner.lock().expect("control state mutex poisoned");
            state.scrubbing = false;
            if state.decoder.is_none() {
                state.pending_seek
            } else {
                None
            }
        };
        if let Some(info) = pending {
            self.request_restart(RestartRequest::Seek(info));
        }
    }

    pub fn seek_to(&self, position: Duration) {
        self.send_seek(SeekInfo::Time {
            position,
            mode: SeekMode::Accurate,
        });
    }

    pub fn seek_to_frame(&self, frame: u64) {
        self.send_seek(SeekInfo::Frame {
            frame,
            mode: SeekMode::Accurate,
        });
    }

    fn is_paused(&self) -> bool {
        let state = self.inner.lock().expect("control state mutex poisoned");
        state.paused
    }

    fn is_scrubbing(&self) -> bool {
        let state = self.inner.lock().expect("control state mutex poisoned");
        state.scrubbing
    }

    fn send_seek(&self, info: SeekInfo) {
        let mut restart = None;
        {
            let mut state = self.inner.lock().expect("control state mutex poisoned");
            if let Some(controller) = state.decoder.as_ref() {
                match controller.seek(info) {
                    Ok(serial) => {
                        state.seek_timing = Some(SeekTiming {
                            serial,
                            sent_at: Instant::now(),
                        });
                    }
                    Err(_) => {
                        state.pending_seek = Some(info);
                        state.decoder = None;
                        restart = Some(RestartRequest::Seek(info));
                    }
                }
            } else {
                state.pending_seek = Some(info);
                restart = Some(RestartRequest::Seek(info));
            }
        }
        if let Some(request) = restart {
            self.request_restart(request);
        }
    }

    fn pending_seek_serial(&self) -> Option<u64> {
        let state = self.inner.lock().expect("control state mutex poisoned");
        state.seek_timing.as_ref().map(|entry| entry.serial)
    }

    fn take_seek_latency(&self, serial: u64) -> Option<Duration> {
        let mut state = self.inner.lock().expect("control state mutex poisoned");
        let entry = state.seek_timing.as_ref()?;
        if entry.serial == serial {
            let elapsed = entry.sent_at.elapsed();
            state.seek_timing = None;
            Some(elapsed)
        } else {
            None
        }
    }
}

struct SeekTiming {
    serial: u64,
    sent_at: Instant,
}

#[derive(Clone, Copy, Debug)]
enum RestartRequest {
    Replay,
    Seek(SeekInfo),
}

#[derive(Clone)]
pub struct VideoPlayerInfoHandle {
    inner: Arc<Mutex<VideoPlayerInfoState>>,
}

impl VideoPlayerInfoHandle {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VideoPlayerInfoState::default())),
        }
    }

    pub fn snapshot(&self) -> VideoPlayerInfoSnapshot {
        let state = self.inner.lock().expect("video info mutex poisoned");
        VideoPlayerInfoSnapshot {
            metadata: state.metadata,
            last_timestamp: state.last_timestamp,
            last_frame_index: state.last_frame_index,
            ended: state.ended,
        }
    }

    fn update(&self, update: impl FnOnce(&mut VideoPlayerInfoState)) {
        let mut state = self.inner.lock().expect("video info mutex poisoned");
        update(&mut state);
    }

    fn reset_for_replay(&self) {
        self.update(|state| {
            state.last_timestamp = None;
            state.last_frame_index = None;
            state.ended = false;
        });
    }

    fn apply_seek_preview(&self, info: SeekInfo) {
        self.update(|state| {
            state.ended = false;
            state.last_timestamp = None;
            state.last_frame_index = None;
            match info {
                SeekInfo::Time { position, .. } => {
                    state.last_timestamp = Some(position);
                    if let Some(fps) = state.metadata.fps {
                        if fps.is_finite() && fps > 0.0 {
                            let frame = position.as_secs_f64() * fps;
                            if frame.is_finite() && frame >= 0.0 {
                                state.last_frame_index = Some(frame.round() as u64);
                            }
                        }
                    }
                }
                SeekInfo::Frame { frame, .. } => {
                    state.last_frame_index = Some(frame);
                    if let Some(fps) = state.metadata.fps {
                        if fps.is_finite() && fps > 0.0 {
                            let seconds = frame as f64 / fps;
                            if seconds.is_finite() && seconds >= 0.0 {
                                state.last_timestamp = Some(Duration::from_secs_f64(seconds));
                            }
                        }
                    }
                }
            }
        });
    }
}

#[derive(Clone, Copy, Debug)]
pub struct VideoPlayerInfoSnapshot {
    pub metadata: VideoMetadata,
    pub last_timestamp: Option<Duration>,
    pub last_frame_index: Option<u64>,
    pub ended: bool,
}

#[derive(Clone, Copy, Debug)]
struct VideoPlayerInfoState {
    metadata: VideoMetadata,
    last_timestamp: Option<Duration>,
    last_frame_index: Option<u64>,
    ended: bool,
}

impl Default for VideoPlayerInfoState {
    fn default() -> Self {
        Self {
            metadata: VideoMetadata::default(),
            last_timestamp: None,
            last_frame_index: None,
            ended: false,
        }
    }
}

pub struct VideoPlayer {
    handle: VideoHandle,
    receiver: Receiver<Frame>,
    sender: SyncSender<Frame>,
    input_path: PathBuf,
    control: VideoPlayerControlHandle,
    info: VideoPlayerInfoHandle,
    generation: Arc<AtomicU64>,
}

impl VideoPlayer {
    pub fn new(
        path: impl Into<PathBuf>,
    ) -> (Self, VideoPlayerControlHandle, VideoPlayerInfoHandle) {
        let path = path.into();
        let handle = VideoHandle::new();
        let (sender, receiver) = sync_channel(1);
        let generation = Arc::new(AtomicU64::new(0));
        let control = VideoPlayerControlHandle::new();
        let info = VideoPlayerInfoHandle::new();

        let generation_token = generation.load(Ordering::SeqCst);
        spawn_decoder(
            sender.clone(),
            path.clone(),
            control.clone(),
            info.clone(),
            Arc::clone(&generation),
            generation_token,
        );

        (
            Self {
                handle,
                receiver,
                sender,
                input_path: path,
                control: control.clone(),
                info: info.clone(),
                generation,
            },
            control,
            info,
        )
    }

    fn restart_decoder(&mut self, request: RestartRequest) {
        let generation_token = self
            .generation
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1);
        self.control.clear_decoder();
        self.control.reset_seek_timing();
        match request {
            RestartRequest::Replay => {
                self.control.clear_pending_seek();
                self.control.play();
                self.control.end_scrub();
                self.info.reset_for_replay();
            }
            RestartRequest::Seek(info) => {
                self.info.apply_seek_preview(info);
            }
        }
        spawn_decoder(
            self.sender.clone(),
            self.input_path.clone(),
            self.control.clone(),
            self.info.clone(),
            Arc::clone(&self.generation),
            generation_token,
        );
    }
}

impl Render for VideoPlayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.request_animation_frame();
        if let Some(request) = self.control.take_restart_request() {
            self.restart_decoder(request);
        }
        let mut latest = None;
        for frame in self.receiver.try_iter() {
            latest = Some(frame);
        }
        if let Some(frame) = latest {
            self.handle.submit(frame);
        }
        let ended = self.info.snapshot().ended;
        let show_replay =
            ended && !self.control.is_scrubbing() && !self.control.is_restart_pending();
        let mut root = div().relative().size_full().bg(rgb(0x111111)).child(
            video(self.handle.clone())
                .object_fit(ObjectFit::Contain)
                .w_full()
                .h_full(),
        );
        if show_replay {
            let replay_button = div()
                .id(("replay-button", cx.entity_id()))
                .flex()
                .items_center()
                .justify_center()
                .px(px(20.0))
                .py(px(10.0))
                .rounded(px(999.0))
                .border_1()
                .border_color(hsla(0.0, 0.0, 1.0, 0.4))
                .bg(hsla(0.0, 0.0, 0.1, 0.7))
                .text_color(hsla(0.0, 0.0, 1.0, 0.9))
                .cursor_pointer()
                .child("Replay");
            let control = self.control.clone();
            let replay_button = replay_button.on_click(cx.listener(move |_, _, _, _| {
                control.request_restart(RestartRequest::Replay);
            }));

            root = root.child(
                div()
                    .id(("replay-overlay", cx.entity_id()))
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(hsla(0.0, 0.0, 0.0, 0.5))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(replay_button),
            );
        }

        root
    }
}

fn spawn_decoder(
    sender: SyncSender<Frame>,
    input_path: PathBuf,
    control: VideoPlayerControlHandle,
    info: VideoPlayerInfoHandle,
    generation: Arc<AtomicU64>,
    generation_token: u64,
) {
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_time()
            .build()
            .expect("failed to create tokio runtime");

        runtime.block_on(async move {
            let is_active = |generation: &Arc<AtomicU64>| {
                generation.load(Ordering::SeqCst) == generation_token
            };
            if !is_active(&generation) {
                return;
            }
            if !input_path.exists() {
                eprintln!("input video not found: {input_path:?}");
                if is_active(&generation) {
                    info.update(|state| state.ended = true);
                }
                return;
            }

            let available = Configuration::available_backends();
            if available.is_empty() {
                eprintln!(
                    "no decoder backend is compiled; enable a backend feature such as backend-ffmpeg"
                );
                if is_active(&generation) {
                    info.update(|state| state.ended = true);
                }
                return;
            }

            let backend = available[0];
            let config = Configuration {
                backend,
                input: Some(input_path),
                channel_capacity: None,
                output_format: OutputFormat::Nv12,
                start_frame: None,
            };

            let provider = match config.create_provider() {
                Ok(provider) => provider,
                Err(err) => {
                    eprintln!("failed to create decoder provider: {err}");
                    if is_active(&generation) {
                        info.update(|state| state.ended = true);
                    }
                    return;
                }
            };

            let metadata = provider.metadata();
            if !is_active(&generation) {
                return;
            }
            info.update(|state| state.metadata = metadata);
            let frame_duration = metadata
                .fps
                .and_then(|fps| (fps > 0.0).then(|| Duration::from_secs_f64(1.0 / fps)));

            let (controller, mut stream) = match provider.open() {
                Ok(value) => value,
                Err(err) => {
                    eprintln!("failed to open decoder stream: {err}");
                    if is_active(&generation) {
                        info.update(|state| state.ended = true);
                    }
                    return;
                }
            };
            if !is_active(&generation) {
                return;
            }
            control.set_decoder(controller);
            let mut started = false;
            let mut start_instant = Instant::now();
            let mut first_timestamp: Option<Duration> = None;
            let mut next_deadline = Instant::now();
            let mut paused_at: Option<Instant> = None;
            let mut active_serial: Option<u64> = None;

            loop {
                if !is_active(&generation) {
                    break;
                }
                let paused = control.is_paused();
                let scrubbing = control.is_scrubbing();
                let paused_like = paused || scrubbing;
                if paused_like {
                    if paused_at.is_none() {
                        paused_at = Some(Instant::now());
                    }
                    if !(scrubbing && control.pending_seek_serial().is_some()) {
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        continue;
                    }
                }

                if !paused_like {
                    if let Some(paused_at) = paused_at.take() {
                        let pause_duration = Instant::now().saturating_duration_since(paused_at);
                        start_instant += pause_duration;
                        next_deadline += pause_duration;
                    }
                }

                let frame = stream.next().await;
                match frame {
                    Some(Ok(frame)) => {
                        if !is_active(&generation) {
                            break;
                        }
                        if let Some(pending) = control.pending_seek_serial() {
                            if frame.serial() != pending {
                                continue;
                            }
                        }
                        if active_serial != Some(frame.serial()) {
                            active_serial = Some(frame.serial());
                            started = false;
                            first_timestamp = None;
                            start_instant = Instant::now();
                            next_deadline = start_instant;
                            paused_at = None;
                        }
                        if let Some(latency) = control.take_seek_latency(frame.serial()) {
                            eprintln!(
                                "seek latency: serial={} elapsed_ms={:.2}",
                                frame.serial(),
                                latency.as_secs_f64() * 1000.0
                            );
                        }
                        if !started {
                            if !paused_like {
                                start_instant = Instant::now();
                                next_deadline = start_instant;
                                started = true;
                            }
                        }

                        if let Some(timestamp) = frame.timestamp() {
                            let first = first_timestamp.get_or_insert(timestamp);
                            if !paused_like {
                                if let Some(delta) = timestamp.checked_sub(*first) {
                                    let target = start_instant + delta;
                                    let now = Instant::now();
                                    if target > now {
                                        tokio::time::sleep(target - now).await;
                                    }
                                }
                            }
                        } else if let Some(duration) = frame_duration {
                            if !paused_like {
                                let now = Instant::now();
                                if next_deadline > now {
                                    tokio::time::sleep(next_deadline - now).await;
                                }
                                next_deadline += duration;
                            }
                        }

                        info.update(|state| {
                            state.last_timestamp = frame.timestamp();
                            state.last_frame_index = frame.frame_index();
                        });

                        if let Some(gpui_frame) = to_gpui_frame(&frame) {
                            if !is_active(&generation) {
                                break;
                            }
                            if sender.send(gpui_frame).is_err() {
                                break;
                            }
                        }
                    }
                    Some(Err(err)) => {
                        eprintln!("decoder error: {err}");
                        break;
                    }
                    None => {
                        break;
                    }
                }
            }

            if is_active(&generation) {
                control.clear_decoder();
                info.update(|state| state.ended = true);
            }
        });
    });
}

fn to_gpui_frame(frame: &VideoFrame) -> Option<Frame> {
    if frame.native().is_some() {
        eprintln!("native frame output is unsupported in this component; use NV12 output");
        return None;
    }

    let y_plane = frame.y_plane().to_vec();
    let uv_plane = frame.uv_plane().to_vec();

    Frame::from_nv12_owned(
        frame.width(),
        frame.height(),
        frame.y_stride(),
        frame.uv_stride(),
        y_plane,
        uv_plane,
    )
    .map_err(|err| {
        eprintln!("failed to build NV12 frame: {err}");
    })
    .ok()
}

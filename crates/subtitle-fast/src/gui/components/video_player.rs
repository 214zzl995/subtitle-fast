use std::path::PathBuf;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use gpui::{
    Context, Frame, ObjectFit, Render, VideoHandle, Window, div, hsla, prelude::*, px, rgb, video,
};
use subtitle_fast_decoder::{
    Backend, Configuration, DecoderController, FrameStream, OutputFormat, SeekInfo, SeekMode,
    VideoFrame, VideoMetadata,
};
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    watch,
};
use tokio_stream::StreamExt;

#[derive(Clone, Copy, Debug)]
pub struct Nv12FrameInfo {
    pub width: u32,
    pub height: u32,
    pub y_stride: usize,
    pub uv_stride: usize,
}

pub type FramePreprocessor = Arc<dyn Fn(&mut [u8], &mut [u8], Nv12FrameInfo) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct VideoPlayerControlHandle {
    sender: UnboundedSender<PlayerCommand>,
}

impl VideoPlayerControlHandle {
    fn new(sender: UnboundedSender<PlayerCommand>) -> Self {
        Self { sender }
    }

    pub fn open(&self, path: impl Into<PathBuf>) {
        let _ = self.sender.send(PlayerCommand::Open(path.into()));
    }

    pub fn pause(&self) {
        let _ = self.sender.send(PlayerCommand::Pause);
    }

    pub fn play(&self) {
        let _ = self.sender.send(PlayerCommand::Play);
    }

    pub fn toggle_pause(&self) {
        let _ = self.sender.send(PlayerCommand::TogglePause);
    }

    pub fn begin_scrub(&self) {
        let _ = self.sender.send(PlayerCommand::BeginScrub);
    }

    pub fn end_scrub(&self) {
        let _ = self.sender.send(PlayerCommand::EndScrub);
    }

    pub fn seek_to(&self, position: Duration) {
        let _ = self.sender.send(PlayerCommand::Seek(SeekInfo::Time {
            position,
            mode: SeekMode::Accurate,
        }));
    }

    pub fn seek_to_frame(&self, frame: u64) {
        let _ = self.sender.send(PlayerCommand::Seek(SeekInfo::Frame {
            frame,
            mode: SeekMode::Accurate,
        }));
    }

    pub fn replay(&self) {
        let _ = self.sender.send(PlayerCommand::Replay);
    }

    pub fn set_preprocessor(&self, preprocessor: FramePreprocessor) {
        let _ = self
            .sender
            .send(PlayerCommand::SetPreprocessor(Some(preprocessor)));
    }

    pub fn clear_preprocessor(&self) {
        let _ = self.sender.send(PlayerCommand::SetPreprocessor(None));
    }
}

#[derive(Clone)]
enum PlayerCommand {
    Open(PathBuf),
    Play,
    Pause,
    TogglePause,
    BeginScrub,
    EndScrub,
    Seek(SeekInfo),
    Replay,
    SetPreprocessor(Option<FramePreprocessor>),
}

#[derive(Clone)]
pub struct VideoPlayerInfoHandle {
    inner: Arc<VideoPlayerInfoInner>,
    playback_rx: watch::Receiver<PlaybackState>,
}

impl VideoPlayerInfoHandle {
    fn new() -> Self {
        let (playback_tx, playback_rx) = watch::channel(PlaybackState::default());
        Self {
            inner: Arc::new(VideoPlayerInfoInner {
                metadata: Mutex::new(VideoMetadata::default()),
                playback_tx,
            }),
            playback_rx,
        }
    }

    pub fn snapshot(&self) -> VideoPlayerInfoSnapshot {
        let metadata = self.metadata();
        let playback = *self.playback_rx.borrow();
        VideoPlayerInfoSnapshot {
            metadata,
            last_timestamp: playback.last_timestamp,
            last_frame_index: playback.last_frame_index,
            has_frame: playback.has_frame,
            paused: playback.paused,
            ended: playback.ended,
            scrubbing: playback.scrubbing,
        }
    }

    fn metadata(&self) -> VideoMetadata {
        *self
            .inner
            .metadata
            .lock()
            .expect("video info mutex poisoned")
    }

    fn set_metadata(&self, metadata: VideoMetadata) {
        let mut guard = self
            .inner
            .metadata
            .lock()
            .expect("video info mutex poisoned");
        *guard = metadata;
    }

    fn update_playback(&self, update: impl FnOnce(&mut PlaybackState)) {
        let _ = self.inner.playback_tx.send_if_modified(|state| {
            let before = *state;
            update(state);
            *state != before
        });
    }

    fn reset_for_replay(&self) {
        self.update_playback(|state| {
            state.last_timestamp = None;
            state.last_frame_index = None;
            state.ended = false;
            state.scrubbing = false;
            state.paused = false;
        });
    }

    fn reset_for_open(&self) {
        self.update_playback(|state| {
            state.last_timestamp = None;
            state.last_frame_index = None;
            state.has_frame = false;
            state.ended = false;
            state.scrubbing = false;
            state.paused = false;
        });
    }

    fn apply_seek_preview(&self, info: SeekInfo) {
        let metadata = self.metadata();
        self.update_playback(|state| {
            state.ended = false;
            state.last_timestamp = None;
            state.last_frame_index = None;
            match info {
                SeekInfo::Time { position, .. } => {
                    state.last_timestamp = Some(position);
                    if let Some(fps) = metadata.fps {
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
                    if let Some(fps) = metadata.fps {
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
    pub has_frame: bool,
    pub paused: bool,
    pub ended: bool,
    pub scrubbing: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct PlaybackState {
    last_timestamp: Option<Duration>,
    last_frame_index: Option<u64>,
    has_frame: bool,
    paused: bool,
    ended: bool,
    scrubbing: bool,
}

struct VideoPlayerInfoInner {
    metadata: Mutex<VideoMetadata>,
    playback_tx: watch::Sender<PlaybackState>,
}

pub struct VideoPlayer {
    handle: VideoHandle,
    receiver: Receiver<Frame>,
    control: VideoPlayerControlHandle,
    info: VideoPlayerInfoHandle,
}

impl VideoPlayer {
    pub fn new() -> (Self, VideoPlayerControlHandle, VideoPlayerInfoHandle) {
        let handle = VideoHandle::new();
        let (sender, receiver) = sync_channel(1);
        let (command_tx, command_rx) = unbounded_channel();
        let control = VideoPlayerControlHandle::new(command_tx);
        let info = VideoPlayerInfoHandle::new();

        spawn_decoder(sender.clone(), command_rx, info.clone());

        (
            Self {
                handle,
                receiver,
                control: control.clone(),
                info: info.clone(),
            },
            control,
            info,
        )
    }
}

impl Render for VideoPlayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.request_animation_frame();
        let mut latest = None;
        for frame in self.receiver.try_iter() {
            latest = Some(frame);
        }
        if let Some(frame) = latest {
            self.handle.submit(frame);
        }
        let snapshot = self.info.snapshot();
        let show_replay = snapshot.ended && !snapshot.scrubbing;
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
                control.replay();
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

struct DecoderSession {
    controller: DecoderController,
    stream: FrameStream,
    frame_duration: Option<Duration>,
}

#[derive(Clone)]
struct CachedFrame {
    width: u32,
    height: u32,
    y_stride: usize,
    uv_stride: usize,
    y_plane: Arc<[u8]>,
    uv_plane: Arc<[u8]>,
}

struct SeekTiming {
    serial: u64,
}

fn open_session(
    backend: Backend,
    input_path: &PathBuf,
    info: &VideoPlayerInfoHandle,
) -> Option<DecoderSession> {
    let config = Configuration {
        backend,
        input: Some(input_path.clone()),
        channel_capacity: None,
        output_format: OutputFormat::Nv12,
        start_frame: None,
    };

    let provider = match config.create_provider() {
        Ok(provider) => provider,
        Err(err) => {
            eprintln!("failed to create decoder provider: {err}");
            info.update_playback(|state| state.ended = true);
            return None;
        }
    };

    let metadata = provider.metadata();
    info.set_metadata(metadata);
    let frame_duration = metadata
        .fps
        .and_then(|fps| (fps > 0.0).then(|| Duration::from_secs_f64(1.0 / fps)));

    let (controller, stream) = match provider.open() {
        Ok(value) => value,
        Err(err) => {
            eprintln!("failed to open decoder stream: {err}");
            info.update_playback(|state| state.ended = true);
            return None;
        }
    };

    Some(DecoderSession {
        controller,
        stream,
        frame_duration,
    })
}

fn handle_command(
    command: PlayerCommand,
    session: Option<&DecoderSession>,
    input_path: &mut Option<PathBuf>,
    paused: &mut bool,
    scrubbing: &mut bool,
    pending_seek: &mut Option<SeekInfo>,
    pending_seek_frame: &mut Option<u64>,
    seek_timing: &mut Option<SeekTiming>,
    open_requested: &mut bool,
    preprocessor: &mut Option<FramePreprocessor>,
    refresh_cached: &mut bool,
    info: &VideoPlayerInfoHandle,
) -> bool {
    match command {
        PlayerCommand::Open(path) => {
            *input_path = Some(path);
            *paused = false;
            *scrubbing = false;
            *pending_seek = None;
            *pending_seek_frame = None;
            *seek_timing = None;
            *open_requested = true;
            info.set_metadata(VideoMetadata::default());
            info.reset_for_open();
        }
        PlayerCommand::Play => {
            *paused = false;
            info.update_playback(|state| state.paused = false);
        }
        PlayerCommand::Pause => {
            *paused = true;
            info.update_playback(|state| state.paused = true);
        }
        PlayerCommand::TogglePause => {
            *paused = !*paused;
            let paused = *paused;
            info.update_playback(|state| state.paused = paused);
        }
        PlayerCommand::BeginScrub => {
            *scrubbing = true;
            info.update_playback(|state| state.scrubbing = true);
        }
        PlayerCommand::EndScrub => {
            *scrubbing = false;
            info.update_playback(|state| state.scrubbing = false);
        }
        PlayerCommand::Seek(seek) => {
            let metadata = info.metadata();
            *pending_seek_frame = match seek {
                SeekInfo::Frame { frame, .. } => Some(frame),
                SeekInfo::Time { position, .. } => metadata.fps.and_then(|fps| {
                    if fps.is_finite() && fps > 0.0 {
                        let frame = position.as_secs_f64() * fps;
                        if frame.is_finite() && frame >= 0.0 {
                            Some(frame.round() as u64)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }),
            };
            info.apply_seek_preview(seek);
            if let Some(session) = session {
                match session.controller.seek(seek) {
                    Ok(serial) => {
                        *pending_seek = None;
                        *seek_timing = Some(SeekTiming { serial });
                    }
                    Err(_) => {
                        *pending_seek = Some(seek);
                        *seek_timing = None;
                        *open_requested = true;
                    }
                }
            } else {
                *pending_seek = Some(seek);
                *seek_timing = None;
                *open_requested = true;
            }
        }
        PlayerCommand::Replay => {
            *paused = false;
            info.update_playback(|state| state.paused = false);
            if *scrubbing {
                *scrubbing = false;
                info.update_playback(|state| state.scrubbing = false);
            }
            *pending_seek = None;
            *pending_seek_frame = None;
            *seek_timing = None;
            *open_requested = true;
            info.reset_for_replay();
        }
        PlayerCommand::SetPreprocessor(hook) => {
            *preprocessor = hook;
            *refresh_cached = true;
        }
    }
    true
}

fn spawn_decoder(
    sender: SyncSender<Frame>,
    mut command_rx: UnboundedReceiver<PlayerCommand>,
    info: VideoPlayerInfoHandle,
) {
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_time()
            .build()
            .expect("failed to create tokio runtime");

        runtime.block_on(async move {
            let available = Configuration::available_backends();
            if available.is_empty() {
                eprintln!(
                    "no decoder backend is compiled; enable a backend feature such as backend-ffmpeg"
                );
                info.update_playback(|state| state.ended = true);
                return;
            }

            let backend = available[0];
            let mut input_path: Option<PathBuf> = None;
            let mut session: Option<DecoderSession> = None;
            let mut open_requested = false;
            let mut paused = false;
            let mut scrubbing = false;
            let mut pending_seek: Option<SeekInfo> = None;
            let mut pending_seek_frame: Option<u64> = None;
            let mut seek_timing: Option<SeekTiming> = None;
            let mut preprocessor: Option<FramePreprocessor> = None;
            let mut last_frame: Option<CachedFrame> = None;

            let mut started = false;
            let mut start_instant = Instant::now();
            let mut first_timestamp: Option<Duration> = None;
            let mut next_deadline = Instant::now();
            let mut paused_at: Option<Instant> = None;
            let mut active_serial: Option<u64> = None;

            loop {
                if session.is_none() {
                    if open_requested {
                        let Some(input_path) = input_path.as_ref() else {
                            open_requested = false;
                            continue;
                        };
                        if !input_path.exists() {
                            eprintln!("input video not found: {input_path:?}");
                            info.update_playback(|state| state.ended = true);
                            open_requested = false;
                            continue;
                        }
                        let new_session = match open_session(backend, input_path, &info) {
                            Some(session) => session,
                            None => return,
                        };

                        if let Some(seek) = pending_seek.take() {
                            match new_session.controller.seek(seek) {
                                Ok(serial) => {
                                    seek_timing = Some(SeekTiming {
                                        serial,
                                    });
                                }
                                Err(_) => {
                                    pending_seek = Some(seek);
                                    seek_timing = None;
                                    open_requested = true;
                                    continue;
                                }
                            }
                        }

                        info.update_playback(|state| state.ended = false);
                        open_requested = false;
                        active_serial = None;
                        started = false;
                        first_timestamp = None;
                        start_instant = Instant::now();
                        next_deadline = start_instant;
                        paused_at = None;
                        session = Some(new_session);
                    } else {
                        let Some(command) = command_rx.recv().await else {
                            break;
                        };
                        let mut refresh_cached = false;
                        if !handle_command(
                            command,
                            session.as_ref(),
                            &mut input_path,
                            &mut paused,
                            &mut scrubbing,
                            &mut pending_seek,
                            &mut pending_seek_frame,
                            &mut seek_timing,
                            &mut open_requested,
                            &mut preprocessor,
                            &mut refresh_cached,
                            &info,
                        ) {
                            break;
                        }
                        if refresh_cached {
                            if let Some(cache) = last_frame.as_ref() {
                                if let Some(gpui_frame) =
                                    frame_from_cache(cache, preprocessor.as_ref())
                                {
                                    let _ = sender.send(gpui_frame);
                                }
                            }
                        }
                    }
                    continue;
                }

                let paused_like = paused || scrubbing;
                if paused_like {
                    if paused_at.is_none() {
                        paused_at = Some(Instant::now());
                    }
                } else if let Some(paused_at) = paused_at.take() {
                    let pause_duration = Instant::now().saturating_duration_since(paused_at);
                    start_instant += pause_duration;
                    next_deadline += pause_duration;
                }

                let allow_seek_frames = seek_timing.is_some();
                if paused_like && !allow_seek_frames {
                    let command = tokio::select! {
                        cmd = command_rx.recv() => cmd,
                        _ = tokio::time::sleep(Duration::from_millis(30)) => None,
                    };
                    if let Some(command) = command {
                        let mut refresh_cached = false;
                        if !handle_command(
                            command,
                            session.as_ref(),
                            &mut input_path,
                            &mut paused,
                            &mut scrubbing,
                            &mut pending_seek,
                            &mut pending_seek_frame,
                            &mut seek_timing,
                            &mut open_requested,
                            &mut preprocessor,
                            &mut refresh_cached,
                            &info,
                        ) {
                            break;
                        }
                        if refresh_cached {
                            if let Some(cache) = last_frame.as_ref() {
                                if let Some(gpui_frame) =
                                    frame_from_cache(cache, preprocessor.as_ref())
                                {
                                    let _ = sender.send(gpui_frame);
                                }
                            }
                        }
                        if open_requested {
                            session = None;
                        }
                    }
                    continue;
                }

                let (frame, frame_duration) = {
                    let session = session.as_mut().expect("session missing");
                    (session.stream.next(), session.frame_duration)
                };
                let mut restart_requested = false;
                tokio::select! {
                    cmd = command_rx.recv() => {
                        let Some(command) = cmd else {
                            break;
                        };
                        let mut refresh_cached = false;
                        if !handle_command(
                            command,
                            session.as_ref(),
                            &mut input_path,
                            &mut paused,
                            &mut scrubbing,
                            &mut pending_seek,
                            &mut pending_seek_frame,
                            &mut seek_timing,
                            &mut open_requested,
                            &mut preprocessor,
                            &mut refresh_cached,
                            &info,
                        ) {
                            break;
                        }
                        if refresh_cached {
                            if let Some(cache) = last_frame.as_ref() {
                                if let Some(gpui_frame) =
                                    frame_from_cache(cache, preprocessor.as_ref())
                                {
                                    let _ = sender.send(gpui_frame);
                                }
                            }
                        }
                        restart_requested = open_requested;
                    }
                    frame = frame => {
                        match frame {
                            Some(Ok(frame)) => {
                                if let Some(pending) = seek_timing.as_ref().map(|entry| entry.serial) {
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
                                // Avoid a 1-frame UI flicker when the first frame after seek
                                // lands within +/-1 of the requested target frame.
                                let mut suppress_seek_frame = false;
                                let clear_seek_timing = if let Some(timing) = seek_timing.as_ref() {
                                    if timing.serial == frame.serial() {
                                        if let (Some(target), Some(actual)) =
                                            (pending_seek_frame, frame.index())
                                        {
                                            if actual.abs_diff(target) <= 1 {
                                                suppress_seek_frame = true;
                                            }
                                        }
                                        true
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                };

                                if clear_seek_timing {
                                    pending_seek_frame = None;
                                    seek_timing = None;
                                }
                                if !started {
                                    if !paused_like {
                                        start_instant = Instant::now();
                                        next_deadline = start_instant;
                                        started = true;
                                    }
                                }

                                if let Some(timestamp) = frame.pts() {
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

                                info.update_playback(|state| {
                                    state.last_timestamp = frame.pts();
                                    if !suppress_seek_frame {
                                        state.last_frame_index = frame.index();
                                    }
                                    state.has_frame = true;
                                });

                                if let Some(cache) = cache_from_video_frame(&frame) {
                                    last_frame = Some(cache.clone());
                                    if let Some(gpui_frame) =
                                        frame_from_cache(&cache, preprocessor.as_ref())
                                    {
                                    if sender.send(gpui_frame).is_err() {
                                        break;
                                    }
                                    }
                                }
                            }
                            Some(Err(err)) => {
                                eprintln!("decoder error: {err}");
                                info.update_playback(|state| state.ended = true);
                                session = None;
                                open_requested = false;
                                seek_timing = None;
                                continue;
                            }
                            None => {
                                info.update_playback(|state| state.ended = true);
                                session = None;
                                open_requested = false;
                                seek_timing = None;
                                continue;
                            }
                        }
                    }
                }

                if restart_requested {
                    session = None;
                    seek_timing = None;
                    active_serial = None;
                    started = false;
                    first_timestamp = None;
                    paused_at = None;
                }
            }
        });
    });
}

fn cache_from_video_frame(frame: &VideoFrame) -> Option<CachedFrame> {
    if frame.native().is_some() {
        eprintln!("native frame output is unsupported in this component; use NV12 output");
        return None;
    }

    let y_plane = Arc::from(frame.y_plane().to_vec().into_boxed_slice());
    let uv_plane = Arc::from(frame.uv_plane().to_vec().into_boxed_slice());

    Some(CachedFrame {
        width: frame.width(),
        height: frame.height(),
        y_stride: frame.y_stride(),
        uv_stride: frame.uv_stride(),
        y_plane,
        uv_plane,
    })
}

fn frame_from_cache(
    cache: &CachedFrame,
    preprocessor: Option<&FramePreprocessor>,
) -> Option<Frame> {
    if let Some(preprocessor) = preprocessor {
        let mut y_plane = cache.y_plane.as_ref().to_vec();
        let mut uv_plane = cache.uv_plane.as_ref().to_vec();
        let info = Nv12FrameInfo {
            width: cache.width,
            height: cache.height,
            y_stride: cache.y_stride,
            uv_stride: cache.uv_stride,
        };
        if !(preprocessor)(&mut y_plane, &mut uv_plane, info) {
            return None;
        }

        Frame::from_nv12_owned(
            cache.width,
            cache.height,
            cache.y_stride,
            cache.uv_stride,
            y_plane,
            uv_plane,
        )
        .map_err(|err| {
            eprintln!("failed to build NV12 frame: {err}");
        })
        .ok()
    } else {
        Frame::from_nv12(
            cache.width,
            cache.height,
            cache.y_stride,
            cache.uv_stride,
            Arc::clone(&cache.y_plane),
            Arc::clone(&cache.uv_plane),
        )
        .map_err(|err| {
            eprintln!("failed to build NV12 frame: {err}");
        })
        .ok()
    }
}

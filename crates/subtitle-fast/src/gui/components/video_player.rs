use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use gpui::{Context, Frame, ObjectFit, Render, VideoHandle, Window, div, prelude::*, rgb, video};
use subtitle_fast_decoder::{
    Configuration, DecoderController, OutputFormat, SeekInfo, SeekMode, VideoFrame, VideoMetadata,
};
use tokio_stream::StreamExt;

#[derive(Clone)]
pub struct VideoPlayerControlHandle {
    paused: Arc<AtomicBool>,
    decoder: Arc<Mutex<Option<DecoderController>>>,
    seek_epoch: Arc<AtomicU64>,
}

impl VideoPlayerControlHandle {
    fn new() -> Self {
        Self {
            paused: Arc::new(AtomicBool::new(false)),
            decoder: Arc::new(Mutex::new(None)),
            seek_epoch: Arc::new(AtomicU64::new(0)),
        }
    }

    fn set_decoder(&self, controller: DecoderController) {
        let mut slot = self
            .decoder
            .lock()
            .expect("decoder controller mutex poisoned");
        self.seek_epoch.store(controller.serial(), Ordering::SeqCst);
        *slot = Some(controller);
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    pub fn play(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    pub fn toggle_pause(&self) {
        let paused = self.paused.load(Ordering::SeqCst);
        self.paused.store(!paused, Ordering::SeqCst);
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
        self.paused.load(Ordering::SeqCst)
    }

    fn send_seek(&self, info: SeekInfo) {
        let slot = self
            .decoder
            .lock()
            .expect("decoder controller mutex poisoned");
        let Some(controller) = slot.as_ref() else {
            return;
        };
        match controller.seek(info) {
            Ok(serial) => {
                self.seek_epoch.store(serial, Ordering::SeqCst);
            }
            Err(err) => {
                eprintln!("decoder seek failed: {err}");
            }
        }
    }

    fn current_seek_epoch(&self) -> u64 {
        self.seek_epoch.load(Ordering::SeqCst)
    }
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
    _info: VideoPlayerInfoHandle,
}

impl VideoPlayer {
    pub fn new(
        path: impl Into<PathBuf>,
    ) -> (Self, VideoPlayerControlHandle, VideoPlayerInfoHandle) {
        let path = path.into();
        let handle = VideoHandle::new();
        let (sender, receiver) = sync_channel(1);
        let control = VideoPlayerControlHandle::new();
        let info = VideoPlayerInfoHandle::new();

        spawn_decoder(sender, path, control.clone(), info.clone());

        (
            Self {
                handle,
                receiver,
                _info: info.clone(),
            },
            control,
            info,
        )
    }
}

impl Render for VideoPlayer {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        window.request_animation_frame();
        let mut latest = None;
        for frame in self.receiver.try_iter() {
            latest = Some(frame);
        }
        if let Some(frame) = latest {
            self.handle.submit(frame);
        }

        div()
            .flex()
            .flex_col()
            .size_full()
            .items_center()
            .justify_center()
            .bg(rgb(0x111111))
            .child(
                video(self.handle.clone())
                    .object_fit(ObjectFit::Contain)
                    .w_full()
                    .h_full(),
            )
    }
}

fn spawn_decoder(
    sender: SyncSender<Frame>,
    input_path: PathBuf,
    control: VideoPlayerControlHandle,
    info: VideoPlayerInfoHandle,
) {
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_time()
            .build()
            .expect("failed to create tokio runtime");

        runtime.block_on(async move {
            if !input_path.exists() {
                eprintln!("input video not found: {input_path:?}");
                info.update(|state| state.ended = true);
                return;
            }

            let available = Configuration::available_backends();
            if available.is_empty() {
                eprintln!(
                    "no decoder backend is compiled; enable a backend feature such as backend-ffmpeg"
                );
                info.update(|state| state.ended = true);
                return;
            }

            let backend = subtitle_fast_decoder::Backend::FFmpeg;
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
                    info.update(|state| state.ended = true);
                    return;
                }
            };

            let metadata = provider.metadata();
            info.update(|state| state.metadata = metadata);
            let frame_duration = metadata
                .fps
                .and_then(|fps| (fps > 0.0).then(|| Duration::from_secs_f64(1.0 / fps)));

            let (controller, mut stream) = match provider.open() {
                Ok(value) => value,
                Err(err) => {
                    eprintln!("failed to open decoder stream: {err}");
                    info.update(|state| state.ended = true);
                    return;
                }
            };
            control.set_decoder(controller);
            let mut started = false;
            let mut start_instant = Instant::now();
            let mut first_timestamp: Option<Duration> = None;
            let mut next_deadline = Instant::now();
            let mut paused_at: Option<Instant> = None;
            let mut last_seek_epoch = control.current_seek_epoch();

            loop {
                if control.is_paused() {
                    if paused_at.is_none() {
                        paused_at = Some(Instant::now());
                    }
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    continue;
                }

                if let Some(paused_at) = paused_at.take() {
                    let pause_duration = Instant::now().saturating_duration_since(paused_at);
                    start_instant += pause_duration;
                    next_deadline += pause_duration;
                }

                let frame = stream.next().await;
                match frame {
                    Some(Ok(frame)) => {
                        let current_seek_epoch = control.current_seek_epoch();
                        if current_seek_epoch != last_seek_epoch {
                            last_seek_epoch = current_seek_epoch;
                            started = false;
                            first_timestamp = None;
                            start_instant = Instant::now();
                            next_deadline = start_instant;
                            paused_at = None;
                        }
                        if !started {
                            start_instant = Instant::now();
                            next_deadline = start_instant;
                            started = true;
                        }

                        if let Some(timestamp) = frame.timestamp() {
                            let first = first_timestamp.get_or_insert(timestamp);
                            if let Some(delta) = timestamp.checked_sub(*first) {
                                let target = start_instant + delta;
                                let now = Instant::now();
                                if target > now {
                                    tokio::time::sleep(target - now).await;
                                }
                            }
                        } else if let Some(duration) = frame_duration {
                            let now = Instant::now();
                            if next_deadline > now {
                                tokio::time::sleep(next_deadline - now).await;
                            }
                            next_deadline += duration;
                        }

                        info.update(|state| {
                            state.last_timestamp = frame.timestamp();
                            state.last_frame_index = frame.frame_index();
                        });

                        if let Some(gpui_frame) = to_gpui_frame(&frame) {
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

            info.update(|state| state.ended = true);
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

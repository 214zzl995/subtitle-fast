use std::{
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, SyncSender, sync_channel},
    thread,
    time::{Duration, Instant},
};

use gpui::{
    App, Application, Bounds, Context, Frame, ObjectFit, Render, VideoHandle, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, rgb, size, video,
};
use subtitle_fast_decoder::{Configuration, OutputFormat, VideoFrame};
use tokio_stream::StreamExt;

const INPUT_VIDEO: &str = "examples/video/big-buck-bunny-480p-30sec.mp4";

struct VideoView {
    handle: VideoHandle,
    receiver: Receiver<Frame>,
}

impl Render for VideoView {
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
            .size_full()
            .items_center()
            .justify_center()
            .bg(rgb(0x111111))
            .child(
                video(self.handle.clone())
                    .object_fit(ObjectFit::Contain)
                    .w(px(854.0))
                    .h(px(480.0)),
            )
    }
}

fn main() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let input_path = manifest_dir.join(INPUT_VIDEO);

    let handle = VideoHandle::new();
    let (sender, receiver) = sync_channel(1);
    spawn_decoder(sender, input_path);

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(980.0), px(600.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| VideoView {
                    handle: handle.clone(),
                    receiver,
                })
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}

fn spawn_decoder(sender: SyncSender<Frame>, input_path: PathBuf) {
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_time()
            .build()
            .expect("failed to create tokio runtime");

        runtime.block_on(async move {
            if !input_path.exists() {
                eprintln!("input video not found: {input_path:?}");
                return;
            }

            let available = Configuration::available_backends();
            if available.is_empty() {
                eprintln!(
                    "no decoder backend is compiled; enable a backend feature such as backend-ffmpeg"
                );
                return;
            }

            let backend = available[0];
            let config = Configuration {
                backend,
                input: Some(input_path),
                channel_capacity: None,
                output_format: OutputFormat::Nv12,
            };

            let provider = match config.create_provider() {
                Ok(provider) => provider,
                Err(err) => {
                    eprintln!("failed to create decoder provider: {err}");
                    return;
                }
            };

            let metadata = provider.metadata();
            let frame_duration = metadata
                .fps
                .and_then(|fps| (fps > 0.0).then(|| Duration::from_secs_f64(1.0 / fps)));

            let mut stream = provider.into_stream();
            let mut started = false;
            let mut start_instant = Instant::now();
            let mut first_timestamp: Option<Duration> = None;
            let mut next_deadline = Instant::now();

            while let Some(frame) = stream.next().await {
                match frame {
                    Ok(frame) => {
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

                        if let Some(gpui_frame) = to_gpui_frame(&frame) {
                            if sender.send(gpui_frame).is_err() {
                                break;
                            }
                        }
                    }
                    Err(err) => {
                        eprintln!("decoder error: {err}");
                        break;
                    }
                }
            }
        });
    });
}

fn to_gpui_frame(frame: &VideoFrame) -> Option<Frame> {
    if frame.native().is_some() {
        eprintln!("native frame output is unsupported in this example; use NV12 output");
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

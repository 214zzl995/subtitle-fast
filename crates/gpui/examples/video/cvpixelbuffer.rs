#[cfg(target_os = "macos")]
mod macos {
    use std::{
        path::{Path, PathBuf},
        sync::mpsc::{Receiver, SyncSender, sync_channel},
        thread,
        time::{Duration, Instant},
    };

    use core_foundation::base::TCFType;
    use core_video::pixel_buffer::{CVPixelBuffer, CVPixelBufferRef};
    use gpui::{
        App, Application, Bounds, Context, ObjectFit, Render, VideoHandle, Window, WindowBounds,
        WindowOptions, div, prelude::*, px, rgb, size, video,
    };
    use subtitle_fast_decoder::{Backend, Configuration, OutputFormat, VideoFrame};
    use tokio_stream::StreamExt;

    const INPUT_VIDEO: &str = "examples/video/big-buck-bunny-480p-30sec.mp4";

    pub fn run() {
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

    struct VideoView {
        handle: VideoHandle,
        receiver: Receiver<VideoFrame>,
    }

    impl Render for VideoView {
        fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            window.request_animation_frame();
            let mut latest = None;
            for frame in self.receiver.try_iter() {
                latest = Some(frame);
            }
            if let Some(frame) = latest {
                if let Some(buffer) = to_cvpixelbuffer(&frame) {
                    self.handle.submit(buffer);
                }
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

    fn spawn_decoder(sender: SyncSender<VideoFrame>, input_path: PathBuf) {
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
                if !available.contains(&Backend::VideoToolbox) {
                    eprintln!("VideoToolbox backend is not available in this build");
                    return;
                }

                let config = Configuration {
                    backend: Backend::VideoToolbox,
                    input: Some(input_path),
                    channel_capacity: None,
                    output_format: OutputFormat::CVPixelBuffer,
                    start_frame: None,
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

                let (_controller, mut stream) = match provider.open() {
                    Ok(value) => value,
                    Err(err) => {
                        eprintln!("failed to open decoder stream: {err}");
                        return;
                    }
                };
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

                            if sender.send(frame).is_err() {
                                break;
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

    fn to_cvpixelbuffer(frame: &VideoFrame) -> Option<CVPixelBuffer> {
        let native = frame.native()?;
        if native.backend() != "videotoolbox" {
            eprintln!("unexpected native backend: {}", native.backend());
            return None;
        }
        let handle = native.handle();
        if handle.is_null() {
            eprintln!("native handle is null");
            return None;
        }

        let buffer_ref = handle as CVPixelBufferRef;
        let buffer = unsafe { CVPixelBuffer::wrap_under_get_rule(buffer_ref) };
        Some(buffer)
    }
}

#[cfg(target_os = "macos")]
fn main() {
    macos::run();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("This example is only supported on macOS.");
}

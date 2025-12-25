use std::{fs::File, io::Read, path::Path};

use gpui::{
    App, Application, Bounds, Context, Frame, ObjectFit, Render, VideoHandle, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, rgb, size, video,
};

const FRAME_WIDTH: u32 = 320;
const FRAME_HEIGHT: u32 = 192;

fn load_nv12_frame(path: &Path) -> anyhow::Result<Frame> {
    let y_stride = FRAME_WIDTH as usize;
    let uv_stride = FRAME_WIDTH as usize;
    let y_len = y_stride * FRAME_HEIGHT as usize;
    let uv_rows = (FRAME_HEIGHT as usize + 1) / 2;
    let uv_len = uv_stride * uv_rows;

    let mut file = File::open(path)?;
    let mut y_plane = vec![0u8; y_len];
    let mut uv_plane = vec![0u8; uv_len];

    file.read_exact(&mut y_plane)?;
    file.read_exact(&mut uv_plane)?;

    Frame::from_nv12_owned(
        FRAME_WIDTH,
        FRAME_HEIGHT,
        y_stride,
        uv_stride,
        y_plane,
        uv_plane,
    )
    .map_err(Into::into)
}

struct Nv12View {
    handle: VideoHandle,
}

impl Render for Nv12View {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .items_center()
            .justify_center()
            .bg(rgb(0x111111))
            .child(
                video(self.handle.clone())
                    .object_fit(ObjectFit::Contain)
                    .w(px(640.0))
                    .h(px(384.0)),
            )
    }
}

fn main() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let frame_path = manifest_dir.join("examples/nv12/bear_320x192_180.nv12.yuv");
    let frame = load_nv12_frame(&frame_path).expect("failed to load nv12 frame");

    let handle = VideoHandle::new();
    handle.submit(frame);

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(720.0), px(480.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| Nv12View {
                    handle: handle.clone(),
                })
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}

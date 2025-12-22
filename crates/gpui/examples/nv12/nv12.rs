use anyhow::anyhow;
use gpui::prelude::*;
use gpui::*;
use std::sync::Arc;

const YUV_PATH: &str = "examples/nv12/bear_320x192_180.nv12.yuv";
const FRAME_WIDTH: u32 = 320;
const FRAME_HEIGHT: u32 = 192;

struct Nv12Preview {
    image: Arc<RenderImage>,
}

impl Render for Nv12Preview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x0f1115))
            .flex()
            .items_center()
            .justify_center()
            .child(
                img(self.image.clone())
                    .object_fit(ObjectFit::Contain)
                    .w_full()
                    .h_full(),
            )
    }
}

fn load_nv12_demo() -> Result<Arc<RenderImage>> {
    let yuv_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(YUV_PATH);
    let nv12 = std::fs::read(&yuv_path)?;
    let y_size = (FRAME_WIDTH as usize) * (FRAME_HEIGHT as usize);
    let uv_width = ((FRAME_WIDTH + 1) / 2) as usize;
    let uv_height = ((FRAME_HEIGHT + 1) / 2) as usize;
    let uv_stride = uv_width * 2;
    let uv_size = uv_stride * uv_height;
    let expected = y_size + uv_size;
    if nv12.len() != expected {
        return Err(anyhow!(
            "unexpected NV12 length {} (expected {} for {}x{})",
            nv12.len(),
            expected,
            FRAME_WIDTH,
            FRAME_HEIGHT
        ));
    }
    let (y_plane, uv_plane) = nv12.split_at(y_size);
    let y_plane = y_plane.to_vec();
    let uv_plane = uv_plane.to_vec();

    let image = RenderImage::from_nv12(
        FRAME_WIDTH,
        FRAME_HEIGHT,
        FRAME_WIDTH as usize,
        uv_stride,
        y_plane,
        uv_plane,
    )?;
    Ok(Arc::new(image))
}

fn main() {
    let image = load_nv12_demo().expect("failed to load NV12 demo frame");
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(960.0), px(540.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("GPUI NV12 Preview".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |_, cx| {
                cx.new(|_| Nv12Preview {
                    image: image.clone(),
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}

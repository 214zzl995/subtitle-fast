use gpui::prelude::*;
use gpui::*;
use rust_embed::RustEmbed;
use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration;

use crate::gui::components::{
    CollapseDirection, DragRange, DraggableEdge, Sidebar, SidebarHandle, Titlebar, VideoControls,
    VideoPlayer, VideoRoiHandle, VideoRoiOverlay, VideoToolbar,
};
use crate::gui::icons::{Icon, icon_md};

#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*.svg"]
struct EmbeddedAssets;

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }

        if let Some(asset) = EmbeddedAssets::get(path) {
            return Ok(Some(asset.data));
        }

        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut entries: Vec<SharedString> = EmbeddedAssets::iter()
            .filter_map(|p| p.starts_with(path).then(|| p.into()))
            .collect();
        entries.extend(gpui_component_assets::Assets.list(path)?);
        entries.sort();
        entries.dedup();
        Ok(entries)
    }
}

pub struct SubtitleFastApp;

impl SubtitleFastApp {
    pub fn new(_cx: &mut App) -> Self {
        Self
    }

    pub fn open_window(&self, cx: &mut App) -> WindowHandle<MainWindow> {
        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);

        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("subtitle-fast".into()),
                        appears_transparent: true,
                        traffic_light_position: None,
                    }),
                    window_decorations: Some(WindowDecorations::Client),
                    ..Default::default()
                },
                move |_, cx| {
                    let titlebar = cx.new(|_| Titlebar::new("main-titlebar", "subtitle-fast"));
                    let (left_panel, left_panel_handle) = Sidebar::create(
                        DraggableEdge::Right,
                        DragRange::new(px(200.0), px(480.0)),
                        CollapseDirection::Left,
                        px(0.0),
                        Duration::from_millis(160),
                        cx,
                    );
                    let (right_panel, _) = Sidebar::create(
                        DraggableEdge::Left,
                        DragRange::new(px(200.0), px(480.0)),
                        CollapseDirection::Right,
                        px(0.0),
                        Duration::from_millis(160),
                        cx,
                    );
                    let toolbar_view = cx.new(|_| VideoToolbar::new());
                    let controls_view = cx.new(|_| VideoControls::new());
                    let (roi_overlay, roi_handle) = VideoRoiOverlay::new();
                    let roi_overlay_view = cx.new(|_| roi_overlay);
                    cx.new(|_| {
                        MainWindow::new(
                            None,
                            titlebar,
                            left_panel,
                            left_panel_handle,
                            right_panel,
                            toolbar_view,
                            controls_view,
                            roi_overlay_view,
                            roi_handle,
                        )
                    })
                },
            )
            .unwrap();

        window
    }
}

pub struct MainWindow {
    player: Option<Entity<VideoPlayer>>,
    titlebar: Entity<Titlebar>,
    left_panel: Entity<Sidebar>,
    _left_panel_handle: SidebarHandle,
    right_panel: Entity<Sidebar>,
    toolbar_view: Entity<VideoToolbar>,
    controls_view: Entity<VideoControls>,
    roi_overlay: Entity<VideoRoiOverlay>,
    _roi_handle: VideoRoiHandle,
}

impl MainWindow {
    fn new(
        player: Option<Entity<VideoPlayer>>,
        titlebar: Entity<Titlebar>,
        left_panel: Entity<Sidebar>,
        left_panel_handle: SidebarHandle,
        right_panel: Entity<Sidebar>,
        toolbar_view: Entity<VideoToolbar>,
        controls_view: Entity<VideoControls>,
        roi_overlay: Entity<VideoRoiOverlay>,
        roi_handle: VideoRoiHandle,
    ) -> Self {
        Self {
            player,
            titlebar,
            left_panel,
            _left_panel_handle: left_panel_handle,
            right_panel,
            toolbar_view,
            controls_view,
            roi_overlay,
            _roi_handle: roi_handle,
        }
    }

    fn prompt_for_video(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select video".into()),
        });

        cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                let selection = match receiver.await {
                    Ok(Ok(Some(mut paths))) => paths.pop(),
                    Ok(Ok(None)) => None,
                    Ok(Err(err)) => {
                        eprintln!("video selection failed: {err}");
                        None
                    }
                    Err(err) => {
                        eprintln!("video selection canceled: {err}");
                        None
                    }
                };

                if let Some(path) = selection {
                    let _ = this.update(&mut cx, |this, cx| {
                        this.load_video(path, cx);
                    });
                }
            }
        })
        .detach();
    }

    fn load_video(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let (player, controls, info) = VideoPlayer::new();
        controls.open(path);
        self.player = Some(cx.new(|_| player));
        let _ = self.controls_view.update(cx, |controls_view, cx| {
            controls_view.set_handles(Some(controls.clone()), Some(info.clone()));
            cx.notify();
        });
        let _ = self.toolbar_view.update(cx, |toolbar_view, cx| {
            toolbar_view.set_controls(Some(controls));
            cx.notify();
        });
        let _ = self.roi_overlay.update(cx, |overlay, cx| {
            overlay.set_info_handle(Some(info), cx);
        });
        cx.notify();
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let video_content = self.player.as_ref().map(|player| player.clone());

        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .child(self.titlebar.clone())
            .child({
                let video_wrapper = if let Some(video) = video_content {
                    let roi_overlay = self.roi_overlay.clone();
                    div()
                        .flex()
                        .flex_none()
                        .w_full()
                        .rounded(px(16.0))
                        .overflow_hidden()
                        .bg(rgb(0x111111))
                        .child(div().relative().size_full().child(video).child(roi_overlay))
                        .id(("video-wrapper", cx.entity_id()))
                } else {
                    div()
                        .flex()
                        .flex_none()
                        .w_full()
                        .rounded(px(16.0))
                        .overflow_hidden()
                        .bg(rgb(0x111111))
                        .cursor_pointer()
                        .items_center()
                        .justify_center()
                        .text_color(hsla(0.0, 0.0, 1.0, 0.7))
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap(px(6.0))
                                .child(icon_md(Icon::Upload, hsla(0.0, 0.0, 1.0, 0.7)))
                                .child("Click to select a video"),
                        )
                        .id(("video-wrapper", cx.entity_id()))
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.prompt_for_video(cx);
                        }))
                };
                let video_wrapper = video_wrapper.map(|mut view| {
                    view.style().aspect_ratio = Some(3.0 / 2.0);
                    view
                });

                let video_area = div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .bg(rgb(0x1b1b1b))
                    .px(px(8.0))
                    .py(px(2.0))
                    .gap(px(2.0))
                    .child(self.toolbar_view.clone())
                    .child(video_wrapper)
                    .child(self.controls_view.clone());

                div()
                    .flex()
                    .flex_row()
                    .flex_grow()
                    .w_full()
                    .child(self.left_panel.clone())
                    .child(video_area)
                    .child(self.right_panel.clone())
            })
    }
}

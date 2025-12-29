use gpui::prelude::*;
use gpui::*;
use gpui_component::button::Button;
use rust_embed::RustEmbed;
use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration;

use crate::gui::components::{
    CollapseDirection, DragRange, DraggableEdge, Sidebar, SidebarHandle, VideoPlayer,
    VideoPlayerControlHandle, VideoPlayerInfoHandle,
};

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
                    ..Default::default()
                },
                move |_, cx| {
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
                    cx.new(|_| {
                        MainWindow::new(
                            None,
                            None,
                            None,
                            left_panel,
                            left_panel_handle,
                            right_panel,
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
    controls: Option<VideoPlayerControlHandle>,
    _info: Option<VideoPlayerInfoHandle>,
    paused: bool,
    left_panel: Entity<Sidebar>,
    left_panel_handle: SidebarHandle,
    right_panel: Entity<Sidebar>,
}

impl MainWindow {
    pub fn new(
        player: Option<Entity<VideoPlayer>>,
        controls: Option<VideoPlayerControlHandle>,
        info: Option<VideoPlayerInfoHandle>,
        left_panel: Entity<Sidebar>,
        left_panel_handle: SidebarHandle,
        right_panel: Entity<Sidebar>,
    ) -> Self {
        Self {
            player,
            controls,
            _info: info,
            paused: false,
            left_panel,
            left_panel_handle,
            right_panel,
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
        let (player, controls, info) = VideoPlayer::new(path);
        self.player = Some(cx.new(|_| player));
        self.controls = Some(controls);
        self._info = Some(info);
        self.paused = false;
        cx.notify();
    }

    fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        let Some(controls) = self.controls.as_ref() else {
            return;
        };
        controls.toggle_pause();
        self.paused = !self.paused;
        cx.notify();
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toggle_label = if self.paused { "Play" } else { "Pause" };
        let video_content = self
            .player
            .as_ref()
            .map(|player| div().flex_grow().child(player.clone()));

        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .p(px(12.0))
                    .child(
                        Button::new("select-video")
                            .label("Select Video")
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.prompt_for_video(cx);
                            })),
                    )
                    .child(Button::new("toggle-playback").label(toggle_label).on_click(
                        cx.listener(|this, _event, _window, cx| {
                            this.toggle_playback(cx);
                        }),
                    ))
                    .child(
                        Button::new("toggle-left-sidebar")
                            .label("Toggle Sidebar")
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.left_panel_handle.toggle(cx);
                            })),
                    ),
            )
            .child({
                let video_area = div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .overflow_hidden();

                let video_area = if let Some(video) = video_content {
                    video_area.child(video)
                } else {
                    video_area
                };

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

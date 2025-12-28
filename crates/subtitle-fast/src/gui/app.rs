use gpui::prelude::*;
use gpui::*;
use gpui_component::button::Button;
use rust_embed::RustEmbed;
use std::borrow::Cow;
use std::path::PathBuf;

use crate::gui::components::{
    DragRange, DraggableEdge, DraggableEdgePanel, VideoPlayer, VideoPlayerControlHandle,
    VideoPlayerInfoHandle,
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
                    let left_panel = cx.new(|_| {
                        DraggableEdgePanel::new(
                            DraggableEdge::Right,
                            DragRange::new(px(200.0), px(480.0)),
                        )
                    });
                    cx.new(|_| MainWindow::new(None, None, None, left_panel))
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
    left_panel: Entity<DraggableEdgePanel>,
}

impl MainWindow {
    pub fn new(
        player: Option<Entity<VideoPlayer>>,
        controls: Option<VideoPlayerControlHandle>,
        info: Option<VideoPlayerInfoHandle>,
        left_panel: Entity<DraggableEdgePanel>,
    ) -> Self {
        Self {
            player,
            controls,
            _info: info,
            paused: false,
            left_panel,
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
                    )),
            )
            .child({
                let layout = div()
                    .flex()
                    .flex_row()
                    .flex_grow()
                    .w_full()
                    .child(self.left_panel.clone());
                if let Some(video) = video_content {
                    layout.child(video)
                } else {
                    layout
                }
            })
    }
}

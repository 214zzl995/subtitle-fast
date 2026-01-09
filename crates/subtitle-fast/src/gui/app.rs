use gpui::prelude::*;
use gpui::*;
use rust_embed::RustEmbed;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::gui::components::{
    CollapseDirection, ColorPicker, DetectedSubtitlesList, DetectionControls, DetectionHandle,
    DetectionMetrics, DetectionSidebar, DetectionSidebarHost, DragRange, DraggableEdge,
    FramePreprocessor, Nv12FrameInfo, Sidebar, SidebarHandle, TaskSidebar, TaskSidebarCallbacks,
    Titlebar, VideoControls, VideoLumaControls, VideoLumaHandle, VideoPlayer,
    VideoPlayerControlHandle, VideoPlayerInfoHandle, VideoRoiHandle, VideoRoiOverlay, VideoToolbar,
};
use crate::gui::icons::{Icon, icon_md, icon_sm};
use crate::gui::session::{SessionHandle, SessionId, VideoSession};

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

        Ok(None)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut entries: Vec<SharedString> = EmbeddedAssets::iter()
            .filter_map(|p| p.starts_with(path).then(|| p.into()))
            .collect();
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
                    window_min_size: Some(size(px(960.0), px(640.0))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("subtitle-fast".into()),
                        appears_transparent: true,
                        traffic_light_position: None,
                    }),
                    window_decorations: Some(WindowDecorations::Client),
                    ..Default::default()
                },
                move |_, cx| {
                    let sessions = SessionHandle::new();
                    let titlebar = cx.new(|_| Titlebar::new("main-titlebar", "subtitle-fast"));
                    let task_sidebar_view = cx.new(|_| {
                        TaskSidebar::new(
                            sessions.clone(),
                            TaskSidebarCallbacks {
                                on_add: Arc::new(|_, _| {}),
                                on_select: Arc::new(|_, _, _| {}),
                            },
                        )
                    });
                    let (left_panel, left_panel_handle) = Sidebar::create(
                        DraggableEdge::Right,
                        DragRange::new(px(200.0), px(480.0)),
                        CollapseDirection::Left,
                        px(0.0),
                        Duration::from_millis(160),
                        px(SIDEBAR_DRAG_HIT_THICKNESS),
                        {
                            let task_sidebar_view = task_sidebar_view.clone();
                            move || task_sidebar_content(task_sidebar_view.clone())
                        },
                        cx,
                    );
                    let detection_sidebar_host = cx.new(|_| DetectionSidebarHost::new());
                    let (right_panel, _) = Sidebar::create(
                        DraggableEdge::Left,
                        DragRange::new(px(240.0), px(520.0)),
                        CollapseDirection::Right,
                        px(0.0),
                        Duration::from_millis(160),
                        px(SIDEBAR_DRAG_HIT_THICKNESS),
                        {
                            let detection_sidebar_host = detection_sidebar_host.clone();
                            move || detection_sidebar_content(detection_sidebar_host.clone())
                        },
                        cx,
                    );
                    let (luma_controls, luma_handle) = VideoLumaControls::new();
                    let luma_controls_view = cx.new(|_| luma_controls);
                    let controls_view = cx.new(|_| VideoControls::new());
                    let (color_picker, color_picker_handle) = ColorPicker::new();
                    let color_picker_view = cx.new(|_| color_picker);
                    let toolbar_view = cx.new(|_| VideoToolbar::new());
                    let (roi_overlay, roi_handle) = VideoRoiOverlay::new();
                    let roi_overlay_view = cx.new(|_| roi_overlay);
                    let _ = toolbar_view.update(cx, |toolbar_view, cx| {
                        toolbar_view.set_luma_controls(
                            Some(luma_handle.clone()),
                            Some(luma_controls_view.clone()),
                            cx,
                        );
                        toolbar_view.set_roi_overlay(Some(roi_overlay_view.clone()), cx);
                        toolbar_view.set_roi_handle(Some(roi_handle.clone()));
                        toolbar_view.set_color_picker(
                            Some(color_picker_view.clone()),
                            Some(color_picker_handle.clone()),
                            cx,
                        );
                        cx.notify();
                    });
                    let _ = roi_overlay_view.update(cx, |overlay, cx| {
                        overlay.set_color_picker(
                            Some(color_picker_view.clone()),
                            Some(color_picker_handle.clone()),
                            cx,
                        );
                    });
                    let main_window = cx.new(|_| {
                        MainWindow::new(
                            None,
                            titlebar,
                            left_panel,
                            left_panel_handle,
                            right_panel,
                            sessions.clone(),
                            task_sidebar_view.clone(),
                            detection_sidebar_host.clone(),
                            luma_handle.clone(),
                            toolbar_view,
                            luma_controls_view,
                            controls_view,
                            roi_overlay_view,
                            roi_handle,
                        )
                    });
                    let weak_main = main_window.downgrade();
                    let add_handle = weak_main.clone();
                    let select_handle = weak_main.clone();
                    let _ = task_sidebar_view.update(cx, |sidebar, cx| {
                        sidebar.set_callbacks(
                            TaskSidebarCallbacks {
                                on_add: Arc::new(move |window, cx| {
                                    let Some(main_window) = add_handle.upgrade() else {
                                        return;
                                    };
                                    let _ = main_window.update(cx, |this, cx| {
                                        this.prompt_for_video(window, false, cx);
                                    });
                                }),
                                on_select: Arc::new(move |session_id, _window, cx| {
                                    let Some(main_window) = select_handle.upgrade() else {
                                        return;
                                    };
                                    let _ = main_window.update(cx, |this, cx| {
                                        this.activate_session(session_id, cx);
                                    });
                                }),
                            },
                            cx,
                        );
                    });
                    main_window
                },
            )
            .unwrap();

        window
    }
}

const SUPPORTED_VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "mkv", "webm", "avi", "m4v", "mpg", "mpeg", "ts",
];
const VIDEO_AREA_HEIGHT_RATIO: f32 = 0.6;
const REPLAY_PREPROCESSOR_KEY: &str = "replay-blur";
const SIDEBAR_DRAG_HIT_THICKNESS: f32 = 6.0;
const SIDEBAR_BORDER_WIDTH: f32 = 1.1;
const SIDEBAR_BORDER_COLOR: u32 = 0x2b2b2b;

fn task_sidebar_content(panel_view: Entity<TaskSidebar>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .size_full()
        .child(panel_view)
        .into_any_element()
}

fn detection_sidebar_content(panel_view: Entity<DetectionSidebarHost>) -> AnyElement {
    let border_width = px(SIDEBAR_BORDER_WIDTH);
    let border_color = rgb(SIDEBAR_BORDER_COLOR);
    div()
        .flex()
        .flex_col()
        .size_full()
        .bg(rgb(0x1a1a1a))
        .border_l(border_width)
        .border_color(border_color)
        .child(panel_view)
        .into_any_element()
}

pub struct MainWindow {
    player: Option<Entity<VideoPlayer>>,
    controls: Option<VideoPlayerControlHandle>,
    video_info: Option<VideoPlayerInfoHandle>,
    video_bounds: Option<Bounds<Pixels>>,
    replay_visible: bool,
    replay_dismissed: bool,
    titlebar: Entity<Titlebar>,
    left_panel: Entity<Sidebar>,
    _left_panel_handle: SidebarHandle,
    right_panel: Entity<Sidebar>,
    sessions: SessionHandle,
    active_session: Option<SessionId>,
    task_sidebar: Entity<TaskSidebar>,
    detection_sidebar_host: Entity<DetectionSidebarHost>,
    luma_handle: VideoLumaHandle,
    toolbar_view: Entity<VideoToolbar>,
    luma_controls_view: Entity<VideoLumaControls>,
    controls_view: Entity<VideoControls>,
    roi_overlay: Entity<VideoRoiOverlay>,
    roi_handle: VideoRoiHandle,
}

impl MainWindow {
    fn new(
        player: Option<Entity<VideoPlayer>>,
        titlebar: Entity<Titlebar>,
        left_panel: Entity<Sidebar>,
        left_panel_handle: SidebarHandle,
        right_panel: Entity<Sidebar>,
        sessions: SessionHandle,
        task_sidebar: Entity<TaskSidebar>,
        detection_sidebar_host: Entity<DetectionSidebarHost>,
        luma_handle: VideoLumaHandle,
        toolbar_view: Entity<VideoToolbar>,
        luma_controls_view: Entity<VideoLumaControls>,
        controls_view: Entity<VideoControls>,
        roi_overlay: Entity<VideoRoiOverlay>,
        roi_handle: VideoRoiHandle,
    ) -> Self {
        Self {
            player,
            controls: None,
            video_info: None,
            video_bounds: None,
            replay_visible: false,
            replay_dismissed: false,
            titlebar,
            left_panel,
            _left_panel_handle: left_panel_handle,
            right_panel,
            sessions,
            active_session: None,
            task_sidebar,
            detection_sidebar_host,
            luma_handle,
            toolbar_view,
            luma_controls_view,
            controls_view,
            roi_overlay,
            roi_handle,
        }
    }

    fn prompt_for_video(&mut self, window: &mut Window, select_new: bool, cx: &mut Context<Self>) {
        let options = PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select video".into()),
            allowed_extensions: Some(
                SUPPORTED_VIDEO_EXTENSIONS
                    .iter()
                    .map(|ext| SharedString::new_static(*ext))
                    .collect(),
            ),
        };
        let supported_detail = supported_video_extensions_detail();

        cx.spawn_in(
            window,
            move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
                let mut cx = cx.clone();
                async move {
                    loop {
                        let receiver =
                            match cx.update(|_, app| app.prompt_for_paths(options.clone())) {
                                Ok(receiver) => receiver,
                                Err(err) => {
                                    eprintln!("video selection failed: {err}");
                                    return;
                                }
                            };

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

                        let Some(path) = selection else {
                            return;
                        };

                        if is_supported_video_path(&path) {
                            let _ = this.update(&mut cx, move |this, cx| {
                                this.enqueue_session(path, select_new, cx);
                            });
                            return;
                        }

                        let answers = [PromptButton::ok("OK")];
                        let _ = cx
                            .prompt(
                                PromptLevel::Warning,
                                "Unsupported video format",
                                Some(&supported_detail),
                                &answers,
                            )
                            .await;
                    }
                }
            },
        )
        .detach();
    }

    fn enqueue_session(&mut self, path: PathBuf, select_new: bool, cx: &mut Context<Self>) {
        let detection_handle = DetectionHandle::new();
        detection_handle.set_video_path(Some(path.clone()));
        detection_handle.set_luma_handle(Some(self.luma_handle.clone()));
        detection_handle.set_roi_handle(Some(self.roi_handle.clone()));
        let session_id = self.sessions.add_session(path, detection_handle);

        if select_new || self.active_session.is_none() {
            self.activate_session(session_id, cx);
        } else {
            self.notify_task_sidebar(cx);
        }
    }

    fn activate_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        if self.active_session == Some(session_id) {
            return;
        }

        self.save_active_session_state(cx);
        self.active_session = Some(session_id);
        self.sessions.set_active(session_id);
        self.notify_task_sidebar(cx);
        self.release_player(cx);

        let Some(session) = self.sessions.session(session_id) else {
            self.update_detection_sidebar(None, cx);
            return;
        };
        self.load_session(&session, cx);
        self.update_detection_sidebar(Some(session.detection.clone()), cx);
    }

    fn save_active_session_state(&mut self, cx: &App) {
        let Some(session_id) = self.active_session else {
            return;
        };

        if let Some(info) = self.video_info.as_ref() {
            let snapshot = info.snapshot();
            self.sessions.update_playback(
                session_id,
                snapshot.last_timestamp,
                snapshot.last_frame_index,
            );
        }

        let luma_values = self.luma_handle.latest();

        let toolbar_state = self.toolbar_view.read(cx).snapshot();

        let roi = self.roi_handle.latest();

        self.sessions.update_settings(
            session_id,
            Some(luma_values.target),
            Some(luma_values.delta),
            Some(toolbar_state),
            Some(roi),
        );
    }

    fn load_session(&mut self, session: &VideoSession, cx: &mut Context<Self>) {
        let (player, controls, info) = VideoPlayer::new();
        let options = crate::gui::components::video_player::VideoOpenOptions {
            paused: true,
            start_frame: session.last_frame_index,
        };
        controls.open_with(session.path.clone(), options);
        self.player = Some(cx.new(|_| player));
        self.controls = Some(controls.clone());
        self.video_info = Some(info.clone());
        self.replay_dismissed = false;
        self.set_replay_visible(false, cx);
        let _ = self.controls_view.update(cx, |controls_view, cx| {
            controls_view.set_handles(Some(controls.clone()), Some(info.clone()));
            cx.notify();
        });
        let _ = self.luma_controls_view.update(cx, |luma_controls, cx| {
            luma_controls.set_enabled(true, cx);
            if let (Some(target), Some(delta)) = (session.luma_target, session.luma_delta) {
                luma_controls.set_values(target, delta, cx);
            }
        });
        let _ = self.toolbar_view.update(cx, |toolbar_view, cx| {
            toolbar_view.set_controls(Some(controls), cx);
            if let Some(state) = session.toolbar_state {
                toolbar_view.restore(state, cx);
            }
            cx.notify();
        });
        let _ = self.roi_overlay.update(cx, |overlay, cx| {
            overlay.set_info_handle(Some(info), cx);
            if let Some(roi) = session.roi {
                overlay.set_roi(roi, cx);
            }
        });
        cx.notify();
    }

    fn release_player(&mut self, cx: &mut Context<Self>) {
        self.player = None;
        self.controls = None;
        self.video_info = None;
        self.replay_dismissed = false;
        self.set_replay_visible(false, cx);
        let _ = self.controls_view.update(cx, |controls_view, cx| {
            controls_view.set_handles(None, None);
            cx.notify();
        });
        let _ = self.luma_controls_view.update(cx, |luma_controls, cx| {
            luma_controls.set_enabled(false, cx);
        });
        let _ = self.toolbar_view.update(cx, |toolbar_view, cx| {
            toolbar_view.set_controls(None, cx);
            cx.notify();
        });
        let _ = self.roi_overlay.update(cx, |overlay, cx| {
            overlay.set_info_handle(None, cx);
        });
        cx.notify();
    }

    fn build_detection_sidebar(
        &self,
        handle: DetectionHandle,
        cx: &mut Context<Self>,
    ) -> Entity<DetectionSidebar> {
        let detection_controls_view = cx.new(|_| DetectionControls::new(handle.clone()));
        let detection_metrics_view = cx.new(|_| DetectionMetrics::new(handle.clone()));
        let detection_subtitles_view = cx.new(|_| DetectedSubtitlesList::new(handle.clone()));
        cx.new(|_| {
            DetectionSidebar::new(
                handle,
                detection_metrics_view.clone(),
                detection_controls_view.clone(),
                detection_subtitles_view.clone(),
            )
        })
    }

    fn update_detection_sidebar(
        &mut self,
        handle: Option<DetectionHandle>,
        cx: &mut Context<Self>,
    ) {
        let sidebar = handle.map(|handle| self.build_detection_sidebar(handle, cx));
        let _ = self.detection_sidebar_host.update(cx, |host, cx| {
            host.set_sidebar(sidebar, cx);
        });
    }

    fn notify_task_sidebar(&mut self, cx: &mut Context<Self>) {
        let task_sidebar = self.task_sidebar.clone();
        cx.defer(move |cx| {
            let _ = cx.update_entity(&task_sidebar, |_, cx| {
                cx.notify();
            });
        });
    }

    fn set_replay_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.replay_visible == visible {
            return;
        }
        self.replay_visible = visible;
        if let Some(controls) = self.controls.as_ref() {
            if visible {
                controls.set_preprocessor(REPLAY_PREPROCESSOR_KEY, replay_blur_preprocessor());
            } else {
                controls.remove_preprocessor(REPLAY_PREPROCESSOR_KEY);
            }
        }
        cx.notify();
    }

    fn update_video_bounds(&mut self, bounds: Option<Bounds<Pixels>>) -> bool {
        if self.video_bounds != bounds {
            self.video_bounds = bounds;
            return true;
        }
        false
    }

    fn video_aspect(&self) -> Option<f32> {
        let info = self.video_info.as_ref()?;
        let snapshot = info.snapshot();
        let (width, height) = (snapshot.metadata.width?, snapshot.metadata.height?);
        if width == 0 || height == 0 {
            return None;
        }
        let aspect = width as f32 / height as f32;
        if !aspect.is_finite() || aspect <= 0.0 {
            return None;
        }
        Some(aspect)
    }

    fn video_frame_size(&self, total_height: f32) -> Option<(f32, f32)> {
        let bounds = self.video_bounds?;
        let container_w: f32 = bounds.size.width.into();
        if container_w <= 0.0 || total_height <= 0.0 {
            return None;
        }

        let width = container_w;
        let height = total_height * VIDEO_AREA_HEIGHT_RATIO;
        Some((width, height))
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let total_height: f32 = window.bounds().size.height.into();
        let video_content = self.player.as_ref().map(|player| player.clone());
        let video_aspect = self.video_aspect();
        let frame_size = self.video_frame_size(total_height);
        let ended = self
            .video_info
            .as_ref()
            .map(|info| {
                let snapshot = info.snapshot();
                snapshot.ended && snapshot.has_frame && !snapshot.scrubbing
            })
            .unwrap_or(false);
        if !ended {
            self.replay_dismissed = false;
        }
        self.set_replay_visible(ended && !self.replay_dismissed, cx);

        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .bg(rgb(0x1b1b1b))
            .child(self.titlebar.clone())
            .child({
                let mut video_frame = div()
                    .flex()
                    .rounded(px(16.0))
                    .overflow_hidden()
                    .bg(rgb(0x111111))
                    .items_center()
                    .justify_center()
                    .id(("video-frame", cx.entity_id()));
                if let Some((width, height)) = frame_size {
                    video_frame = video_frame.w(px(width)).h(px(height));
                } else {
                    video_frame = video_frame.w_full().h_full();
                }

                let frame_content = if let Some(video) = video_content {
                    let roi_overlay = self.roi_overlay.clone();
                    let replay_overlay = if self.replay_visible {
                        if let Some(controls) = self.controls.clone() {
                            let overlay_label = div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .text_xs()
                                .text_color(hsla(0.0, 0.0, 1.0, 0.85))
                                .child(icon_sm(Icon::RotateCcw, hsla(0.0, 0.0, 1.0, 0.85)))
                                .child("Replay");
                            Some(
                                div()
                                    .id(("replay-overlay", cx.entity_id()))
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .child(overlay_label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        controls.replay();
                                        this.replay_dismissed = true;
                                        this.set_replay_visible(false, cx);
                                    })),
                            )
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let mut video_wrapper = div()
                        .relative()
                        .child(
                            div()
                                .relative()
                                .size_full()
                                .child(video)
                                .child(roi_overlay)
                                .children(replay_overlay),
                        )
                        .id(("video-wrapper", cx.entity_id()));

                    if let Some(aspect) = video_aspect {
                        let fit_by_height = frame_size
                            .map(|(width, height)| (width / height) >= aspect)
                            .unwrap_or(aspect < 1.0);
                        video_wrapper = video_wrapper.map(|mut view| {
                            view.style().aspect_ratio = Some(aspect);
                            view
                        });
                        video_wrapper = if fit_by_height {
                            video_wrapper.h_full()
                        } else {
                            video_wrapper.w_full()
                        };
                    } else {
                        video_wrapper = video_wrapper.w_full().h_full();
                    }

                    video_wrapper
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .size_full()
                        .cursor_pointer()
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
                        .on_click(cx.listener(|this, _event, window, cx| {
                            this.prompt_for_video(window, true, cx);
                        }))
                };
                let video_wrapper = video_frame.child(frame_content);

                let handle = cx.entity();
                let video_slot = div()
                    .flex()
                    .flex_none()
                    .w_full()
                    .on_children_prepainted(move |bounds, _window, cx| {
                        let bounds = bounds.first().copied();
                        let _ = handle.update(cx, |this, cx| {
                            if this.update_video_bounds(bounds) {
                                cx.notify();
                            }
                        });
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w_full()
                            .child(video_wrapper),
                    );

                let toolbar_video_group = div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(self.toolbar_view.clone())
                    .child(video_slot);

                let video_area = div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .bg(rgb(0x1b1b1b))
                    .justify_start()
                    .px(px(8.0))
                    .pt(px(6.0))
                    .pb(px(2.0))
                    .gap(px(6.0))
                    .child(toolbar_video_group)
                    .child(self.controls_view.clone())
                    .child(self.luma_controls_view.clone());

                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .child(self.left_panel.clone())
                    .child(video_area)
                    .child(self.right_panel.clone())
            })
    }
}

fn is_supported_video_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    SUPPORTED_VIDEO_EXTENSIONS
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(ext))
}

fn supported_video_extensions_detail() -> String {
    let list = SUPPORTED_VIDEO_EXTENSIONS
        .iter()
        .map(|ext| format!(".{ext}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("Supported formats: {list}")
}

fn replay_blur_preprocessor() -> FramePreprocessor {
    Arc::new(|y_plane, uv_plane, info| {
        blur_nv12_plane(y_plane, uv_plane, info, 6);
        tint_nv12_plane(y_plane, uv_plane);
        true
    })
}

fn blur_nv12_plane(y_plane: &mut [u8], uv_plane: &mut [u8], info: Nv12FrameInfo, passes: usize) {
    for _ in 0..passes {
        blur_luma(y_plane, info);
        blur_chroma(uv_plane, info);
    }
}

fn blur_luma(y_plane: &mut [u8], info: Nv12FrameInfo) {
    let width = info.width as usize;
    let height = info.height as usize;
    let stride = info.y_stride;
    if width == 0 || height == 0 || stride == 0 {
        return;
    }
    if stride.saturating_mul(height) > y_plane.len() {
        return;
    }

    let mut out = y_plane.to_vec();
    for y in 0..height {
        let y0 = y.saturating_sub(1);
        let y2 = (y + 1).min(height - 1);
        for x in 0..width {
            let x0 = x.saturating_sub(1);
            let x2 = (x + 1).min(width - 1);
            let mut sum = 0u32;
            for yy in [y0, y, y2] {
                let row = yy * stride;
                for xx in [x0, x, x2] {
                    sum += y_plane[row + xx] as u32;
                }
            }
            out[y * stride + x] = (sum / 9) as u8;
        }
    }
    y_plane.copy_from_slice(&out);
}

fn blur_chroma(uv_plane: &mut [u8], info: Nv12FrameInfo) {
    let width = info.width as usize;
    let stride = info.uv_stride;
    if width == 0 || stride == 0 {
        return;
    }
    let uv_height = uv_plane.len() / stride;
    if uv_height == 0 {
        return;
    }
    let uv_width = ((width + 1) / 2).min(stride / 2);
    if uv_width == 0 {
        return;
    }

    let mut out = uv_plane.to_vec();
    for y in 0..uv_height {
        let y0 = y.saturating_sub(1);
        let y2 = (y + 1).min(uv_height - 1);
        for x in 0..uv_width {
            let x0 = x.saturating_sub(1);
            let x2 = (x + 1).min(uv_width - 1);
            let mut sum_u = 0u32;
            let mut sum_v = 0u32;
            for yy in [y0, y, y2] {
                let row = yy * stride;
                for xx in [x0, x, x2] {
                    let idx = row + xx * 2;
                    sum_u += uv_plane[idx] as u32;
                    sum_v += uv_plane[idx + 1] as u32;
                }
            }
            let out_idx = y * stride + x * 2;
            out[out_idx] = (sum_u / 9) as u8;
            out[out_idx + 1] = (sum_v / 9) as u8;
        }
    }
    uv_plane.copy_from_slice(&out);
}

fn tint_nv12_plane(y_plane: &mut [u8], uv_plane: &mut [u8]) {
    for y in y_plane.iter_mut() {
        *y = ((*y as u16 * 165) / 255) as u8;
    }

    for pair in uv_plane.chunks_exact_mut(2) {
        let u = pair[0] as i16;
        let v = pair[1] as i16;
        let u_shift = (u - 128) * 80 / 100;
        let v_shift = (v - 128) * 80 / 100;
        let tinted_u = 128 + u_shift + 6;
        let tinted_v = 128 + v_shift - 6;
        pair[0] = tinted_u.clamp(0, 255) as u8;
        pair[1] = tinted_v.clamp(0, 255) as u8;
    }
}
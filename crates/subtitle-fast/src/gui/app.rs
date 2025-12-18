use gpui::prelude::*;
use gpui::*;
use std::sync::Arc;

use crate::gui::components::*;
use crate::gui::state::AppState;
use crate::gui::theme::AppTheme;

pub struct SubtitleFastApp {
    state: Arc<AppState>,
    theme: AppTheme,
}

impl SubtitleFastApp {
    pub fn new(_cx: &mut App) -> Self {
        let state = AppState::new();
        let theme = AppTheme::auto();

        Self { state, theme }
    }

    pub fn open_window(&self, cx: &mut App) -> WindowHandle<MainWindow> {
        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Subtitle Fast".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                window_min_size: Some(size(px(1150.0), px(720.0))),
                ..Default::default()
            },
            |_, cx| cx.new(|_| MainWindow::new(Arc::clone(&self.state), self.theme)),
        )
        .unwrap()
    }
}

pub struct MainWindow {
    state: Arc<AppState>,
    theme: AppTheme,
}

impl MainWindow {
    pub fn new(state: Arc<AppState>, theme: AppTheme) -> Self {
        Self { state, theme }
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = cx.new(|_| Sidebar::new(Arc::clone(&self.state), self.theme));
        let preview = cx.new(|_| PreviewPanel::new(Arc::clone(&self.state), self.theme));
        let control_panel = cx.new(|_| ControlPanel::new(Arc::clone(&self.state), self.theme));
        let status_panel = cx.new(|_| StatusPanel::new(Arc::clone(&self.state), self.theme));
        let subtitle_list = cx.new(|_| SubtitleList::new(Arc::clone(&self.state), self.theme));

        div()
            .flex()
            .w_full()
            .h_full()
            .bg(self.theme.background())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .h_full()
                    .gap(px(10.0))
                    .child(self.render_header())
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .gap(px(12.0))
                            .px(px(14.0))
                            .pb(px(14.0))
                            .child(div().w(px(260.0)).min_w(px(240.0)).h_full().child(sidebar))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .gap(px(12.0))
                                    .child(div().flex_1().child(preview))
                                    .child(div().child(control_panel)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .w(px(320.0))
                                    .min_w(px(280.0))
                                    .h_full()
                                    .gap(px(12.0))
                                    .child(div().child(status_panel))
                                    .child(div().flex_1().child(subtitle_list)),
                            ),
                    ),
            )
    }
}

impl MainWindow {
    fn render_header(&self) -> Div {
        div().h(px(56.0)).px(px(16.0)).pt(px(8.0)).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .w_full()
                .h_full()
                .bg(self.theme.surface())
                .border_1()
                .border_color(self.theme.border())
                .rounded_md()
                .px(px(12.0))
                .py(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(
                            div()
                                .w(px(32.0))
                                .h(px(32.0))
                                .rounded_full()
                                .bg(self.theme.accent_muted())
                                .border_1()
                                .border_color(self.theme.border())
                                .items_center()
                                .justify_center()
                                .flex()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(self.theme.text_primary())
                                        .child("SF"),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(self.theme.text_primary())
                                        .child("subtitle-fast"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(self.theme.text_secondary())
                                        .child("字幕检测 · 预览面板"),
                                ),
                        ),
                )
                .child(div().flex().items_center().gap(px(8.0)).children(
                    ["预览", "检测", "导出"].iter().map(|label| {
                        div()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded_md()
                            .bg(self.theme.surface_elevated())
                            .border_1()
                            .border_color(self.theme.border())
                            .text_xs()
                            .text_color(self.theme.text_secondary())
                            .child(*label)
                    }),
                )),
        )
    }
}

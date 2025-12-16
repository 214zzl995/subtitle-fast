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
        div()
            .flex()
            .w_full()
            .h_full()
            .bg(self.theme.background())
            .child(
                div()
                    .flex()
                    .w_full()
                    .h_full()
                    .child(
                        div()
                            .w(relative(0.2))
                            .min_w(px(240.0))
                            .max_w(px(340.0))
                            .h_full()
                            .child(cx.new(|_| Sidebar::new(Arc::clone(&self.state), self.theme))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .h_full()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .h_full()
                                    .p_3()
                                    .gap_3()
                                    .child(div().flex_1().child(cx.new(|_| {
                                        PreviewPanel::new(Arc::clone(&self.state), self.theme)
                                    })))
                                    .child(div().h(px(200.0)).child(cx.new(|_| {
                                        ControlPanel::new(Arc::clone(&self.state), self.theme)
                                    }))),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .w(px(280.0))
                                    .min_w(px(240.0))
                                    .h_full()
                                    .p_3()
                                    .gap_3()
                                    .child(div().h(px(400.0)).child(cx.new(|_| {
                                        StatusPanel::new(Arc::clone(&self.state), self.theme)
                                    })))
                                    .child(div().flex_1().child(cx.new(|_| {
                                        SubtitleList::new(Arc::clone(&self.state), self.theme)
                                    }))),
                            ),
                    ),
            )
    }
}

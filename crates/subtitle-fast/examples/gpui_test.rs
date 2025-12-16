// Minimal GPUI 0.2 test to verify API
use gpui::prelude::*;
use gpui::*;

struct TestApp {
    message: SharedString,
}

impl Render for TestApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .size_full()
            .bg(rgb(0x2d2d2d))
            .child(
                div()
                    .text_xl()
                    .text_color(rgb(0xffffff))
                    .child(format!("Test: {}", self.message)),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(400.0), px(300.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("GPUI Test".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| TestApp {
                    message: "Hello GPUI 0.2!".into(),
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}

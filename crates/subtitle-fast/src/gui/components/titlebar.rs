use std::mem;

use gpui::prelude::*;
use gpui::*;

#[cfg(target_os = "windows")]
const WINDOWS_TITLEBAR_HEIGHT: f32 = 32.0;
#[cfg(not(target_os = "windows"))]
const TITLEBAR_MIN_HEIGHT: f32 = 34.0;
const MAC_TRAFFIC_LIGHT_PADDING: f32 = 72.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlatformStyle {
    Mac,
    Windows,
    Linux,
}

impl PlatformStyle {
    fn platform() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

pub struct Titlebar {
    id: ElementId,
    title: SharedString,
    platform_style: PlatformStyle,
    children: Vec<AnyElement>,
    should_move: bool,
}

impl Titlebar {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            platform_style: PlatformStyle::platform(),
            children: Vec::new(),
            should_move: false,
        }
    }

    pub fn set_children<T>(&mut self, children: T)
    where
        T: IntoIterator<Item = AnyElement>,
    {
        self.children = children.into_iter().collect();
    }

    #[cfg(target_os = "windows")]
    fn height(_window: &Window) -> Pixels {
        px(WINDOWS_TITLEBAR_HEIGHT)
    }

    #[cfg(not(target_os = "windows"))]
    fn height(window: &Window) -> Pixels {
        (1.75 * window.rem_size()).max(px(TITLEBAR_MIN_HEIGHT))
    }

    fn titlebar_color(&self, window: &Window) -> Hsla {
        if window.is_window_active() {
            rgb(0x101010).into()
        } else {
            rgb(0x1a1a1a).into()
        }
    }

    fn titlebar_text_color(&self, window: &Window) -> Hsla {
        if window.is_window_active() {
            rgb(0xe6e6e6).into()
        } else {
            rgb(0x9a9a9a).into()
        }
    }

    fn control_button(
        id: &'static str,
        label: &'static str,
        area: WindowControlArea,
        hover: Hsla,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .w(px(40.0))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .window_control_area(area)
            .hover(move |style| style.bg(hover))
            .child(label)
            .on_click(move |_, window, cx| on_click(window, cx))
    }
}

impl Render for Titlebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let supported_controls = window.window_controls();
        let titlebar_color = self.titlebar_color(window);
        let text_color = self.titlebar_text_color(window);
        let height = Self::height(window);
        let children = mem::take(&mut self.children);

        let drag_region = div()
            .flex()
            .items_center()
            .h_full()
            .flex_1()
            .px(px(12.0))
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down_out(cx.listener(|this, _ev, _window, _| {
                this.should_move = false;
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _ev, _window, _| {
                    this.should_move = false;
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, _| {
                    if event.click_count == 2 {
                        match this.platform_style {
                            PlatformStyle::Mac => window.titlebar_double_click(),
                            PlatformStyle::Linux => window.zoom_window(),
                            PlatformStyle::Windows => {}
                        }
                    } else {
                        this.should_move = true;
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, _ev, window, _| {
                if this.should_move {
                    this.should_move = false;
                    window.start_window_move();
                }
            }));

        let drag_region = if self.platform_style == PlatformStyle::Mac {
            drag_region.pl(px(MAC_TRAFFIC_LIGHT_PADDING))
        } else {
            drag_region
        };

        let drag_region = drag_region.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(self.title.clone())
                .children(children),
        );

        let mut controls = div().flex().items_center().h_full().gap(px(2.0));
        if self.platform_style != PlatformStyle::Mac && supported_controls.minimize {
            controls = controls.child(Self::control_button(
                "titlebar-minimize",
                "-",
                WindowControlArea::Min,
                hsla(0.0, 0.0, 1.0, 0.08),
                |window, _| window.minimize_window(),
            ));
        }

        if self.platform_style != PlatformStyle::Mac && supported_controls.maximize {
            controls = controls.child(Self::control_button(
                "titlebar-maximize",
                "[]",
                WindowControlArea::Max,
                hsla(0.0, 0.0, 1.0, 0.08),
                |window, _| window.zoom_window(),
            ));
        }

        if self.platform_style != PlatformStyle::Mac {
            controls = controls.child(Self::control_button(
                "titlebar-close",
                "X",
                WindowControlArea::Close,
                hsla(0.0, 0.8, 0.55, 0.35),
                |window, _| window.remove_window(),
            ));
        }

        div()
            .id(self.id.clone())
            .flex()
            .items_center()
            .w_full()
            .h(height)
            .bg(titlebar_color)
            .text_color(text_color)
            .child(drag_region)
            .child(controls)
    }
}

impl ParentElement for Titlebar {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

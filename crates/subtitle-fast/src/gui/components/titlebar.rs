use std::mem;

use gpui::prelude::*;
use gpui::*;

#[cfg(target_os = "windows")]
const WINDOWS_TITLEBAR_HEIGHT: f32 = 32.0;
const WINDOWS_CAPTION_BUTTON_WIDTH: f32 = 36.0;
const WINDOWS_CAPTION_FONT: &str = "Segoe MDL2 Assets";
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowsCaptionButtonIcon {
    Minimize,
    Restore,
    Maximize,
    Close,
}

#[derive(IntoElement)]
struct WindowsCaptionButton {
    id: ElementId,
    icon: WindowsCaptionButtonIcon,
    hover_background: Hsla,
    active_background: Hsla,
}

impl WindowsCaptionButton {
    fn new(
        id: impl Into<ElementId>,
        icon: WindowsCaptionButtonIcon,
        hover_background: Hsla,
        active_background: Hsla,
    ) -> Self {
        Self {
            id: id.into(),
            icon,
            hover_background,
            active_background,
        }
    }
}

impl RenderOnce for WindowsCaptionButton {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .id(self.id)
            .flex()
            .items_center()
            .justify_center()
            .occlude()
            .w(px(WINDOWS_CAPTION_BUTTON_WIDTH))
            .h_full()
            .text_size(px(10.0))
            .cursor_pointer()
            .hover(|style| style.bg(self.hover_background))
            .active(|style| style.bg(self.active_background))
            .map(|this| match self.icon {
                WindowsCaptionButtonIcon::Close => {
                    this.window_control_area(WindowControlArea::Close)
                }
                WindowsCaptionButtonIcon::Maximize | WindowsCaptionButtonIcon::Restore => {
                    this.window_control_area(WindowControlArea::Max)
                }
                WindowsCaptionButtonIcon::Minimize => {
                    this.window_control_area(WindowControlArea::Min)
                }
            })
            .child(match self.icon {
                WindowsCaptionButtonIcon::Minimize => "\u{e921}",
                WindowsCaptionButtonIcon::Restore => "\u{e923}",
                WindowsCaptionButtonIcon::Maximize => "\u{e922}",
                WindowsCaptionButtonIcon::Close => "\u{e8bb}",
            })
    }
}

#[derive(IntoElement)]
struct WindowsWindowControls {
    id: ElementId,
    button_height: Pixels,
    supported_controls: WindowControls,
}

impl WindowsWindowControls {
    fn new(
        id: impl Into<ElementId>,
        button_height: Pixels,
        supported_controls: WindowControls,
    ) -> Self {
        Self {
            id: id.into(),
            button_height,
            supported_controls,
        }
    }
}

impl RenderOnce for WindowsWindowControls {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let hover_background = hsla(0.0, 0.0, 1.0, 0.08);
        let active_background = hsla(0.0, 0.0, 1.0, 0.14);
        let close_hover_background = hsla(0.0, 0.8, 0.55, 0.35);

        let mut controls = div()
            .id(self.id)
            .font_family(WINDOWS_CAPTION_FONT)
            .flex()
            .items_center()
            .justify_center()
            .max_h(self.button_height)
            .min_h(self.button_height);

        if self.supported_controls.minimize {
            controls = controls.child(WindowsCaptionButton::new(
                "titlebar-minimize",
                WindowsCaptionButtonIcon::Minimize,
                hover_background,
                active_background,
            ));
        }

        if self.supported_controls.maximize {
            let icon = if window.is_maximized() {
                WindowsCaptionButtonIcon::Restore
            } else {
                WindowsCaptionButtonIcon::Maximize
            };
            controls = controls.child(WindowsCaptionButton::new(
                "titlebar-maximize",
                icon,
                hover_background,
                active_background,
            ));
        }

        controls.child(WindowsCaptionButton::new(
            "titlebar-close",
            WindowsCaptionButtonIcon::Close,
            close_hover_background,
            active_background,
        ))
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
                .child(
                    div()
                        .text_size(px(12.0))
                        .child(self.title.clone()),
                )
                .children(children),
        );

        let mut controls = div().flex().items_center().h_full().gap(px(2.0));
        if self.platform_style == PlatformStyle::Windows {
            controls = controls.child(WindowsWindowControls::new(
                "windows-titlebar-controls",
                height,
                supported_controls,
            ));
        } else if self.platform_style != PlatformStyle::Mac {
            if supported_controls.minimize {
                controls = controls.child(Self::control_button(
                    "titlebar-minimize",
                    "-",
                    WindowControlArea::Min,
                    hsla(0.0, 0.0, 1.0, 0.08),
                    |window, _| window.minimize_window(),
                ));
            }

            if supported_controls.maximize {
                controls = controls.child(Self::control_button(
                    "titlebar-maximize",
                    "[]",
                    WindowControlArea::Max,
                    hsla(0.0, 0.0, 1.0, 0.08),
                    |window, _| window.zoom_window(),
                ));
            }

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

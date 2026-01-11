use gpui::prelude::*;
use gpui::*;

#[derive(Clone, Copy)]
pub enum Icon {
    Activity,
    Check,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    Crosshair,
    Eye,
    EyeOff,
    Film,
    Frame,
    GalleryThumbnails,
    Gauge,
    Merge,
    MessageSquare,
    MousePointer,
    Pause,
    PanelLeftClose,
    PanelLeftOpen,
    Play,
    PlaySquare,
    Scan,
    ScanText,
    RotateCcw,
    Sparkles,
    Stop,
    Sun,
    Trash,
    Upload,
}

impl Icon {
    fn path(self) -> SharedString {
        match self {
            Self::Activity => "icons/activity.svg",
            Self::Check => "icons/check.svg",
            Self::ChevronDown => "icons/chevron-down.svg",
            Self::ChevronLeft => "icons/chevron-left.svg",
            Self::ChevronRight => "icons/chevron-right.svg",
            Self::Crosshair => "icons/crosshair.svg",
            Self::Eye => "icons/eye.svg",
            Self::EyeOff => "icons/eye-off.svg",
            Self::Film => "icons/film.svg",
            Self::Frame => "icons/frame.svg",
            Self::GalleryThumbnails => "icons/gallery-thumbnails.svg",
            Self::Gauge => "icons/gauge.svg",
            Self::Merge => "icons/merge.svg",
            Self::MessageSquare => "icons/message-square.svg",
            Self::MousePointer => "icons/mouse-pointer-2.svg",
            Self::Pause => "icons/pause.svg",
            Self::PanelLeftClose => "icons/panel-left-close.svg",
            Self::PanelLeftOpen => "icons/panel-left-open.svg",
            Self::Play => "icons/play.svg",
            Self::PlaySquare => "icons/square-play.svg",
            Self::Scan => "icons/scan.svg",
            Self::ScanText => "icons/scan-text.svg",
            Self::RotateCcw => "icons/rotate-ccw.svg",
            Self::Sparkles => "icons/sparkles.svg",
            Self::Stop => "icons/square.svg",
            Self::Sun => "icons/sun.svg",
            Self::Trash => "icons/trash.svg",
            Self::Upload => "icons/upload.svg",
        }
        .into()
    }
}

fn icon_base(name: Icon, color: Hsla) -> Svg {
    svg().path(name.path()).text_color(color)
}

pub fn icon(name: Icon, color: Hsla) -> Svg {
    icon_base(name, color)
}

pub fn icon_sm(name: Icon, color: Hsla) -> Svg {
    icon_base(name, color).w(px(16.0)).h(px(16.0))
}

pub fn icon_md(name: Icon, color: Hsla) -> Svg {
    icon_base(name, color).w(px(20.0)).h(px(20.0))
}

pub fn icon_lg(name: Icon, color: Hsla) -> Svg {
    icon_base(name, color).w(px(24.0)).h(px(24.0))
}

pub fn icon_button(name: Icon, color: Hsla, hover_bg: Hsla) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .child(icon_sm(name, color))
}

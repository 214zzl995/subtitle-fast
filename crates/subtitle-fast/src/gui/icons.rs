use gpui::prelude::*;
use gpui::*;

use gpui_component::Icon as IconComponent;
pub use gpui_component::IconName as Icon;

pub fn icon(name: Icon, color: Hsla) -> IconComponent {
    IconComponent::new(name).text_color(color)
}

pub fn icon_sm(name: Icon, color: Hsla) -> IconComponent {
    IconComponent::new(name)
        .w(px(16.0))
        .h(px(16.0))
        .text_color(color)
}

pub fn icon_md(name: Icon, color: Hsla) -> IconComponent {
    IconComponent::new(name)
        .w(px(20.0))
        .h(px(20.0))
        .text_color(color)
}

pub fn icon_lg(name: Icon, color: Hsla) -> IconComponent {
    IconComponent::new(name)
        .w(px(24.0))
        .h(px(24.0))
        .text_color(color)
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

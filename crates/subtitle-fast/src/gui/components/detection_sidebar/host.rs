use gpui::prelude::*;
use gpui::{Context, Entity, Render, Window, div, hsla, px};

use super::DetectionSidebar;

pub struct DetectionSidebarHost {
    active: Option<Entity<DetectionSidebar>>,
}

impl DetectionSidebarHost {
    pub fn new() -> Self {
        Self { active: None }
    }

    pub fn set_sidebar(
        &mut self,
        sidebar: Option<Entity<DetectionSidebar>>,
        cx: &mut Context<Self>,
    ) {
        self.active = sidebar;
        cx.notify();
    }
}

impl Render for DetectionSidebarHost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(active) = self.active.clone() {
            return active.into_any_element();
        }

        let placeholder_color = hsla(0.0, 0.0, 1.0, 0.4);
        div()
            .id(("detection-sidebar-placeholder", cx.entity_id()))
            .flex()
            .items_center()
            .justify_center()
            .size_full()
            .text_size(px(12.0))
            .text_color(placeholder_color)
            .child("Select a video to view detection details")
            .into_any_element()
    }
}

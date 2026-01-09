use gpui::prelude::*;
use gpui::{Context, Entity, Render, Window};

use super::{
    DetectedSubtitlesList, DetectionControls, DetectionHandle, DetectionMetrics, DetectionSidebar,
};

pub struct DetectionSidebarHost {
    active: Entity<DetectionSidebar>,
}

impl DetectionSidebarHost {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let empty_handle = DetectionHandle::new();
        let sidebar = Self::build_sidebar(empty_handle, cx);
        Self { active: sidebar }
    }

    fn build_sidebar(handle: DetectionHandle, cx: &mut Context<Self>) -> Entity<DetectionSidebar> {
        let detection_controls_view = cx.new(|_| DetectionControls::new(handle.clone()));
        let detection_metrics_view = cx.new(|_| DetectionMetrics::new(handle.clone()));
        let detection_subtitles_view = cx.new(|_| DetectedSubtitlesList::new(handle.clone()));
        cx.new(|_| {
            DetectionSidebar::new(
                handle,
                detection_metrics_view,
                detection_controls_view,
                detection_subtitles_view,
            )
        })
    }

    pub fn set_sidebar(
        &mut self,
        sidebar: Option<Entity<DetectionSidebar>>,
        cx: &mut Context<Self>,
    ) {
        if let Some(sidebar) = sidebar {
            self.active = sidebar;
        } else {
            let empty_handle = DetectionHandle::new();
            self.active = Self::build_sidebar(empty_handle, cx);
        }
        cx.notify();
    }
}

impl Render for DetectionSidebarHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.active.clone()
    }
}

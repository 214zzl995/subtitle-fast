use gpui::prelude::*;
use gpui::{Context, Entity, FontWeight, Render, Window, div, hsla, px, relative};

use crate::gui::icons::{Icon, icon_sm};

use super::{DetectionControls, DetectionMetrics};

pub struct DetectionSidebar {
    metrics_view: Entity<DetectionMetrics>,
    controls_view: Entity<DetectionControls>,
}

impl DetectionSidebar {
    pub fn new(
        metrics_view: Entity<DetectionMetrics>,
        controls_view: Entity<DetectionControls>,
    ) -> Self {
        Self {
            metrics_view,
            controls_view,
        }
    }

    fn section_title(
        &self,
        id: &'static str,
        label: &'static str,
        icon: Icon,
        title_color: gpui::Hsla,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        div()
            .id((id, cx.entity_id()))
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_size(px(12.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(title_color)
            .child(icon_sm(icon, title_color).w(px(14.0)).h(px(14.0)))
            .child(label)
    }
}

impl Render for DetectionSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title_color = hsla(0.0, 0.0, 1.0, 0.72);
        let padding_x = px(12.0);
        let padding_y = px(16.0);

        let upper = div()
            .id(("detection-sidebar-upper", cx.entity_id()))
            .flex()
            .flex_col()
            .flex_none()
            .h(relative(1.0 / 3.0))
            .min_h(px(0.0))
            .gap(px(12.0))
            .child(self.section_title(
                "detection-sidebar-progress-title",
                "Detection",
                Icon::ScanText,
                title_color,
                cx,
            ))
            .child(self.metrics_view.clone())
            .child(self.controls_view.clone());

        let lower = div()
            .id(("detection-sidebar-lower", cx.entity_id()))
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(12.0))
            .child(self.section_title(
                "detection-sidebar-subtitles-title",
                "Detected Subtitles",
                Icon::MessageSquare,
                title_color,
                cx,
            ))
            .child(div().flex_1().min_h(px(0.0)));

        div()
            .id(("detection-sidebar-panel", cx.entity_id()))
            .flex()
            .flex_col()
            .size_full()
            .pt(padding_y)
            .pb(padding_y)
            .pl(padding_x)
            .pr(padding_x)
            .child(upper)
            .child(lower)
    }
}

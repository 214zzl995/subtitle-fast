use gpui::prelude::*;
use gpui::{Context, Entity, FontWeight, Render, Window, div, hsla, px, relative};

use crate::gui::icons::{Icon, icon_sm};

use super::{DetectedSubtitlesList, DetectionControls, DetectionMetrics};

pub struct DetectionSidebar {
    metrics_view: Entity<DetectionMetrics>,
    controls_view: Entity<DetectionControls>,
    subtitles_view: Entity<DetectedSubtitlesList>,
}

impl DetectionSidebar {
    pub fn new(
        metrics_view: Entity<DetectionMetrics>,
        controls_view: Entity<DetectionControls>,
        subtitles_view: Entity<DetectedSubtitlesList>,
    ) -> Self {
        Self {
            metrics_view,
            controls_view,
            subtitles_view,
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

    fn subtitles_header(&self, cx: &Context<Self>) -> impl IntoElement {
        let title_color = hsla(0.0, 0.0, 1.0, 0.72);
        let export_color = hsla(0.0, 0.0, 1.0, 0.9);
        let export_hover = hsla(0.0, 0.0, 1.0, 0.08);
        let export_border = hsla(0.0, 0.0, 1.0, 0.12);

        let label = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_size(px(12.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(title_color)
            .child(
                icon_sm(Icon::MessageSquare, title_color)
                    .w(px(14.0))
                    .h(px(14.0)),
            )
            .child("Detected Subtitles");

        let export_button = div()
            .id(("detection-sidebar-export", cx.entity_id()))
            .flex()
            .items_center()
            .justify_center()
            .w(px(26.0))
            .h(px(26.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(export_border)
            .cursor_pointer()
            .hover(move |s| s.bg(export_hover))
            .child(icon_sm(Icon::Upload, export_color).w(px(14.0)).h(px(14.0)))
            .on_click(cx.listener(|_this, _event, _window, _cx| {
                eprintln!("export requested for detection subtitles");
            }));

        div()
            .id(("detection-sidebar-subtitles-header", cx.entity_id()))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(label)
            .child(export_button)
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
            .h(relative(0.4))
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
            .child(self.subtitles_header(cx))
            .child(self.subtitles_view.clone());

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

use gpui::prelude::*;
use gpui::{Context, Entity, FontWeight, Render, Window, div, hsla, px, rgb};

use crate::gui::icons::{Icon, icon_sm};

use super::{DetectedSubtitlesList, DetectionControls, DetectionHandle, DetectionMetrics};

pub struct DetectionSidebar {
    handle: DetectionHandle,
    metrics_view: Entity<DetectionMetrics>,
    controls_view: Entity<DetectionControls>,
    subtitles_view: Entity<DetectedSubtitlesList>,
}

impl DetectionSidebar {
    pub fn new(
        handle: DetectionHandle,
        metrics_view: Entity<DetectionMetrics>,
        controls_view: Entity<DetectionControls>,
        subtitles_view: Entity<DetectedSubtitlesList>,
    ) -> Self {
        Self {
            handle,
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
        let export_enabled = self.handle.has_subtitles();
        let export_color = if export_enabled {
            hsla(0.0, 0.0, 1.0, 0.9)
        } else {
            hsla(0.0, 0.0, 1.0, 0.35)
        };
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

        let mut export_button = div()
            .id(("detection-sidebar-export", cx.entity_id()))
            .flex()
            .items_center()
            .justify_center()
            .w(px(26.0))
            .h(px(26.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(export_border)
            .child(icon_sm(Icon::Upload, export_color).w(px(14.0)).h(px(14.0)));

        if export_enabled {
            export_button = export_button
                .cursor_pointer()
                .hover(move |s| s.bg(export_hover))
                .on_click(cx.listener(|this, _event, window, cx| {
                    this.request_export(window, cx);
                }));
        }

        div()
            .id(("detection-sidebar-subtitles-header", cx.entity_id()))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(label)
            .child(export_button)
    }

    fn request_export(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.handle.has_subtitles() {
            eprintln!("export ignored: no subtitles detected");
            return;
        }

        let (directory, suggested_name) = self.handle.export_dialog_seed();
        let receiver = cx.prompt_for_new_path(&directory, suggested_name.as_deref());
        let handle = self.handle.clone();

        let _ = window.spawn(cx, async move |_| match receiver.await {
            Ok(Ok(Some(path))) => {
                handle.export_subtitles_to(path);
            }
            Ok(Ok(None)) => {}
            Ok(Err(err)) => {
                eprintln!("export dialog failed: {err}");
            }
            Err(err) => {
                eprintln!("export dialog failed: {err}");
            }
        });
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
            .gap(px(12.0))
            .px(padding_x)
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
            .px(padding_x)
            .child(self.subtitles_header(cx))
            .child(self.subtitles_view.clone());

        let divider = div().h(px(1.0)).w_full().bg(rgb(0x2b2b2b)).mt(px(12.0)).mb(px(16.0));

        div()
            .id(("detection-sidebar-panel", cx.entity_id()))
            .flex()
            .flex_col()
            .size_full()
            .pt(padding_y)
            .pb(padding_y)
            .child(upper)
            .child(divider)
            .child(lower)
    }
}

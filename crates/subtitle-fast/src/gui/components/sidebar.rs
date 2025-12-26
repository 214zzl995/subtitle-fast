use crate::gui::icons::{Icon, icon_sm};
use crate::gui::state::AppState;
use crate::gui::theme::AppTheme;
use gpui::prelude::*;
use gpui::*;

pub struct Sidebar {
    state: Entity<AppState>,
    theme: AppTheme,
    state_subscription: Option<Subscription>,
}

impl Sidebar {
    pub fn new(state: Entity<AppState>) -> Self {
        Self {
            state,
            theme: AppTheme::dark(),
            state_subscription: None,
        }
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_state_subscription(cx);
        self.theme = self.state.read(cx).get_theme();

        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .bg(self.theme.surface())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(12.0))
                    .pt(px(14.0))
                    .pb(px(10.0))
                    .child(icon_sm(Icon::Film, self.theme.text_secondary()))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(self.theme.text_primary())
                            .child("Navigation"),
                    ),
            )
            .child(div().flex_1().w_full().h_full())
    }
}

impl Sidebar {
    fn ensure_state_subscription(&mut self, cx: &mut Context<Self>) {
        if self.state_subscription.is_some() {
            return;
        }

        let state = self.state.clone();
        self.state_subscription = Some(cx.observe(&state, |_, _, cx| {
            cx.notify();
        }));
    }
}

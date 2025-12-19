use gpui::prelude::*;
use gpui::*;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CollapseDirection {
    #[default]
    Left,
    Right,
}

#[derive(Clone, Copy, Debug)]
pub struct AnimatedPanelConfig {
    pub expanded_width: f32,
    pub collapsed_width: f32,
    pub duration: Duration,
    pub direction: CollapseDirection,
}

impl Default for AnimatedPanelConfig {
    fn default() -> Self {
        Self {
            expanded_width: 240.0,
            collapsed_width: 0.0,
            duration: Duration::from_millis(200),
            direction: CollapseDirection::Left,
        }
    }
}

impl AnimatedPanelConfig {
    pub fn new(expanded_width: f32) -> Self {
        Self {
            expanded_width,
            ..Default::default()
        }
    }

    pub fn with_collapsed_width(mut self, width: f32) -> Self {
        self.collapsed_width = width;
        self
    }

    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

    pub fn with_direction(mut self, direction: CollapseDirection) -> Self {
        self.direction = direction;
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AnimatedPanelState {
    is_collapsed: bool,
    toggle_count: u32,
}

impl Default for AnimatedPanelState {
    fn default() -> Self {
        Self {
            is_collapsed: false,
            toggle_count: 0,
        }
    }
}

impl AnimatedPanelState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn collapsed() -> Self {
        Self {
            is_collapsed: true,
            toggle_count: 0,
        }
    }

    pub fn is_collapsed(&self) -> bool {
        self.is_collapsed
    }

    pub fn is_expanded(&self) -> bool {
        !self.is_collapsed
    }

    pub fn toggle(&mut self) {
        self.is_collapsed = !self.is_collapsed;
        self.toggle_count = self.toggle_count.wrapping_add(1);
    }

    pub fn open(&mut self) {
        if self.is_collapsed {
            self.is_collapsed = false;
            self.toggle_count = self.toggle_count.wrapping_add(1);
        }
    }

    pub fn close(&mut self) {
        if !self.is_collapsed {
            self.is_collapsed = true;
            self.toggle_count = self.toggle_count.wrapping_add(1);
        }
    }

    pub fn set_collapsed(&mut self, collapsed: bool) {
        if self.is_collapsed != collapsed {
            self.is_collapsed = collapsed;
            self.toggle_count = self.toggle_count.wrapping_add(1);
        }
    }

    pub fn animation_id(&self, prefix: &str) -> SharedString {
        format!("{}-{}-{}", prefix, self.is_collapsed, self.toggle_count).into()
    }
}

pub fn animated_panel_container(
    state: AnimatedPanelState,
    config: AnimatedPanelConfig,
    animation_id_prefix: &str,
    child: impl IntoElement,
) -> AnimationElement<Div> {
    let (from, to) = if state.is_collapsed() {
        (config.expanded_width, config.collapsed_width)
    } else {
        (config.collapsed_width, config.expanded_width)
    };

    let animation = Animation::new(config.duration).with_easing(ease_out_quint());
    let animation_id = state.animation_id(animation_id_prefix);

    div()
        .overflow_hidden()
        .h_full()
        .child(child)
        .with_animation(animation_id, animation, move |this, t| {
            let w = from + (to - from) * t;
            this.w(px(w))
        })
}

pub trait AnimatedPanelExt {
    type Output;

    fn with_animated_width(
        self,
        state: AnimatedPanelState,
        config: AnimatedPanelConfig,
        animation_id_prefix: &str,
    ) -> Self::Output;
}

impl AnimatedPanelExt for Div {
    type Output = AnimationElement<Div>;

    fn with_animated_width(
        self,
        state: AnimatedPanelState,
        config: AnimatedPanelConfig,
        animation_id_prefix: &str,
    ) -> Self::Output {
        let (from, to) = if state.is_collapsed() {
            (config.expanded_width, config.collapsed_width)
        } else {
            (config.collapsed_width, config.expanded_width)
        };

        let animation = Animation::new(config.duration).with_easing(ease_out_quint());
        let animation_id = state.animation_id(animation_id_prefix);

        self.overflow_hidden()
            .with_animation(animation_id, animation, move |this, t| {
                let w = from + (to - from) * t;
                this.w(px(w))
            })
    }
}

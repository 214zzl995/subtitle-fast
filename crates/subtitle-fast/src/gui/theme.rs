use gpui::*;

#[derive(Clone, Copy, Debug)]
pub struct AppTheme {
    pub is_dark: bool,
}

impl AppTheme {
    pub fn light() -> Self {
        Self { is_dark: false }
    }

    pub fn dark() -> Self {
        Self { is_dark: true }
    }

    pub fn from_window_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
        }
    }

    pub fn background(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0 / 360.0, 0.15, 0.07, 1.0)
        } else {
            hsla(220.0 / 360.0, 0.02, 0.97, 1.0)
        }
    }

    pub fn surface(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0 / 360.0, 0.12, 0.10, 1.0)
        } else {
            hsla(220.0 / 360.0, 0.03, 0.93, 1.0)
        }
    }

    pub fn surface_elevated(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0 / 360.0, 0.10, 0.13, 1.0)
        } else {
            hsla(220.0 / 360.0, 0.04, 0.9, 1.0)
        }
    }

    pub fn surface_hover(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0 / 360.0, 0.12, 0.16, 1.0)
        } else {
            hsla(220.0 / 360.0, 0.04, 0.88, 1.0)
        }
    }

    pub fn surface_active(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0 / 360.0, 0.12, 0.18, 1.0)
        } else {
            hsla(220.0 / 360.0, 0.04, 0.85, 1.0)
        }
    }

    pub fn translucent_panel(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0 / 360.0, 0.12, 0.05, 0.9)
        } else {
            hsla(220.0 / 360.0, 0.02, 0.98, 0.8)
        }
    }

    pub fn text_primary(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0 / 360.0, 0.02, 0.92, 1.0)
        } else {
            hsla(220.0 / 360.0, 0.02, 0.12, 1.0)
        }
    }

    pub fn text_secondary(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0 / 360.0, 0.03, 0.60, 1.0)
        } else {
            hsla(220.0 / 360.0, 0.03, 0.35, 1.0)
        }
    }

    pub fn text_tertiary(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0 / 360.0, 0.03, 0.45, 1.0)
        } else {
            hsla(220.0 / 360.0, 0.03, 0.55, 1.0)
        }
    }

    pub fn border(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0 / 360.0, 0.10, 0.18, 1.0)
        } else {
            hsla(220.0 / 360.0, 0.08, 0.78, 1.0)
        }
    }

    pub fn border_focused(&self) -> Hsla {
        if self.is_dark {
            hsla(210.0 / 360.0, 0.75, 0.50, 0.8)
        } else {
            hsla(210.0 / 360.0, 0.75, 0.45, 0.8)
        }
    }

    pub fn accent(&self) -> Hsla {
        if self.is_dark {
            hsla(210.0 / 360.0, 0.85, 0.56, 1.0)
        } else {
            hsla(210.0 / 360.0, 0.85, 0.46, 1.0)
        }
    }

    pub fn accent_muted(&self) -> Hsla {
        if self.is_dark {
            hsla(210.0 / 360.0, 0.75, 0.50, 0.15)
        } else {
            hsla(210.0 / 360.0, 0.75, 0.46, 0.18)
        }
    }

    pub fn accent_hover(&self) -> Hsla {
        if self.is_dark {
            hsla(210.0 / 360.0, 0.85, 0.62, 1.0)
        } else {
            hsla(210.0 / 360.0, 0.85, 0.40, 1.0)
        }
    }

    pub fn success(&self) -> Hsla {
        if self.is_dark {
            hsla(140.0 / 360.0, 0.60, 0.48, 1.0)
        } else {
            hsla(140.0 / 360.0, 0.60, 0.35, 1.0)
        }
    }

    pub fn warning(&self) -> Hsla {
        if self.is_dark {
            hsla(40.0 / 360.0, 0.90, 0.50, 1.0)
        } else {
            hsla(40.0 / 360.0, 0.90, 0.40, 1.0)
        }
    }

    pub fn error(&self) -> Hsla {
        if self.is_dark {
            hsla(0.0 / 360.0, 0.72, 0.52, 1.0)
        } else {
            hsla(0.0 / 360.0, 0.72, 0.45, 1.0)
        }
    }

    pub fn danger(&self) -> Hsla {
        if self.is_dark {
            hsla(0.0 / 360.0, 0.72, 0.52, 1.0)
        } else {
            hsla(0.0 / 360.0, 0.72, 0.45, 1.0)
        }
    }

    pub fn danger_hover(&self) -> Hsla {
        if self.is_dark {
            hsla(0.0 / 360.0, 0.72, 0.58, 1.0)
        } else {
            hsla(0.0 / 360.0, 0.72, 0.40, 1.0)
        }
    }

    pub fn overlay(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0 / 360.0, 0.10, 0.02, 0.65)
        } else {
            hsla(220.0 / 360.0, 0.10, 0.9, 0.35)
        }
    }

    pub fn overlay_dashed(&self) -> Hsla {
        if self.is_dark {
            hsla(210.0 / 360.0, 0.75, 0.55, 0.65)
        } else {
            hsla(210.0 / 360.0, 0.75, 0.46, 0.65)
        }
    }

    pub fn titlebar_bg(&self) -> Hsla {
        self.background()
    }

    pub fn titlebar_border(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0 / 360.0, 0.10, 0.15, 1.0)
        } else {
            hsla(220.0 / 360.0, 0.08, 0.85, 1.0)
        }
    }
}

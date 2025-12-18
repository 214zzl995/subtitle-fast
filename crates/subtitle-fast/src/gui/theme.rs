use gpui::*;

/// Color scheme: Black, white, and greys with high contrast
/// Avoiding neutral/middle greys and blue/purple tones
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

    pub fn auto() -> Self {
        // TODO: Detect system theme
        Self::dark()
    }

    // Background colors
    pub fn background(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0, 0.1, 0.08, 1.0)
        } else {
            hsla(220.0, 0.02, 0.97, 1.0)
        }
    }

    pub fn surface(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0, 0.08, 0.11, 1.0)
        } else {
            hsla(220.0, 0.03, 0.93, 1.0)
        }
    }

    pub fn surface_elevated(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0, 0.08, 0.15, 1.0)
        } else {
            hsla(220.0, 0.04, 0.9, 1.0)
        }
    }

    pub fn translucent_panel(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0, 0.12, 0.07, 0.8)
        } else {
            hsla(220.0, 0.02, 0.98, 0.8)
        }
    }

    // Text colors
    pub fn text_primary(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0, 0.02, 0.92, 1.0)
        } else {
            hsla(220.0, 0.02, 0.12, 1.0)
        }
    }

    pub fn text_secondary(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0, 0.03, 0.65, 1.0)
        } else {
            hsla(220.0, 0.03, 0.35, 1.0)
        }
    }

    pub fn text_tertiary(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0, 0.03, 0.48, 1.0)
        } else {
            hsla(220.0, 0.03, 0.55, 1.0)
        }
    }

    // Border colors
    pub fn border(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0, 0.08, 0.22, 1.0)
        } else {
            hsla(220.0, 0.08, 0.78, 1.0)
        }
    }

    pub fn border_focused(&self) -> Hsla {
        if self.is_dark {
            hsla(216.0, 0.8, 0.64, 0.8)
        } else {
            hsla(216.0, 0.8, 0.4, 0.8)
        }
    }

    // Accent color
    pub fn accent(&self) -> Hsla {
        if self.is_dark {
            hsla(216.0, 0.78, 0.64, 1.0)
        } else {
            hsla(216.0, 0.78, 0.46, 1.0)
        }
    }

    pub fn accent_muted(&self) -> Hsla {
        if self.is_dark {
            hsla(216.0, 0.78, 0.64, 0.16)
        } else {
            hsla(216.0, 0.78, 0.46, 0.18)
        }
    }

    pub fn accent_hover(&self) -> Hsla {
        if self.is_dark {
            hsla(216.0, 0.86, 0.7, 1.0)
        } else {
            hsla(216.0, 0.86, 0.36, 1.0)
        }
    }

    // Status colors
    pub fn success(&self) -> Hsla {
        if self.is_dark {
            hsla(152.0, 0.5, 0.55, 1.0)
        } else {
            hsla(152.0, 0.5, 0.35, 1.0)
        }
    }

    pub fn warning(&self) -> Hsla {
        if self.is_dark {
            hsla(42.0, 0.9, 0.56, 1.0)
        } else {
            hsla(42.0, 0.9, 0.4, 1.0)
        }
    }

    pub fn error(&self) -> Hsla {
        if self.is_dark {
            hsla(2.0, 0.84, 0.62, 1.0)
        } else {
            hsla(2.0, 0.84, 0.45, 1.0)
        }
    }

    // Overlay colors
    pub fn overlay(&self) -> Hsla {
        if self.is_dark {
            hsla(220.0, 0.1, 0.02, 0.65)
        } else {
            hsla(220.0, 0.1, 0.9, 0.35)
        }
    }

    pub fn overlay_dashed(&self) -> Hsla {
        if self.is_dark {
            hsla(216.0, 0.78, 0.64, 0.65)
        } else {
            hsla(216.0, 0.78, 0.46, 0.65)
        }
    }
}

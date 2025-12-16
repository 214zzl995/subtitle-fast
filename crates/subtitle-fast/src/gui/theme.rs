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
            hsla(0.0, 0.0, 0.05, 1.0) // Very dark grey, almost black
        } else {
            hsla(0.0, 0.0, 0.98, 1.0) // Very light grey, almost white
        }
    }

    pub fn surface(&self) -> Hsla {
        if self.is_dark {
            hsla(0.0, 0.0, 0.12, 1.0) // Dark grey
        } else {
            hsla(0.0, 0.0, 1.0, 1.0) // Pure white
        }
    }

    pub fn surface_elevated(&self) -> Hsla {
        if self.is_dark {
            hsla(0.0, 0.0, 0.18, 1.0) // Slightly lighter dark grey
        } else {
            hsla(0.0, 0.0, 0.95, 1.0) // Light grey
        }
    }

    // Text colors
    pub fn text_primary(&self) -> Hsla {
        if self.is_dark {
            hsla(0.0, 0.0, 0.95, 1.0) // Very light grey
        } else {
            hsla(0.0, 0.0, 0.1, 1.0) // Very dark grey
        }
    }

    pub fn text_secondary(&self) -> Hsla {
        if self.is_dark {
            hsla(0.0, 0.0, 0.65, 1.0) // Light grey (high contrast)
        } else {
            hsla(0.0, 0.0, 0.4, 1.0) // Dark grey (high contrast)
        }
    }

    pub fn text_tertiary(&self) -> Hsla {
        if self.is_dark {
            hsla(0.0, 0.0, 0.45, 1.0) // Medium-light grey
        } else {
            hsla(0.0, 0.0, 0.55, 1.0) // Medium-dark grey
        }
    }

    // Border colors
    pub fn border(&self) -> Hsla {
        if self.is_dark {
            hsla(0.0, 0.0, 0.25, 1.0) // Dark grey border
        } else {
            hsla(0.0, 0.0, 0.85, 1.0) // Light grey border
        }
    }

    pub fn border_focused(&self) -> Hsla {
        if self.is_dark {
            hsla(0.0, 0.0, 0.4, 1.0) // Lighter grey for focus
        } else {
            hsla(0.0, 0.0, 0.6, 1.0) // Darker grey for focus
        }
    }

    // Accent color (avoiding blue/purple, using grey-based)
    pub fn accent(&self) -> Hsla {
        if self.is_dark {
            hsla(0.0, 0.0, 0.8, 1.0) // Light grey accent
        } else {
            hsla(0.0, 0.0, 0.2, 1.0) // Dark grey accent
        }
    }

    pub fn accent_hover(&self) -> Hsla {
        if self.is_dark {
            hsla(0.0, 0.0, 0.9, 1.0) // Very light grey
        } else {
            hsla(0.0, 0.0, 0.1, 1.0) // Very dark grey
        }
    }

    // Status colors (minimal, grey-based)
    pub fn success(&self) -> Hsla {
        if self.is_dark {
            hsla(0.0, 0.0, 0.75, 1.0)
        } else {
            hsla(0.0, 0.0, 0.25, 1.0)
        }
    }

    pub fn warning(&self) -> Hsla {
        if self.is_dark {
            hsla(0.0, 0.0, 0.7, 1.0)
        } else {
            hsla(0.0, 0.0, 0.3, 1.0)
        }
    }

    pub fn error(&self) -> Hsla {
        if self.is_dark {
            hsla(0.0, 0.0, 0.65, 1.0)
        } else {
            hsla(0.0, 0.0, 0.35, 1.0)
        }
    }

    // Overlay colors
    pub fn overlay(&self) -> Hsla {
        if self.is_dark {
            hsla(0.0, 0.0, 0.0, 0.6)
        } else {
            hsla(0.0, 0.0, 0.0, 0.3)
        }
    }
}

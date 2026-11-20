use serde::{Deserialize, Serialize};

use crate::model::Status;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Auto,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeColor {
    pub hex: &'static str,
    pub rgb: (u8, u8, u8),
}

impl ThemeColor {
    pub const fn new(hex: &'static str, r: u8, g: u8, b: u8) -> Self {
        Self {
            hex,
            rgb: (r, g, b),
        }
    }

    pub const fn r(self) -> u8 {
        self.rgb.0
    }

    pub const fn g(self) -> u8 {
        self.rgb.1
    }

    pub const fn b(self) -> u8 {
        self.rgb.2
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub mode: ThemeMode,
    pub bg: ThemeColor,
    pub surface: ThemeColor,
    pub surface_alt: ThemeColor,
    pub border: ThemeColor,
    pub fg: ThemeColor,
    pub muted: ThemeColor,
    pub accent: ThemeColor,
    pub selection: ThemeColor,
    pub highlight: ThemeColor,
    pub ok: ThemeColor,
    pub warn: ThemeColor,
    pub error: ThemeColor,
    pub active: ThemeColor,
    pub idle: ThemeColor,
    pub status_muted: ThemeColor,
    pub chart_palette: [ThemeColor; 8],
}

impl Theme {
    pub const fn dark() -> Self {
        Self {
            mode: ThemeMode::Dark,
            bg: ThemeColor::new("#0F1117", 15, 17, 23),
            surface: ThemeColor::new("#181C25", 24, 28, 37),
            surface_alt: ThemeColor::new("#222736", 34, 39, 54),
            border: ThemeColor::new("#343B4F", 52, 59, 79),
            fg: ThemeColor::new("#E6EDF3", 230, 237, 243),
            muted: ThemeColor::new("#94A3B8", 148, 163, 184),
            accent: ThemeColor::new("#60A5FA", 96, 165, 250),
            selection: ThemeColor::new("#1E3A5F", 30, 58, 95),
            highlight: ThemeColor::new("#F59E0B", 245, 158, 11),
            ok: ThemeColor::new("#4ADE80", 74, 222, 128),
            warn: ThemeColor::new("#FBBF24", 251, 191, 36),
            error: ThemeColor::new("#F87171", 248, 113, 113),
            active: ThemeColor::new("#38BDF8", 56, 189, 248),
            idle: ThemeColor::new("#A78BFA", 167, 139, 250),
            status_muted: ThemeColor::new("#94A3B8", 148, 163, 184),
            chart_palette: [
                ThemeColor::new("#60A5FA", 96, 165, 250),
                ThemeColor::new("#34D399", 52, 211, 153),
                ThemeColor::new("#FBBF24", 251, 191, 36),
                ThemeColor::new("#F87171", 248, 113, 113),
                ThemeColor::new("#A78BFA", 167, 139, 250),
                ThemeColor::new("#F472B6", 244, 114, 182),
                ThemeColor::new("#2DD4BF", 45, 212, 191),
                ThemeColor::new("#FB923C", 251, 146, 60),
            ],
        }
    }

    pub const fn light() -> Self {
        Self {
            mode: ThemeMode::Light,
            bg: ThemeColor::new("#FFFFFF", 255, 255, 255),
            surface: ThemeColor::new("#F8FAFC", 248, 250, 252),
            surface_alt: ThemeColor::new("#F1F5F9", 241, 245, 249),
            border: ThemeColor::new("#CBD5E1", 203, 213, 225),
            fg: ThemeColor::new("#0F172A", 15, 23, 42),
            muted: ThemeColor::new("#475569", 71, 85, 105),
            accent: ThemeColor::new("#1D4ED8", 29, 78, 216),
            selection: ThemeColor::new("#DBEAFE", 219, 234, 254),
            highlight: ThemeColor::new("#B45309", 180, 83, 9),
            ok: ThemeColor::new("#15803D", 21, 128, 61),
            warn: ThemeColor::new("#B45309", 180, 83, 9),
            error: ThemeColor::new("#B91C1C", 185, 28, 28),
            active: ThemeColor::new("#0369A1", 3, 105, 161),
            idle: ThemeColor::new("#6D28D9", 109, 40, 217),
            status_muted: ThemeColor::new("#475569", 71, 85, 105),
            chart_palette: [
                ThemeColor::new("#1D4ED8", 29, 78, 216),
                ThemeColor::new("#047857", 4, 120, 87),
                ThemeColor::new("#B45309", 180, 83, 9),
                ThemeColor::new("#B91C1C", 185, 28, 28),
                ThemeColor::new("#6D28D9", 109, 40, 217),
                ThemeColor::new("#BE185D", 190, 24, 93),
                ThemeColor::new("#0F766E", 15, 118, 110),
                ThemeColor::new("#C2410C", 194, 65, 12),
            ],
        }
    }

    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Light => Self::light(),
            // The core has no terminal to query; callers resolve Auto first.
            ThemeMode::Dark | ThemeMode::Auto => Self::dark(),
        }
    }

    pub const fn status_color(&self, status: Status) -> ThemeColor {
        match status {
            Status::Ok => self.ok,
            Status::Warn => self.warn,
            Status::Error => self.error,
            Status::Active => self.active,
            Status::Idle => self.idle,
            Status::Muted => self.status_muted,
        }
    }

    pub const fn syntect_theme_name(&self) -> &'static str {
        match self.mode {
            ThemeMode::Light => "InspiredGitHub",
            ThemeMode::Dark | ThemeMode::Auto => "base16-ocean.dark",
        }
    }

    pub fn resolve_svg_vars(&self, svg: &str) -> String {
        let replacements = [
            ("var(--bg)", self.bg.hex),
            ("var(--surface)", self.surface.hex),
            ("var(--surface-alt)", self.surface_alt.hex),
            ("var(--fg)", self.fg.hex),
            ("var(--muted)", self.muted.hex),
            ("var(--border)", self.border.hex),
            ("var(--accent)", self.accent.hex),
            ("var(--selection)", self.selection.hex),
            ("var(--highlight)", self.highlight.hex),
            ("var(--ok)", self.ok.hex),
            ("var(--warn)", self.warn.hex),
            ("var(--error)", self.error.hex),
            ("var(--active)", self.active.hex),
            ("var(--idle)", self.idle.hex),
        ];

        let mut out = svg.to_string();
        for (needle, value) in replacements {
            if out.contains(needle) {
                out = out.replace(needle, value);
            }
        }
        out
    }
}

pub fn parse_hex_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.trim().strip_prefix('#').unwrap_or(hex.trim());
    if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some((r, g, b))
    } else if hex.len() == 3 {
        let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
        let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
        let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
        Some((r * 17, g * 17, b * 17))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relative_luminance(rgb: (u8, u8, u8)) -> f64 {
        let to_linear = |c: u8| {
            let s = f64::from(c) / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * to_linear(rgb.0) + 0.7152 * to_linear(rgb.1) + 0.0722 * to_linear(rgb.2)
    }

    fn contrast_ratio(a: ThemeColor, b: ThemeColor) -> f64 {
        let la = relative_luminance(a.rgb);
        let lb = relative_luminance(b.rgb);
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn palettes_meet_wcag_aa_contrast() {
        for theme in [Theme::dark(), Theme::light()] {
            assert!(contrast_ratio(theme.fg, theme.bg) >= 7.0);
            assert!(contrast_ratio(theme.fg, theme.surface) >= 7.0);
            assert!(contrast_ratio(theme.muted, theme.bg) >= 4.5);
            assert!(contrast_ratio(theme.muted, theme.surface) >= 4.5);
            assert!(contrast_ratio(theme.accent, theme.bg) >= 4.5);
        }
    }

    #[test]
    fn syntect_themes_match_mode() {
        assert_eq!(Theme::light().syntect_theme_name(), "InspiredGitHub");
        assert_eq!(Theme::dark().syntect_theme_name(), "base16-ocean.dark");
    }

    #[test]
    fn resolve_svg_vars_replaces_all_tokens() {
        let svg = r#"<svg fill="var(--bg)" stroke="var(--border)"><rect fill="var(--surface)"/><text fill="var(--fg)"/><circle fill="var(--accent)" stroke="var(--ok)"/><path fill="var(--warn)" stroke="var(--error)"/><g fill="var(--surface-alt)" stroke="var(--muted)"/></svg>"#;
        let resolved = Theme::light().resolve_svg_vars(svg);
        assert!(!resolved.contains("var(--"));
        assert!(resolved.contains(Theme::light().bg.hex));
        assert!(resolved.contains(Theme::light().accent.hex));
        assert!(resolved.contains(Theme::light().error.hex));
    }
}

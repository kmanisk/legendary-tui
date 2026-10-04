//! System theme integration (read-only).
//!
//! Parses `~/.local/state/theme/colors.sh` (written by theme-set) so the TUI
//! follows the active desktop theme: accent-fill selection with dark text
//! (same language as the Rofi system theme), muted borders. Falls back to
//! dark defaults when the file is missing. Never writes anything.

use ratatui::style::Color;

use crate::filesystem;

#[derive(Clone, Debug)]
pub struct Theme {
    pub bg: Color,
    pub bg2: Color,
    pub fg: Color,
    pub accent: Color,
    pub sel: Color,
    pub muted: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub cyan: Color,
    /// Dark text for use on top of accent fills.
    pub on_accent: Color,
}

fn hex(s: &str, fallback: Color) -> Color {
    let s = s.trim().trim_matches('"').trim_start_matches('#');
    if s.len() == 6 {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&s[0..2], 16),
            u8::from_str_radix(&s[2..4], 16),
            u8::from_str_radix(&s[4..6], 16),
        ) {
            return Color::Rgb(r, g, b);
        }
    }
    fallback
}

fn var(text: &str, name: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some(rest) = line.strip_prefix(name) {
            let rest = rest.trim_start();
            if let Some(val) = rest.strip_prefix('=') {
                let v = val.trim().trim_matches('"').trim_matches('\'').trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

pub fn is_bright(c: Color) -> bool {
    if let Color::Rgb(r, g, b) = c {
        let lum = 0.299 * (r as f32) + 0.587 * (g as f32) + 0.114 * (b as f32);
        lum > 140.0
    } else {
        false
    }
}

pub fn theme_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(filesystem::home().join(".local/state/theme/colors.sh"))
        .ok()
        .and_then(|m| m.modified().ok())
}

pub fn load() -> Theme {
    let text = std::fs::read_to_string(filesystem::home().join(".local/state/theme/colors.sh"))
        .unwrap_or_default();
    let get = |name: &str, fb: Color| var(&text, name).map(|v| hex(&v, fb)).unwrap_or(fb);
    let accent_col = get("THEME_ACCENT", Color::Rgb(0x6d, 0x8d, 0xad));
    let on_accent = if is_bright(accent_col) {
        Color::Rgb(0x11, 0x14, 0x16)
    } else {
        Color::Rgb(0xff, 0xff, 0xff)
    };
    Theme {
        bg: get("THEME_BG", Color::Rgb(0x1e, 0x21, 0x22)),
        bg2: get("THEME_BG2", Color::Rgb(0x28, 0x2b, 0x2c)),
        fg: get("THEME_FG", Color::Rgb(0xc7, 0xb8, 0x9d)),
        accent: accent_col,
        sel: get("THEME_SEL", Color::Rgb(0x39, 0x3c, 0x3d)),
        muted: get("THEME_MUTED", Color::Rgb(0x57, 0x5a, 0x5b)),
        red: get("THEME_RED", Color::Rgb(0xec, 0x6b, 0x64)),
        green: get("THEME_GREEN", Color::Rgb(0x89, 0xb4, 0x82)),
        yellow: get("THEME_YELLOW", Color::Rgb(0xd6, 0xb6, 0x76)),
        cyan: get("THEME_CYAN", Color::Rgb(0x82, 0xb3, 0xa8)),
        on_accent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_defaults_without_file() {
        let t = Theme {
            bg: hex("", Color::Black),
            bg2: Color::Black,
            fg: Color::Black,
            accent: Color::Black,
            sel: Color::Black,
            muted: Color::Black,
            red: Color::Black,
            green: Color::Black,
            yellow: Color::Black,
            cyan: Color::Black,
            on_accent: Color::White,
        };
        assert!(matches!(t.bg, Color::Black));
        assert_eq!(hex("#6d8dad", Color::Black), Color::Rgb(0x6d, 0x8d, 0xad));
        assert_eq!(
            var("export THEME_BG=\"#1e2122\"\n", "THEME_BG"),
            Some("#1e2122".to_string())
        );
        // Verify THEME_BG does not accidentally match THEME_BG2
        assert_eq!(var("export THEME_BG2=\"#282b2c\"\n", "THEME_BG"), None);
    }
}

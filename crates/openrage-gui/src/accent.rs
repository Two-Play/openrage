//! The accent color (View → Accent color): the widget style's, OpenRAGE
//! orange (the logo's) or one of the user's choice. Every window gets it when
//! it opens and when it changes; it is saved in the settings as `accent`.

use std::cell::Cell;

use slint::{Color, ComponentHandle};

use crate::Theme;

thread_local! {
    static CURRENT: Cell<(i32, Color)> = const { Cell::new((0, Color::from_rgb_u8(0xc2, 0x41, 0x0c))) };
}

/// Applies the current accent to a window.
pub fn apply<T>(win: &T)
where
    T: ComponentHandle,
    for<'a> Theme<'a>: slint::Global<'a, T>,
{
    let (mode, color) = CURRENT.with(Cell::get);
    let theme = win.global::<Theme>();
    theme.set_accent_custom(color);
    theme.set_accent_mode(mode);
}

/// Sets the current accent (applying it is up to the caller).
pub fn set(mode: i32, color: Color) {
    CURRENT.with(|c| c.set((mode, color)));
}

/// The setting's text: `orange`, `system` or `#rrggbb`.
pub fn to_setting(mode: i32, color: Color) -> String {
    match mode {
        0 => "system".to_owned(),
        2 => hex(color),
        _ => "orange".to_owned(),
    }
}

/// Reads the setting (the style's own accent when missing or unreadable).
pub fn from_setting(value: Option<&str>) -> (i32, Color) {
    let orange = Color::from_rgb_u8(0xc2, 0x41, 0x0c);
    match value {
        Some("orange") => (1, orange),
        Some(v) => parse_hex(v).map_or((0, orange), |c| (2, c)),
        None => (0, orange),
    }
}

/// `#rrggbb` as a Slint string (for the color dialog).
pub fn hex_string(c: Color) -> slint::SharedString {
    hex(c).into()
}

/// `#rrggbb`.
pub fn hex(c: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue())
}

/// Parses `#rrggbb` or `rrggbb` (also `#rgb`).
pub fn parse_hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    let s = if s.len() == 3 { s.chars().flat_map(|c| [c, c]).collect::<String>() } else { s.to_owned() };
    if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(&s, 16).ok()?;
    Some(Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip() {
        assert_eq!(parse_hex("#C2410C"), Some(Color::from_rgb_u8(0xc2, 0x41, 0x0c)));
        assert_eq!(parse_hex("f80"), Some(Color::from_rgb_u8(0xff, 0x88, 0x00)));
        assert_eq!(parse_hex("#12345"), None);
        let c = Color::from_rgb_u8(1, 2, 3);
        assert_eq!(from_setting(Some(&to_setting(2, c))), (2, c));
        assert_eq!(from_setting(Some("system")).0, 0);
        assert_eq!(from_setting(Some("orange")).0, 1);
        assert_eq!(from_setting(None).0, 0);
    }
}

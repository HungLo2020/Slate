//! Terminal colour depth. Slate's palette is defined in RGB; terminals
//! without truecolor get the nearest 256- or 16-colour entry, and
//! `NO_COLOR` disables colour entirely (selection uses reverse video).
use ratatui::style::{Color, Modifier, Style};
use std::{cell::RefCell, collections::HashMap};

thread_local! {
    static CACHE: RefCell<HashMap<(bool, u32), Color>> = RefCell::new(HashMap::new());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    TrueColor,
    Ansi256,
    Ansi16,
    None,
}

impl ColorMode {
    pub fn detect() -> Self {
        Self::from_env(|name| std::env::var(name).ok())
    }
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Self {
        if let Some(forced) = var("SLATE_COLORS") {
            match forced.as_str() {
                "truecolor" | "24bit" => return Self::TrueColor,
                "256" => return Self::Ansi256,
                "16" => return Self::Ansi16,
                "none" | "0" => return Self::None,
                _ => {}
            }
        }
        if var("NO_COLOR").is_some_and(|v| !v.is_empty()) {
            return Self::None;
        }
        let term = var("TERM").unwrap_or_default();
        if term == "dumb" {
            return Self::None;
        }
        if var("COLORTERM").is_some_and(|v| v == "truecolor" || v == "24bit")
            || term.contains("direct")
        {
            return Self::TrueColor;
        }
        if term.contains("256") {
            return Self::Ansi256;
        }
        if term == "linux" || term.starts_with("vt") || term == "xterm" || term == "screen" {
            return Self::Ansi16;
        }
        Self::Ansi256
    }
    pub fn color(self, hex: &str) -> Color {
        let value = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
        let (r, g, b) = ((value >> 16) as u8, (value >> 8) as u8, value as u8);
        match self {
            Self::TrueColor => Color::Rgb(r, g, b),
            Self::None => Color::Reset,
            // Every cell is converted on every frame; a document uses few colours.
            mode => CACHE.with(|cache| {
                *cache
                    .borrow_mut()
                    .entry((mode == Self::Ansi16, value))
                    .or_insert_with(|| {
                        if mode == Self::Ansi16 {
                            nearest_16(r, g, b)
                        } else {
                            Color::Indexed(nearest_256(r, g, b))
                        }
                    })
            }),
        }
    }
    pub fn style(self, fg: &str, bg: &str) -> Style {
        if self == Self::None {
            return Style::default();
        }
        Style::default().fg(self.color(fg)).bg(self.color(bg))
    }
    pub fn border(self, focused: bool) -> Style {
        match (self, focused) {
            (Self::None, true) => Style::default().add_modifier(Modifier::BOLD),
            (Self::None, false) => Style::default(),
            (_, true) => Style::default().fg(Color::Cyan),
            (_, false) => Style::default().fg(Color::DarkGray),
        }
    }
    pub fn tab(self, active: bool) -> Style {
        match (self, active) {
            (Self::None, true) => {
                Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            }
            (Self::None, false) => Style::default(),
            (_, true) => Style::default().fg(Color::Cyan),
            (_, false) => Style::default().fg(Color::Gray),
        }
    }
    pub fn highlight(self) -> Style {
        if self == Self::None {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default().bg(Color::DarkGray)
        }
    }
    pub fn status(self) -> Style {
        if self == Self::None {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(Color::White).bg(Color::DarkGray)
        }
    }
    pub fn palette_item(self, enabled: bool) -> Style {
        match (self, enabled) {
            (Self::None, true) => Style::default(),
            (Self::None, false) => Style::default().add_modifier(Modifier::DIM),
            (_, true) => Style::default().fg(Color::White),
            (_, false) => Style::default().fg(Color::DarkGray),
        }
    }
}

fn distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2) as u32;
    d(a.0, b.0) * 3 + d(a.1, b.1) * 4 + d(a.2, b.2) * 2
}

/// The closest xterm-256 entry from the 6×6×6 cube or the grey ramp.
pub fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let level = |v: u8| {
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, l)| (**l as i32 - v as i32).abs())
            .map(|(i, _)| i as u8)
            .unwrap()
    };
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = (
        16 + 36 * ri + 6 * gi + bi,
        (
            LEVELS[ri as usize],
            LEVELS[gi as usize],
            LEVELS[bi as usize],
        ),
    );
    let average = (r as u32 + g as u32 + b as u32) / 3;
    let grey_index = (average.saturating_sub(8) / 10).min(23) as u8;
    let grey_value = 8 + grey_index * 10;
    let grey = (232 + grey_index, (grey_value, grey_value, grey_value));
    if distance((r, g, b), grey.1) < distance((r, g, b), cube.1) {
        grey.0
    } else {
        cube.0
    }
}

/// The closest of the 16 basic ANSI colours (xterm's default values).
pub fn nearest_16(r: u8, g: u8, b: u8) -> Color {
    const ANSI: [(Color, (u8, u8, u8)); 16] = [
        (Color::Black, (0, 0, 0)),
        (Color::Red, (205, 0, 0)),
        (Color::Green, (0, 205, 0)),
        (Color::Yellow, (205, 205, 0)),
        (Color::Blue, (0, 0, 238)),
        (Color::Magenta, (205, 0, 205)),
        (Color::Cyan, (0, 205, 205)),
        (Color::Gray, (229, 229, 229)),
        (Color::DarkGray, (127, 127, 127)),
        (Color::LightRed, (255, 0, 0)),
        (Color::LightGreen, (0, 255, 0)),
        (Color::LightYellow, (255, 255, 0)),
        (Color::LightBlue, (92, 92, 255)),
        (Color::LightMagenta, (255, 0, 255)),
        (Color::LightCyan, (0, 255, 255)),
        (Color::White, (255, 255, 255)),
    ];
    ANSI.iter()
        .min_by_key(|(_, rgb)| distance((r, g, b), *rgb))
        .map(|(c, _)| *c)
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_and_mapping() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(
            ColorMode::from_env(env(&[
                ("COLORTERM", "truecolor"),
                ("TERM", "xterm-256color")
            ])),
            ColorMode::TrueColor
        );
        assert_eq!(
            ColorMode::from_env(env(&[("TERM", "xterm-256color")])),
            ColorMode::Ansi256
        );
        assert_eq!(
            ColorMode::from_env(env(&[("TERM", "linux")])),
            ColorMode::Ansi16
        );
        assert_eq!(
            ColorMode::from_env(env(&[("TERM", "xterm-256color"), ("NO_COLOR", "1")])),
            ColorMode::None
        );
        assert_eq!(
            ColorMode::from_env(env(&[("TERM", "dumb")])),
            ColorMode::None
        );
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(255, 255, 255), 231);
        assert_eq!(nearest_256(0x80, 0x80, 0x80), 244);
        assert_eq!(nearest_16(0x20, 0x24, 0x2c), Color::Black);
        assert_eq!(nearest_16(250, 250, 250), Color::White);
        assert_eq!(ColorMode::Ansi256.color("#ff0000"), Color::Indexed(196));
        assert_eq!(ColorMode::None.color("#ff0000"), Color::Reset);
    }
}

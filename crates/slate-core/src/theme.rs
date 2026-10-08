//! Content palettes shared by both frontends. Desktop chrome remains native;
//! editor and terminal content select their palettes independently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub foreground: String,
    pub background: String,
    pub selection: String,
    pub selection_foreground: String,
    pub accent: String,
    pub ansi: [String; 16],
}

impl Palette {
    pub fn dark(terminal: bool) -> Self {
        Self::new(
            "#d8dee9",
            if terminal { "#14171c" } else { "#1b1e26" },
            "#35465e",
            "#ffffff",
            "#88c0d0",
        )
    }

    pub fn light() -> Self {
        Self::new("#20242c", "#fafafa", "#bdd9f5", "#20242c", "#1769aa")
    }

    pub fn new(fg: &str, bg: &str, selection: &str, selection_fg: &str, accent: &str) -> Self {
        let colors = if is_light(bg) {
            [
                "#20242c", "#a82435", "#326522", "#805d00", "#245c91", "#784c92", "#166b75",
                "#666c76", "#687080", "#bb3344", "#397a27", "#926b00", "#326fa6", "#915ba3",
                "#237d87", "#ffffff",
            ]
        } else {
            [
                "#14171c", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0",
                "#e5e9f0", "#4c566a", "#d08770", "#b8d7a3", "#f0dcab", "#a3c5e5", "#c9acd7",
                "#a3d8e0", "#ffffff",
            ]
        };
        Self {
            foreground: fg.into(),
            background: bg.into(),
            selection: selection.into(),
            selection_foreground: selection_fg.into(),
            accent: accent.into(),
            ansi: colors.map(String::from),
        }
    }

    pub fn is_light(&self) -> bool {
        is_light(&self.background)
    }
}

pub fn is_light(color: &str) -> bool {
    let color = u32::from_str_radix(color.trim_start_matches('#'), 16).unwrap_or(0);
    ((color >> 16) & 255) * 299 + ((color >> 8) & 255) * 587 + (color & 255) * 114 > 128_000
}

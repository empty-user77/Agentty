//! Workbench chrome colors (VS Code "Dark Modern"-inspired) and terminal color themes.
//! Terminal themes are built in or imported from iTerm2 `.itermcolors` files in `~/.agentty/themes`.

use gpui::{rgb, FontWeight, Hsla, Rgba};
use std::path::{Path, PathBuf};

pub fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

pub fn hex_alpha(value: u32, alpha: f32) -> Hsla {
    let mut color: Rgba = rgb(value);
    color.a = alpha;
    color.into()
}

/// `value` mixed toward white by `amount` (0…1), kept fully opaque. Translucency over the dark
/// chrome turns a colour into mud; a lighter solid tone is what "a shade brighter" should mean.
pub fn lighten(value: u32, amount: f32) -> Hsla {
    hex(lighten_rgb(value, amount))
}

/// [`lighten`] as a plain `0xRRGGBB`, for when the fill also has to be measured (see [`ink_on`]).
pub fn lighten_rgb(value: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let c = ((value >> shift) & 0xff) as f32;
        (c + (255. - c) * amount.clamp(0., 1.)).round() as u32
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

/// Ink on a light fill. Not pure black, for the same reason [`Chrome::BRIGHT`] is not pure white.
pub const INK: u32 = 0x14141a;

/// Relative luminance of an sRGB colour (WCAG 2.1): 0 for black, 1 for white. The channels are
/// weighted by how bright the eye finds them, so a saturated yellow counts as light and a
/// saturated blue as dark — which is the whole point of asking.
pub fn luminance(value: u32) -> f32 {
    let channel = |shift: u32| {
        let c = ((value >> shift) & 0xff) as f32 / 255.;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

/// WCAG contrast ratio between two colours, 1 (identical) to 21 (black on white).
pub fn contrast(a: u32, b: u32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

/// Text that stays readable on `background`: [`INK`] on a light fill, [`Chrome::BRIGHT`] on a dark
/// one. A colour the user picked for a card can be anything — a pale yellow leaves white letters
/// invisible — so the choice is measured against the fill rather than assumed.
pub fn ink_on(background: u32) -> u32 {
    if contrast(INK, background) >= contrast(Chrome::BRIGHT, background) {
        INK
    } else {
        Chrome::BRIGHT
    }
}

pub struct Chrome;

/// Weight for the names and headings the UI repeats everywhere (cards, rows, section headers).
/// One step below bold: a screen full of semibold at 13px reads as noise rather than emphasis.
/// Every such place goes through this constant, so the whole app is tuned in one line.
pub const EMPHASIS: FontWeight = FontWeight::MEDIUM;

impl Chrome {
    // Slightly cooler and a touch further apart than VS Code's greys, so the sidebar, the
    // terminals and the panels read as separate surfaces instead of one flat dark field.
    pub const ACTIVITY_BAR: u32 = 0x151517;
    pub const SIDE_BAR: u32 = 0x17171a;
    pub const EDITOR: u32 = 0x1e1e21;
    pub const PANEL: u32 = 0x212124;
    pub const TAB_INACTIVE: u32 = 0x1a1a1d;
    pub const STATUS_BAR: u32 = 0x17171a;
    pub const BORDER: u32 = 0x2f2f34;
    pub const FOREGROUND: u32 = 0xd2d2d8;
    /// Not pure white: at this size it glares against the dark chrome.
    pub const BRIGHT: u32 = 0xf2f2f5;
    pub const MUTED: u32 = 0x8e8e96;
    pub const ACCENT: u32 = 0x0078d4;
    pub const ATTENTION: u32 = 0x3b9cff;
    pub const HOVER: u32 = 0x2a2a30;
    pub const SELECTED: u32 = 0x35353c;
    pub const OVERLAY: u32 = 0x232326;
    pub const OVERLAY_BORDER: u32 = 0x41414a;
    pub const CLAUDE: u32 = 0xd97757;
    pub const CODEX: u32 = 0x10a37f;
    pub const SHELL: u32 = 0x8b8b8b;
    pub const SUCCESS: u32 = 0x89d185;
    pub const WARNING: u32 = 0xcca700;
    pub const ERROR: u32 = 0xf14c4c;
    pub const PURPLE: u32 = 0xb180d7;
    pub const BLUE: u32 = 0x4daafc;
    pub const ORANGE: u32 = 0xe8a33d;
    pub const GREEN: u32 = 0x4ec9b0;
    pub const FAVORITE: u32 = 0xf5c518;
}

#[derive(Debug, Clone, PartialEq)]
pub struct TerminalTheme {
    pub name: String,
    pub background: u32,
    pub foreground: u32,
    pub cursor: u32,
    pub selection: u32,
    pub ansi: [u32; 16],
    /// Imported from `~/.agentty/themes` rather than built in.
    pub imported: bool,
}

impl TerminalTheme {
    const fn builtin(name: &'static str, background: u32, foreground: u32, cursor: u32, selection: u32, ansi: [u32; 16]) -> BuiltinTheme {
        BuiltinTheme { name, background, foreground, cursor, selection, ansi }
    }

    /// xterm 256-color table: 16 theme colors, 6x6x6 cube, 24-step grayscale.
    pub fn indexed(&self, index: u8) -> u32 {
        match index {
            0..=15 => self.ansi[index as usize],
            16..=231 => {
                let i = index - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v as u32 * 40 };
                (level(i / 36) << 16) | (level((i / 6) % 6) << 8) | level(i % 6)
            }
            232..=255 => {
                let v = 8 + (index as u32 - 232) * 10;
                (v << 16) | (v << 8) | v
            }
        }
    }
}

pub struct BuiltinTheme {
    name: &'static str,
    background: u32,
    foreground: u32,
    cursor: u32,
    selection: u32,
    ansi: [u32; 16],
}

pub const DEFAULT_THEME: &str = "Apple System Colors";
/// Default theme of Agentty 0.1 builds, migrated to [`DEFAULT_THEME`].
pub const LEGACY_DEFAULT_THEME: &str = "Agentty Dark";

const BUILTIN: &[BuiltinTheme] = &[
    // Ghostty's "Apple System Colors" — the default dark theme in cmux.
    TerminalTheme::builtin(
        "Apple System Colors",
        0x1e1e1e,
        0xffffff,
        0x98989d,
        0x3f638b,
        [
            0x1a1a1a, 0xcc372e, 0x26a439, 0xcdac08, 0x0869cb, 0x9647bf, 0x479ec2, 0x98989d, 0x464646, 0xff453a, 0x32d74b, 0xffd60a,
            0x0a84ff, 0xbf5af2, 0x76d6ff, 0xffffff,
        ],
    ),
    TerminalTheme::builtin(
        "Tomorrow Night",
        0x282c34,
        0xffffff,
        0xc5c8c6,
        0x373b41,
        [
            0x1d1f21, 0xcc6666, 0xb5bd68, 0xf0c674, 0x81a2be, 0xb294bb, 0x8abeb7, 0xc5c8c6, 0x666666, 0xd54e53, 0xb9ca4a, 0xe7c547,
            0x7aa6da, 0xc397d8, 0x70c0b1, 0xeaeaea,
        ],
    ),
    TerminalTheme::builtin(
        "VS Code Dark",
        0x1f1f1f,
        0xcccccc,
        0xaeafad,
        0x264f78,
        [
            0x000000, 0xcd3131, 0x0dbc79, 0xe5e510, 0x2472c8, 0xbc3fbc, 0x11a8cd, 0xe5e5e5, 0x666666, 0xf14c4c, 0x23d18b, 0xf5f543,
            0x3b8eea, 0xd670d6, 0x29b8db, 0xe5e5e5,
        ],
    ),
    TerminalTheme::builtin(
        "Dracula",
        0x282a36,
        0xf8f8f2,
        0xf8f8f2,
        0x44475a,
        [
            0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2, 0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5,
            0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
        ],
    ),
    TerminalTheme::builtin(
        "One Dark",
        0x282c34,
        0xabb2bf,
        0x528bff,
        0x3e4451,
        [
            0x282c34, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, 0x5c6370, 0xe06c75, 0x98c379, 0xe5c07b,
            0x61afef, 0xc678dd, 0x56b6c2, 0xffffff,
        ],
    ),
    TerminalTheme::builtin(
        "Catppuccin Mocha",
        0x1e1e2e,
        0xcdd6f4,
        0xf5e0dc,
        0x585b70,
        [
            0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, 0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af,
            0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
        ],
    ),
    TerminalTheme::builtin(
        "Nord",
        0x2e3440,
        0xd8dee9,
        0xd8dee9,
        0x434c5e,
        [
            0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0, 0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b,
            0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
        ],
    ),
    TerminalTheme::builtin(
        "Gruvbox Dark",
        0x282828,
        0xebdbb2,
        0xebdbb2,
        0x504945,
        [
            0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984, 0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f,
            0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
        ],
    ),
    TerminalTheme::builtin(
        "Solarized Dark",
        0x002b36,
        0x839496,
        0x93a1a1,
        0x073642,
        [
            0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5, 0x002b36, 0xcb4b16, 0x586e75, 0x657b83,
            0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
        ],
    ),
    TerminalTheme::builtin(
        "Solarized Light",
        0xfdf6e3,
        0x657b83,
        0x586e75,
        0xeee8d5,
        [
            0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5, 0x002b36, 0xcb4b16, 0x586e75, 0x657b83,
            0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
        ],
    ),
    TerminalTheme::builtin(
        "GitHub Light",
        0xffffff,
        0x24292f,
        0x0969da,
        0xb6e3ff,
        [
            0x24292f, 0xcf222e, 0x116329, 0x4d2d00, 0x0550ae, 0x8250df, 0x1b7c83, 0x6e7781, 0x57606a, 0xa40e26, 0x1a7f37, 0x633c01,
            0x0969da, 0x8250df, 0x1b7c83, 0x8c959f,
        ],
    ),
];

pub fn themes_dir() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("themes")
}

/// Built-in themes followed by every `.itermcolors` file in the themes directory.
pub fn load_themes() -> Vec<TerminalTheme> {
    let mut themes: Vec<TerminalTheme> = BUILTIN
        .iter()
        .map(|b| TerminalTheme {
            name: b.name.to_string(),
            background: b.background,
            foreground: b.foreground,
            cursor: b.cursor,
            selection: b.selection,
            ansi: b.ansi,
            imported: false,
        })
        .collect();
    if let Ok(entries) = std::fs::read_dir(themes_dir()) {
        let mut imported: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "itermcolors"))
            .filter_map(|p| parse_itermcolors(&p).ok())
            .collect();
        imported.sort_by(|a, b| a.name.cmp(&b.name));
        themes.extend(imported);
    }
    themes
}

/// Parses an iTerm2 color preset (XML plist of `{Red,Green,Blue} Component` floats).
pub fn parse_itermcolors(path: &Path) -> anyhow::Result<TerminalTheme> {
    let dict = plist::Value::from_file(path)?.into_dictionary().ok_or_else(|| anyhow::anyhow!("not a color preset"))?;
    let color = |key: &str| -> Option<u32> {
        let entry = dict.get(key)?.as_dictionary()?;
        let channel = |name: &str| -> u32 {
            let value = entry.get(name).and_then(|v| v.as_real().or_else(|| v.as_signed_integer().map(|i| i as f64))).unwrap_or(0.0);
            (value.clamp(0.0, 1.0) * 255.0).round() as u32
        };
        Some((channel("Red Component") << 16) | (channel("Green Component") << 8) | channel("Blue Component"))
    };
    let base = &BUILTIN[0];
    let mut ansi = base.ansi;
    for (i, slot) in ansi.iter_mut().enumerate() {
        if let Some(c) = color(&format!("Ansi {i} Color")) {
            *slot = c;
        }
    }
    let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Imported".into());
    let background = color("Background Color").unwrap_or(base.background);
    let foreground = color("Foreground Color").unwrap_or(base.foreground);
    Ok(TerminalTheme {
        name,
        background,
        foreground,
        cursor: color("Cursor Color").unwrap_or(foreground),
        selection: color("Selection Color").unwrap_or(base.selection),
        ansi,
        imported: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_itermcolors() {
        let dir = std::env::temp_dir().join(format!("agentty-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Test Theme.itermcolors");
        std::fs::write(
            &path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Ansi 1 Color</key><dict><key>Red Component</key><real>1</real><key>Green Component</key><real>0</real><key>Blue Component</key><real>0</real></dict>
<key>Background Color</key><dict><key>Red Component</key><real>0.0</real><key>Green Component</key><real>0.0</real><key>Blue Component</key><real>0.5</real></dict>
</dict></plist>"#,
        )
        .unwrap();
        let theme = parse_itermcolors(&path).unwrap();
        assert_eq!(theme.name, "Test Theme");
        assert_eq!(theme.ansi[1], 0xff0000);
        assert_eq!(theme.background, 0x000080);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn builtin_names_are_unique() {
        let themes = load_themes();
        let mut names: Vec<_> = themes.iter().filter(|t| !t.imported).map(|t| t.name.clone()).collect();
        names.dedup();
        assert_eq!(names.len(), BUILTIN.len());
        assert!(themes.iter().any(|t| t.name == DEFAULT_THEME));
    }
}

#[cfg(test)]
mod tone_tests {
    use super::{contrast, ink_on, lighten, luminance, Chrome, INK};

    #[test]
    fn lighten_stays_opaque_and_moves_toward_white() {
        let base = lighten(0x2f6fed, 0.0);
        let lighter = lighten(0x2f6fed, 0.2);
        assert_eq!(base.a, 1.0);
        assert_eq!(lighter.a, 1.0);
        assert!(lighter.l > base.l, "lightening must raise the lightness");
        assert_eq!(lighten(0x000000, 1.0).l, 1.0);
    }

    #[test]
    fn luminance_runs_from_black_to_white_and_weighs_green_most() {
        assert!(luminance(0x000000).abs() < 0.001);
        assert!((luminance(0xffffff) - 1.0).abs() < 0.001);
        assert!(luminance(0x00ff00) > luminance(0xff0000), "green looks brighter than red");
        assert!(luminance(0xff0000) > luminance(0x0000ff), "red looks brighter than blue");
    }

    #[test]
    fn contrast_is_symmetric_and_peaks_between_black_and_white() {
        assert!((contrast(0x000000, 0xffffff) - 21.0).abs() < 0.01);
        assert!((contrast(0xffffff, 0x000000) - 21.0).abs() < 0.01);
        assert!((contrast(0x2f6fed, 0x2f6fed) - 1.0).abs() < 0.001);
    }

    /// The whole point: whichever ink is picked has to be the readable one.
    #[test]
    fn ink_follows_the_fill_and_always_takes_the_higher_contrast() {
        assert_eq!(ink_on(0xffffff), INK, "white card, dark letters");
        assert_eq!(ink_on(0x000000), Chrome::BRIGHT, "black card, light letters");
        // The pale end of the built-in palette, where white letters used to wash out.
        for light in [0xf0b357, 0x7aa2f7, 0x5fd1a0, 0xe887b6, 0x94a3b8, 0xb69cfb, 0x4fc7d3, 0xe97b7b] {
            assert_eq!(ink_on(light), INK, "{light:#08x} is a light fill");
        }
        // The deep end, where dark letters would.
        for dark in [0x1b46a8, 0x136b3a, 0x8f2c2c, 0x5b34c2, 0x076a75, 0x9a2c63, 0x3a4553, 0x2f6fed] {
            assert_eq!(ink_on(dark), Chrome::BRIGHT, "{dark:#08x} is a dark fill");
        }
    }

    #[test]
    fn every_colour_a_user_can_pick_gets_readable_ink() {
        // Every 5th step of the whole 24-bit space — the colour panel hands out any of them. No
        // fill may take the worse of the two inks. 4:1 is the floor rather than the 4.5:1 the
        // guideline asks of body text: on a mid-grey fill neither black nor white can reach it,
        // and the card's name is a size up and a weight heavier than body text.
        for r in (0u32..=255).step_by(5) {
            for g in (0u32..=255).step_by(5) {
                for b in (0u32..=255).step_by(5) {
                    let fill = (r << 16) | (g << 8) | b;
                    let ink = ink_on(fill);
                    let other = if ink == INK { Chrome::BRIGHT } else { INK };
                    let (chosen, rejected) = (contrast(ink, fill), contrast(other, fill));
                    assert!(chosen >= rejected, "{fill:#08x} took the worse ink ({chosen:.2} < {rejected:.2})");
                    assert!(chosen >= 4.0, "{fill:#08x} left its title at {chosen:.2}:1");
                }
            }
        }
    }
}

//! Agent identity for icons and labels: brand logo (Simple Icons, CC0), accent color and name.

use crate::theme::{hex, hex_alpha, Chrome};
use gpui::{div, prelude::*, px, svg, AnyElement, FontWeight, SharedString};

pub struct Brand {
    pub id: &'static str,
    pub name: &'static str,
    /// Monochrome SVG under `logos/`, tinted with `color`.
    pub logo: Option<&'static str>,
    pub color: u32,
}

const BRANDS: &[Brand] = &[
    Brand { id: "claude", name: "Claude Code", logo: Some("logos/claude.svg"), color: 0xd97757 },
    Brand { id: "codex", name: "Codex", logo: Some("logos/openai.svg"), color: 0xe8e8e8 },
    Brand { id: "gemini", name: "Gemini CLI", logo: Some("logos/googlegemini.svg"), color: 0x4796e3 },
    Brand { id: "agy", name: "Antigravity CLI", logo: Some("logos/antigravity.svg"), color: 0x3186ff },
    Brand { id: "amp", name: "Amp", logo: None, color: 0xf34e3f },
    Brand { id: "copilot", name: "GitHub Copilot CLI", logo: Some("logos/githubcopilot.svg"), color: 0xb392f0 },
    Brand { id: "cursor", name: "Cursor CLI", logo: Some("logos/cursor.svg"), color: 0xe6e6e6 },
    Brand { id: "opencode", name: "OpenCode", logo: Some("logos/opencode.svg"), color: 0xf5a623 },
    Brand { id: "qwen", name: "Qwen Code", logo: Some("logos/qwen.svg"), color: 0x8b87ff },
    Brand { id: "kimi", name: "Kimi CLI", logo: Some("logos/kimi.svg"), color: 0x4a9dff },
    Brand { id: "cline", name: "Cline CLI", logo: Some("logos/cline.svg"), color: 0xd4d4d4 },
    Brand { id: "grok", name: "Grok Build", logo: Some("logos/x.svg"), color: 0xe5e5e5 },
    Brand { id: "ollama", name: "Ollama", logo: Some("logos/ollama.svg"), color: 0xe8e8e8 },
];

const SHELL: Brand = Brand { id: "shell", name: "Terminal", logo: None, color: Chrome::SHELL };

/// Brand for an agent id; unknown ids (and `shell`) get the terminal look.
pub fn brand(id: &str) -> &'static Brand {
    static OTHERS: std::sync::OnceLock<Vec<Brand>> = std::sync::OnceLock::new();
    if let Some(found) = BRANDS.iter().find(|b| b.id == id) {
        return found;
    }
    // Other known CLIs: letter avatar in their color.
    let others = OTHERS
        .get_or_init(|| crate::agents::OTHER_AGENTS.iter().map(|a| Brand { id: a.id, name: a.name, logo: None, color: a.color }).collect());
    others.iter().find(|b| b.id == id).unwrap_or(&SHELL)
}

pub fn kind_id(kind: crate::launch::PaneKind) -> &'static str {
    match kind {
        crate::launch::PaneKind::Shell => "shell",
        crate::launch::PaneKind::Claude => "claude",
        crate::launch::PaneKind::Codex => "codex",
    }
}

/// Every logo path, for the asset test.
#[cfg(test)]
pub fn logo_paths() -> impl Iterator<Item = &'static str> {
    BRANDS.iter().filter_map(|b| b.logo)
}

/// Round avatar with the tool's logo (or first letter / terminal glyph), `size` px wide.
///
/// Drawn in one plain tone rather than each brand's colour: a window full of orange, green and
/// blue badges pulls the eye away from the work. The brand colour is still what `brand().color`
/// gives anything that really needs to tell tools apart.
pub fn avatar(id: &str, size: f32) -> gpui::Div {
    plate(id, size, false)
}

/// The same avatar in the tool's own colour. For the places that are about choosing a tool — the
/// launcher menu — where the colour is the point rather than noise.
pub fn avatar_brand(id: &str, size: f32) -> gpui::Div {
    plate(id, size, true)
}

/// The tool's logo, or — while a turn is running — the braille dot spinner in its place, the same
/// mark everywhere it appears. `key` keeps each row's animation apart.
pub fn avatar_working(id: &str, size: f32, working: bool, key: u64) -> AnyElement {
    if !working {
        return avatar(id, size).into_any_element();
    }
    crate::ui::dot_spinner(("agent-working", key as usize), size, hex_alpha(Chrome::BRIGHT, 0.9)).into_any_element()
}

fn plate(id: &str, size: f32, colored: bool) -> gpui::Div {
    let brand = brand(id);
    // The plain one is the glyph and nothing else: a dark disc behind every logo, on a card that
    // already has a colour of its own, reads as a hole punched in it.
    let inner = if colored { size * 0.64 } else { size * 0.9 };
    let base = div().flex_shrink_0().size(px(size)).rounded_full().flex().items_center().justify_center().when(colored, |d| {
        d.bg(hex_alpha(0xffffff, 0.08)).border_1().border_color(hex_alpha(if brand.id == "shell" { 0x9a9a9a } else { brand.color }, 0.85))
    });
    base.child(glyph(brand, inner, size, colored))
}

/// The tool's logo, terminal glyph or first letter, in one quiet tone or in its brand colour.
fn glyph(brand: &Brand, inner: f32, size: f32, colored: bool) -> AnyElement {
    let tone = if colored { hex(brand.color) } else { hex_alpha(Chrome::BRIGHT, 0.85) };
    let tone = if colored && brand.id == "shell" { hex(Chrome::BRIGHT) } else { tone };
    match (brand.logo, brand.id) {
        (Some(path), _) => svg().path(SharedString::from(path)).size(px(inner)).text_color(tone).into_any_element(),
        (None, "shell") => crate::ui::icon("terminal", inner, tone).into_any_element(),
        (None, _) => div()
            .text_size(px((size * 0.52).max(7.)))
            .font_weight(FontWeight::BOLD)
            .text_color(tone)
            .child(brand.name.chars().next().unwrap_or('?').to_string())
            .into_any_element(),
    }
}

/// Square tile tinted with the tool's color, like an app icon: the start page's cards.
pub fn tile(id: &str, size: f32) -> gpui::Div {
    let brand = brand(id);
    // The start page's cards are a wall of tools to choose from: colour tells them apart.
    tinted_tile(if brand.id == "shell" { 0x9a9a9a } else { brand.color }, size).child(glyph(brand, size * 0.56, size, true))
}

/// An empty [`tile`] in any color, for cards that show an icon instead of a brand.
pub fn tinted_tile(color: u32, size: f32) -> gpui::Div {
    div()
        .flex_shrink_0()
        .size(px(size))
        .rounded_lg()
        .flex()
        .items_center()
        .justify_center()
        .bg(hex_alpha(color, 0.16))
        .border_1()
        .border_color(hex_alpha(color, 0.35))
}

#[cfg(test)]
mod tests {
    #[test]
    fn known_and_unknown_brands() {
        assert_eq!(super::brand("claude").name, "Claude Code");
        assert_eq!(super::brand("agy").name, "Antigravity CLI");
        assert_eq!(super::brand("droid").name, "Factory Droid");
        assert_eq!(super::brand("zsh").id, "shell");
    }
}

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
    Brand { id: "agy", name: "Antigravity CLI", logo: None, color: 0x3c82f6 },
    Brand { id: "amp", name: "Amp", logo: None, color: 0xf34e3f },
    Brand { id: "copilot", name: "GitHub Copilot CLI", logo: Some("logos/githubcopilot.svg"), color: 0xb392f0 },
    Brand { id: "cursor", name: "Cursor Agent", logo: Some("logos/cursor.svg"), color: 0xe6e6e6 },
    Brand { id: "opencode", name: "OpenCode", logo: Some("logos/opencode.svg"), color: 0xf5a623 },
    Brand { id: "qwen", name: "Qwen Code", logo: Some("logos/qwen.svg"), color: 0x8b87ff },
    Brand { id: "kimi", name: "Kimi CLI", logo: Some("logos/kimi.svg"), color: 0x4a9dff },
    Brand { id: "cline", name: "Cline CLI", logo: Some("logos/cline.svg"), color: 0xd4d4d4 },
    Brand { id: "grok", name: "Grok CLI", logo: Some("logos/x.svg"), color: 0xe5e5e5 },
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
pub fn avatar(id: &str, size: f32) -> gpui::Div {
    let brand = brand(id);
    let inner = size * 0.64;
    // Lighter than every panel background, with a brand-colored rim, so it reads on dark and
    // selected (blue) rows alike.
    let base = div()
        .flex_shrink_0()
        .size(px(size))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(hex(0x333438))
        .border_1()
        .border_color(hex_alpha(if brand.id == "shell" { 0x9a9a9a } else { brand.color }, 0.85));
    match (brand.logo, brand.id) {
        (Some(path), _) => base.child(svg().path(SharedString::from(path)).size(px(inner)).text_color(hex(brand.color))),
        (None, "shell") => base.child(crate::ui::icon("terminal", inner, hex(Chrome::BRIGHT))),
        (None, _) => base.child(
            div()
                .text_size(px((size * 0.52).max(7.)))
                .font_weight(FontWeight::BOLD)
                .text_color(hex(brand.color))
                .child(brand.name.chars().next().unwrap_or('?').to_string()),
        ),
    }
}

/// Overlapping avatars like profile stacks: at most `max` shown, then `+N`.
pub fn avatar_stack(ids: &[&'static str], size: f32, max: usize, ring: u32) -> AnyElement {
    // Absolutely placed: negative margins confuse the row's measured width.
    let shown = ids.len().min(max);
    let step = size * 0.65;
    let more = ids.len() > max;
    let width = size + step * shown.saturating_sub(1) as f32 + if more { size * 0.9 } else { 0. };
    let mut row = div().relative().flex_shrink_0().w(px(width)).h(px(size));
    for (index, id) in ids.iter().take(max).enumerate() {
        // A ring in the row's color separates overlapping avatars without hiding their rim.
        row = row.child(
            div()
                .absolute()
                .top(px(-1.5))
                .left(px(step * index as f32 - 1.5))
                .p(px(1.5))
                .rounded_full()
                .bg(hex(ring))
                .child(avatar(id, size)),
        );
    }
    if ids.len() > max {
        row = row.child(
            div()
                .absolute()
                .top_0()
                .left(px(step * shown as f32))
                .h(px(size))
                .min_w(px(size))
                .px(px(3.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex(0x46474c))
                .border_1()
                .border_color(hex_alpha(0xffffff, 0.35))
                .text_size(px((size * 0.45).max(8.)))
                .text_color(hex(Chrome::BRIGHT))
                .child(format!("+{}", ids.len() - max)),
        );
    }
    row.into_any_element()
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

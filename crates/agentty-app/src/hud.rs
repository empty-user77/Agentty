//! What the AI CLI status bar shows, and in which order (Settings → Appearance → Status bar).
//!
//! Every item can be moved. Items that say what the pane is doing — model, context, status, branch —
//! cannot be hidden; the rest can. The spacer is where the bar splits into a left and a right side.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HudItem {
    Model,
    Context,
    Usage,
    Status,
    Elapsed,
    /// Subagents and session links.
    Links,
    /// Everything after it is pushed to the right edge.
    Spacer,
    /// Buttons plugins add to agent panes.
    Plugins,
    Worktree,
    Branch,
    Folder,
    /// Ports of local servers started in the pane.
    Ports,
}

impl HudItem {
    /// The order a fresh install shows.
    pub const DEFAULT: [HudItem; 12] = [
        Self::Model,
        Self::Context,
        Self::Usage,
        Self::Status,
        Self::Elapsed,
        Self::Links,
        Self::Spacer,
        Self::Ports,
        Self::Plugins,
        Self::Worktree,
        Self::Branch,
        Self::Folder,
    ];

    /// Required items can be moved but not hidden.
    pub fn required(self) -> bool {
        matches!(self, Self::Model | Self::Context | Self::Status | Self::Branch | Self::Spacer)
    }

    /// Hidden until the user turns it on.
    fn opt_in(self) -> bool {
        matches!(self, Self::Ports)
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Model => "hud.model",
            Self::Context => "hud.context",
            Self::Usage => "hud.usage",
            Self::Status => "hud.status",
            Self::Elapsed => "hud.elapsed",
            Self::Links => "hud.links",
            Self::Spacer => "hud.spacer",
            Self::Plugins => "hud.plugins",
            Self::Worktree => "hud.worktree",
            Self::Branch => "hud.branch",
            Self::Folder => "hud.folder",
            Self::Ports => "hud.ports",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HudEntry {
    pub item: HudItem,
    pub visible: bool,
}

/// Reads a saved layout leniently: an entry this version does not know (written by a newer one) is
/// skipped instead of failing the whole settings file, which would reset every setting.
pub fn lenient<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<HudEntry>, D::Error> {
    let raw = Vec::<serde_json::Value>::deserialize(deserializer).unwrap_or_default();
    Ok(raw.into_iter().filter_map(|value| serde_json::from_value(value).ok()).collect())
}

pub fn default_layout() -> Vec<HudEntry> {
    HudItem::DEFAULT.iter().map(|&item| HudEntry { item, visible: !item.opt_in() }).collect()
}

/// A saved layout made whole: each item once, required ones visible, and items this version added
/// put where the default has them (after the item that precedes them there).
pub fn normalized(saved: &[HudEntry]) -> Vec<HudEntry> {
    let mut layout: Vec<HudEntry> = Vec::new();
    for entry in saved {
        if !layout.iter().any(|e| e.item == entry.item) {
            layout.push(HudEntry { item: entry.item, visible: entry.visible || entry.item.required() });
        }
    }
    for (index, &item) in HudItem::DEFAULT.iter().enumerate() {
        if layout.iter().any(|e| e.item == item) {
            continue;
        }
        let after = HudItem::DEFAULT[..index].iter().rev().find_map(|before| layout.iter().position(|e| e.item == *before));
        layout.insert(after.map_or(0, |at| at + 1), HudEntry { item, visible: !item.opt_in() });
    }
    layout
}

/// Moves the item at `index` one place up (`-1`) or down (`1`).
pub fn moved(layout: &[HudEntry], index: usize, by: isize) -> Vec<HudEntry> {
    let mut layout = layout.to_vec();
    let target = index as isize + by;
    if index < layout.len() && target >= 0 && (target as usize) < layout.len() {
        layout.swap(index, target as usize);
    }
    layout
}

/// Shows or hides an item; required ones stay.
pub fn toggled(layout: &[HudEntry], item: HudItem) -> Vec<HudEntry> {
    layout.iter().map(|e| if e.item == item && !item.required() { HudEntry { item, visible: !e.visible } } else { *e }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_items_move_but_never_hide() {
        let layout = default_layout();
        let hidden = toggled(&layout, HudItem::Context);
        assert_eq!(hidden, layout, "context cannot be hidden");
        let hidden = toggled(&layout, HudItem::Folder);
        assert!(!hidden.iter().find(|e| e.item == HudItem::Folder).unwrap().visible);
        // Moving works for required items too, and stops at the ends.
        let first = moved(&layout, 1, -1);
        assert_eq!(first[0].item, HudItem::Context);
        assert_eq!(moved(&layout, 0, -1), layout);
        assert_eq!(moved(&layout, layout.len() - 1, 1), layout);
    }

    #[test]
    fn unknown_items_do_not_fail_the_settings_file() {
        #[derive(Deserialize)]
        struct File {
            #[serde(deserialize_with = "lenient")]
            hud: Vec<HudEntry>,
        }
        let file: File = serde_json::from_str(
            r#"{"hud":[{"item":"branch","visible":true},{"item":"fromTheFuture","visible":true},{"item":"folder","visible":false},"nonsense"]}"#,
        )
        .unwrap();
        assert_eq!(file.hud, vec![HudEntry { item: HudItem::Branch, visible: true }, HudEntry { item: HudItem::Folder, visible: false }]);
    }

    #[test]
    fn saved_layouts_are_made_whole() {
        // An old file: two items, one of them required but saved hidden, and a repeat.
        let saved = [
            HudEntry { item: HudItem::Branch, visible: false },
            HudEntry { item: HudItem::Model, visible: true },
            HudEntry { item: HudItem::Branch, visible: true },
        ];
        let layout = normalized(&saved);
        assert_eq!(layout.len(), HudItem::DEFAULT.len());
        assert_eq!(layout.iter().filter(|e| e.item == HudItem::Branch).count(), 1);
        assert!(layout.iter().all(|e| e.visible || !e.item.required()));
        // The user's order is kept: the branch still comes before the model.
        let at = |item| layout.iter().position(|e| e.item == item).unwrap();
        assert!(at(HudItem::Branch) < at(HudItem::Model));
        // New items follow their neighbour from the default order.
        assert_eq!(at(HudItem::Context), at(HudItem::Model) + 1);
        assert_eq!(normalized(&[]), default_layout());
        assert_eq!(normalized(&default_layout()), default_layout());
    }
}

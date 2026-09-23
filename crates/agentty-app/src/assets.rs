//! Embedded assets: Lucide icons (ISC, see assets/icons/LICENSE.txt), tool logos (Simple Icons, CC0) and brand images.

use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

const FILES: &[(&str, &[u8])] = &[
    ("icons/app-window.svg", include_bytes!("../assets/icons/app-window.svg")),
    ("icons/smartphone.svg", include_bytes!("../assets/icons/smartphone.svg")),
    ("icons/container.svg", include_bytes!("../assets/icons/container.svg")),
    ("icons/git-fork.svg", include_bytes!("../assets/icons/git-fork.svg")),
    ("icons/file.svg", include_bytes!("../assets/icons/file.svg")),
    ("icons/lock.svg", include_bytes!("../assets/icons/lock.svg")),
    ("icons/graduation-cap.svg", include_bytes!("../assets/icons/graduation-cap.svg")),
    ("icons/x-twitter.svg", include_bytes!("../assets/icons/x-twitter.svg")),
    ("icons/minus.svg", include_bytes!("../assets/icons/minus.svg")),
    ("icons/key-round.svg", include_bytes!("../assets/icons/key-round.svg")),
    ("icons/puzzle.svg", include_bytes!("../assets/icons/puzzle.svg")),
    ("icons/notebook-pen.svg", include_bytes!("../assets/icons/notebook-pen.svg")),
    ("icons/send.svg", include_bytes!("../assets/icons/send.svg")),
    ("icons/save.svg", include_bytes!("../assets/icons/save.svg")),
    ("icons/file-input.svg", include_bytes!("../assets/icons/file-input.svg")),
    ("icons/trash-2.svg", include_bytes!("../assets/icons/trash-2.svg")),
    ("icons/download.svg", include_bytes!("../assets/icons/download.svg")),
    ("icons/upload.svg", include_bytes!("../assets/icons/upload.svg")),
    ("icons/power.svg", include_bytes!("../assets/icons/power.svg")),
    ("icons/scroll-text.svg", include_bytes!("../assets/icons/scroll-text.svg")),
    ("icons/zap.svg", include_bytes!("../assets/icons/zap.svg")),
    ("icons/wand-sparkles.svg", include_bytes!("../assets/icons/wand-sparkles.svg")),
    ("icons/code.svg", include_bytes!("../assets/icons/code.svg")),
    ("icons/file-plus.svg", include_bytes!("../assets/icons/file-plus.svg")),
    ("icons/list.svg", include_bytes!("../assets/icons/list.svg")),
    ("icons/message-square.svg", include_bytes!("../assets/icons/message-square.svg")),
    ("icons/notebook.svg", include_bytes!("../assets/icons/notebook.svg")),
    ("icons/sticky-note.svg", include_bytes!("../assets/icons/sticky-note.svg")),
    ("icons/bookmark.svg", include_bytes!("../assets/icons/bookmark.svg")),
    ("icons/calendar.svg", include_bytes!("../assets/icons/calendar.svg")),
    ("icons/tag.svg", include_bytes!("../assets/icons/tag.svg")),
    ("icons/clipboard.svg", include_bytes!("../assets/icons/clipboard.svg")),
    ("icons/clipboard-paste.svg", include_bytes!("../assets/icons/clipboard-paste.svg")),
    ("icons/git-pull-request.svg", include_bytes!("../assets/icons/git-pull-request.svg")),
    ("icons/bug.svg", include_bytes!("../assets/icons/bug.svg")),
    ("icons/rocket.svg", include_bytes!("../assets/icons/rocket.svg")),
    ("icons/book-open.svg", include_bytes!("../assets/icons/book-open.svg")),
    ("icons/hammer.svg", include_bytes!("../assets/icons/hammer.svg")),
    ("icons/wrench.svg", include_bytes!("../assets/icons/wrench.svg")),
    ("icons/database.svg", include_bytes!("../assets/icons/database.svg")),
    ("icons/cloud.svg", include_bytes!("../assets/icons/cloud.svg")),
    ("icons/eye.svg", include_bytes!("../assets/icons/eye.svg")),
    ("icons/play.svg", include_bytes!("../assets/icons/play.svg")),
    ("icons/square.svg", include_bytes!("../assets/icons/square.svg")),
    ("icons/lightbulb.svg", include_bytes!("../assets/icons/lightbulb.svg")),
    ("icons/plug.svg", include_bytes!("../assets/icons/plug.svg")),
    ("icons/hash.svg", include_bytes!("../assets/icons/hash.svg")),
    ("icons/house.svg", include_bytes!("../assets/icons/house.svg")),
    ("icons/at-sign.svg", include_bytes!("../assets/icons/at-sign.svg")),
    ("icons/mail.svg", include_bytes!("../assets/icons/mail.svg")),
    ("icons/image.svg", include_bytes!("../assets/icons/image.svg")),
    ("icons/arrow-down.svg", include_bytes!("../assets/icons/arrow-down.svg")),
    ("icons/arrow-left.svg", include_bytes!("../assets/icons/arrow-left.svg")),
    ("icons/arrow-right.svg", include_bytes!("../assets/icons/arrow-right.svg")),
    ("icons/arrow-up.svg", include_bytes!("../assets/icons/arrow-up.svg")),
    ("icons/arrow-up-right.svg", include_bytes!("../assets/icons/arrow-up-right.svg")),
    ("icons/bell.svg", include_bytes!("../assets/icons/bell.svg")),
    ("icons/bell-dot.svg", include_bytes!("../assets/icons/bell-dot.svg")),
    ("icons/blocks.svg", include_bytes!("../assets/icons/blocks.svg")),
    ("icons/brain.svg", include_bytes!("../assets/icons/brain.svg")),
    ("icons/bot.svg", include_bytes!("../assets/icons/bot.svg")),
    ("icons/chart-column.svg", include_bytes!("../assets/icons/chart-column.svg")),
    ("icons/check.svg", include_bytes!("../assets/icons/check.svg")),
    ("icons/chevron-down.svg", include_bytes!("../assets/icons/chevron-down.svg")),
    ("icons/chevron-right.svg", include_bytes!("../assets/icons/chevron-right.svg")),
    ("icons/chevron-up.svg", include_bytes!("../assets/icons/chevron-up.svg")),
    ("icons/circle-check.svg", include_bytes!("../assets/icons/circle-check.svg")),
    ("icons/circle-dot.svg", include_bytes!("../assets/icons/circle-dot.svg")),
    ("icons/circle-pause.svg", include_bytes!("../assets/icons/circle-pause.svg")),
    ("icons/circle-x.svg", include_bytes!("../assets/icons/circle-x.svg")),
    ("icons/clock.svg", include_bytes!("../assets/icons/clock.svg")),
    ("icons/columns-2.svg", include_bytes!("../assets/icons/columns-2.svg")),
    ("icons/command.svg", include_bytes!("../assets/icons/command.svg")),
    ("icons/copy.svg", include_bytes!("../assets/icons/copy.svg")),
    ("icons/ellipsis.svg", include_bytes!("../assets/icons/ellipsis.svg")),
    ("icons/external-link.svg", include_bytes!("../assets/icons/external-link.svg")),
    ("icons/file-text.svg", include_bytes!("../assets/icons/file-text.svg")),
    ("icons/folder.svg", include_bytes!("../assets/icons/folder.svg")),
    ("icons/folder-open.svg", include_bytes!("../assets/icons/folder-open.svg")),
    ("icons/folder-plus.svg", include_bytes!("../assets/icons/folder-plus.svg")),
    ("icons/git-branch.svg", include_bytes!("../assets/icons/git-branch.svg")),
    ("icons/git-commit-horizontal.svg", include_bytes!("../assets/icons/git-commit-horizontal.svg")),
    ("icons/globe.svg", include_bytes!("../assets/icons/globe.svg")),
    ("icons/grip-vertical.svg", include_bytes!("../assets/icons/grip-vertical.svg")),
    ("icons/history.svg", include_bytes!("../assets/icons/history.svg")),
    ("icons/info.svg", include_bytes!("../assets/icons/info.svg")),
    ("icons/layout-panel-left.svg", include_bytes!("../assets/icons/layout-panel-left.svg")),
    ("icons/link.svg", include_bytes!("../assets/icons/link.svg")),
    ("icons/list-tree.svg", include_bytes!("../assets/icons/list-tree.svg")),
    ("icons/loader-circle.svg", include_bytes!("../assets/icons/loader-circle.svg")),
    ("icons/maximize-2.svg", include_bytes!("../assets/icons/maximize-2.svg")),
    ("icons/message-circle-question.svg", include_bytes!("../assets/icons/message-circle-question.svg")),
    ("icons/minimize-2.svg", include_bytes!("../assets/icons/minimize-2.svg")),
    ("icons/network.svg", include_bytes!("../assets/icons/network.svg")),
    ("icons/panel-left-close.svg", include_bytes!("../assets/icons/panel-left-close.svg")),
    ("icons/panel-left-open.svg", include_bytes!("../assets/icons/panel-left-open.svg")),
    ("icons/package.svg", include_bytes!("../assets/icons/package.svg")),
    ("icons/fold-vertical.svg", include_bytes!("../assets/icons/fold-vertical.svg")),
    ("icons/pencil.svg", include_bytes!("../assets/icons/pencil.svg")),
    ("icons/picture-in-picture-2.svg", include_bytes!("../assets/icons/picture-in-picture-2.svg")),
    ("icons/plus.svg", include_bytes!("../assets/icons/plus.svg")),
    ("icons/refresh-cw.svg", include_bytes!("../assets/icons/refresh-cw.svg")),
    ("icons/rotate-cw.svg", include_bytes!("../assets/icons/rotate-cw.svg")),
    ("icons/rows-2.svg", include_bytes!("../assets/icons/rows-2.svg")),
    ("icons/search.svg", include_bytes!("../assets/icons/search.svg")),
    ("icons/settings.svg", include_bytes!("../assets/icons/settings.svg")),
    ("icons/shield-alert.svg", include_bytes!("../assets/icons/shield-alert.svg")),
    ("icons/sparkles.svg", include_bytes!("../assets/icons/sparkles.svg")),
    ("icons/square-plus.svg", include_bytes!("../assets/icons/square-plus.svg")),
    ("icons/square-terminal.svg", include_bytes!("../assets/icons/square-terminal.svg")),
    ("icons/star.svg", include_bytes!("../assets/icons/star.svg")),
    ("icons/terminal.svg", include_bytes!("../assets/icons/terminal.svg")),
    ("icons/undo-2.svg", include_bytes!("../assets/icons/undo-2.svg")),
    ("icons/unlink.svg", include_bytes!("../assets/icons/unlink.svg")),
    ("icons/users.svg", include_bytes!("../assets/icons/users.svg")),
    ("icons/workflow.svg", include_bytes!("../assets/icons/workflow.svg")),
    ("icons/x.svg", include_bytes!("../assets/icons/x.svg")),
    ("logos/antigravity.svg", include_bytes!("../assets/logos/antigravity.svg")),
    ("logos/claude.svg", include_bytes!("../assets/logos/claude.svg")),
    ("logos/cline.svg", include_bytes!("../assets/logos/cline.svg")),
    ("logos/cursor.svg", include_bytes!("../assets/logos/cursor.svg")),
    ("logos/githubcopilot.svg", include_bytes!("../assets/logos/githubcopilot.svg")),
    ("logos/googlegemini.svg", include_bytes!("../assets/logos/googlegemini.svg")),
    ("logos/kimi.svg", include_bytes!("../assets/logos/kimi.svg")),
    ("logos/ollama.svg", include_bytes!("../assets/logos/ollama.svg")),
    ("logos/openai.svg", include_bytes!("../assets/logos/openai.svg")),
    ("logos/opencode.svg", include_bytes!("../assets/logos/opencode.svg")),
    ("logos/qwen.svg", include_bytes!("../assets/logos/qwen.svg")),
    ("logos/sourcegraph.svg", include_bytes!("../assets/logos/sourcegraph.svg")),
    ("logos/x.svg", include_bytes!("../assets/logos/x.svg")),
    ("brand/logo.png", include_bytes!("../assets/brand/logo-256.png")),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES.iter().find(|(name, _)| *name == path).map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(FILES.iter().filter(|(name, _)| name.starts_with(path)).map(|(name, _)| SharedString::from(*name)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plugin authors pick icons from the list in the plugin guide.
    #[test]
    fn plugin_guide_lists_every_icon() {
        let guide = agentty_bridge::plugins::store::GUIDE;
        let listed = guide.lines().find(|l| l.starts_with('`') && l.contains("puzzle")).unwrap_or_default();
        let names: Vec<&str> = listed.trim_matches(|c| c == '`' || c == '.').split_whitespace().collect();
        for name in crate::ui::ICONS {
            assert!(names.contains(name), "docs/plugins/README.md does not list icon {name}");
        }
        for name in names {
            assert!(crate::ui::ICONS.contains(&name), "docs/plugins/README.md lists unknown icon {name}");
        }
    }

    #[test]
    fn every_icon_used_in_code_exists() {
        for name in crate::ui::ICONS {
            assert!(Assets.load(&format!("icons/{name}.svg")).unwrap().is_some(), "missing icon {name}");
        }
        for path in crate::brand::logo_paths() {
            assert!(Assets.load(path).unwrap().is_some(), "missing logo {path}");
        }
    }
}

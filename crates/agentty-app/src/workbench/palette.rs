//! Command palette (⇧⌘P): actions, workspaces, themes and custom commands from `agentty.json`.
//!
//! Custom commands (project `agentty.json`, found by walking up from the active pane's directory,
//! plus `<data dir>/commands.json`):
//!
//! ```json
//! { "commands": [ { "name": "Dev server", "command": "npm run dev", "target": "split" } ] }
//! ```
//! `target` is `tab` (default), `split`, `splitDown`, `workspace` or `current` (typed into the active pane).

use super::{LaunchTarget, Page, SidePanel, Workbench};
use crate::i18n::t;
use crate::launch::{home_dir, LaunchSpec, PaneKind};
use crate::settings::{update_settings, SettingsStore};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::hint;
use crate::ui::TypeScale;
use gpui::{div, prelude::*, px, ClickEvent, Context, Entity, Focusable, FontWeight, Subscription, Window};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::rc::Rc;

type Run = Rc<dyn Fn(&mut Workbench, &mut Window, &mut Context<Workbench>)>;

#[derive(Clone)]
struct Item {
    label: String,
    category: String,
    hint: Option<String>,
    run: Run,
}

pub struct Palette {
    input: Entity<TextInput>,
    items: Vec<Item>,
    selected: usize,
    _subscription: Subscription,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomCommand {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub target: CommandTarget,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CommandTarget {
    #[default]
    Tab,
    Split,
    SplitDown,
    Workspace,
    Current,
}

#[derive(Debug, Default, Deserialize)]
struct CommandFile {
    #[serde(default)]
    commands: Vec<CustomCommand>,
}

/// Project commands (nearest `agentty.json` up to the home directory) and global commands.
pub fn custom_commands(start: Option<&Path>) -> Vec<(CustomCommand, PathBuf)> {
    let mut out = Vec::new();
    let home = home_dir();
    let mut dir = start;
    while let Some(current) = dir {
        let file = current.join("agentty.json");
        if let Some(parsed) = read_commands(&file) {
            out.extend(parsed.commands.into_iter().map(|c| (c, current.to_path_buf())));
            break;
        }
        if current == home {
            break;
        }
        dir = current.parent();
    }
    if let Some(parsed) = read_commands(&agentty_bridge::fsutil::data_dir().join("commands.json")) {
        out.extend(parsed.commands.into_iter().map(|c| (c, start.map(Path::to_path_buf).unwrap_or_else(home_dir))));
    }
    out
}

fn read_commands(path: &Path) -> Option<CommandFile> {
    std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

/// Case-insensitive subsequence match; higher is better. Rewards contiguous runs and word starts.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut position = 0;
    let mut previous: Option<usize> = None;
    for q in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let found = (position..text.len()).find(|&i| text[i] == q)?;
        score += 1;
        if previous == Some(found.wrapping_sub(1)) {
            score += 4;
        }
        if found == 0 || matches!(text[found - 1], ' ' | '-' | '_' | '/' | ':' | '.') {
            score += 3;
        }
        score -= (found - position).min(5) as i32;
        previous = Some(found);
        position = found + 1;
    }
    Some(score)
}

impl Workbench {
    pub(super) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.palette_items(cx);
        let input = cx.new(|cx| TextInput::localized("", "palette.placeholder", window, cx));
        let subscription = cx.subscribe_in(&input, window, |this, input, event: &TextInputEvent, window, cx| {
            let query = input.read(cx).text().to_string();
            let Some(palette) = this.palette.as_mut() else { return };
            let count = filtered(&palette.items, &query).len();
            match event {
                TextInputEvent::Changed => palette.selected = 0,
                TextInputEvent::Up => palette.selected = palette.selected.saturating_sub(1),
                TextInputEvent::Down => palette.selected = (palette.selected + 1).min(count.saturating_sub(1)),
                TextInputEvent::Confirmed => {
                    let chosen = filtered(&palette.items, &query).get(palette.selected).map(|item| item.run.clone());
                    this.close_palette(window, cx);
                    if let Some(run) = chosen {
                        run(this, window, cx);
                    }
                    return;
                }
                TextInputEvent::Cancelled => return this.close_palette(window, cx),
                TextInputEvent::Blurred | TextInputEvent::PastedLines(_) => {}
            }
            cx.notify();
        });
        window.focus(&input.focus_handle(cx));
        self.palette = Some(Palette { input, items, selected: 0, _subscription: subscription });
        self.launcher_open = false;
        self.notices_open = false;
        cx.notify();
    }

    pub(super) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn palette_items(&self, cx: &mut Context<Self>) -> Vec<Item> {
        let mut items: Vec<Item> = Vec::new();
        let mut push = |label: &str, category: &str, hint: Option<&str>, run: Run| {
            // Shortcut hints are written for macOS; shown as this platform's keys.
            let hint = hint.map(|h| crate::keymap::display(h).into_owned());
            items.push(Item { label: label.to_string(), category: category.to_string(), hint, run });
        };

        let project = self.active_pane().map(|p| p.read(cx).display_cwd());
        for (command, dir) in custom_commands(project.as_deref()) {
            let label = command.name.clone();
            let detail = command.command.clone();
            push(
                &label,
                t(cx, "palette.custom"),
                Some(&detail),
                Rc::new(move |this, window, cx| this.run_custom_command(&command, &dir, window, cx)),
            );
        }

        let general = t(cx, "palette.general");
        let launch = |kind: PaneKind, target: LaunchTarget| -> Run {
            Rc::new(move |this, window, cx| this.request_launch(kind, target, window, cx))
        };
        push(t(cx, "new.terminal"), general, Some("⌘T"), launch(PaneKind::Shell, LaunchTarget::NewTab));
        push(t(cx, "new.claude"), general, Some("⌥⌘C"), launch(PaneKind::Claude, LaunchTarget::NewTab));
        push(t(cx, "new.codex"), general, Some("⌥⌘X"), launch(PaneKind::Codex, LaunchTarget::NewTab));
        push(t(cx, "new.workspace"), general, Some("⌘N"), launch(PaneKind::Shell, LaunchTarget::NewWorkspace));
        push(t(cx, "split.right"), general, Some("⌘D"), Rc::new(|this, window, cx| this.split(super::Axis::Horizontal, window, cx)));
        push(t(cx, "split.down"), general, Some("⇧⌘D"), Rc::new(|this, window, cx| this.split(super::Axis::Vertical, window, cx)));
        push(t(cx, "notice.jump"), general, Some("⇧⌘U"), Rc::new(|this, window, cx| this.jump_to_unread(window, cx)));
        if self.active_pane().is_some_and(|p| p.read(cx).advisor().is_some()) {
            for choice in crate::settings::AdvisorChoice::ALL {
                let label = format!("{}: {}", t(cx, "advisor.palette"), t(cx, choice.label_key()));
                push(&label, general, None, Rc::new(move |this, _, cx| this.set_pane_advisor(choice, cx)));
            }
        }
        push(
            t(cx, "new.group"),
            general,
            None,
            Rc::new(|this, window, cx| {
                this.create_group(window, cx);
            }),
        );
        for (page, key, shortcut) in [
            (Page::Flow, "page.flow", "⇧⌘F"),
            (Page::Usage, "page.monitoring", "⌥⌘U"),
            (Page::Processes, "page.processes", ""),
            (Page::Proxy, "page.proxy", ""),
            (Page::Worktrees, "page.worktrees", ""),
            (Page::Disk, "page.disk", ""),
            (Page::Extensions, "page.extensions", "⇧⌘X"),
            (Page::Plugins, "page.plugins", ""),
            (Page::Idea, "page.idea", ""),
            (Page::Database, "page.database", ""),
            (Page::Settings, "page.settings", "⌘,"),
        ] {
            if page == Page::Idea && !crate::settings::settings(cx).idea_mode {
                continue;
            }
            // Only for a project with databases configured.
            if page == Page::Database && self.db.connections.is_empty() {
                continue;
            }
            push(
                t(cx, key),
                general,
                Some(shortcut),
                Rc::new(move |this, _, cx| {
                    this.page = Some(page);
                    cx.notify();
                }),
            );
        }
        push(t(cx, "panel.sessions"), general, Some("⇧⌘S"), Rc::new(|this, _, cx| this.show_panel(SidePanel::Sessions, cx)));
        push(t(cx, "files.title"), general, Some("⌥⌘B"), Rc::new(|this, _, cx| this.toggle_files_panel(cx)));
        push(t(cx, "docker.title"), general, None, Rc::new(|this, _, cx| this.toggle_docker_panel(cx)));
        push(t(cx, "onboarding.show_again"), general, None, Rc::new(|this, _, cx| this.open_onboarding(cx)));
        push(
            t(cx, "menu.toggle_sidebar"),
            general,
            Some("⌘B"),
            Rc::new(|this, _, cx| {
                this.sidebar_open = !this.sidebar_open;
                cx.notify();
            }),
        );

        // Plugin commands (and panels) of enabled plugins.
        let plugin_category = t(cx, "palette.plugin");
        let mut plugin_items: Vec<(String, Option<String>, Run)> = Vec::new();
        for (plugin, manifest) in crate::plugins::active(cx) {
            if let Some(panel) = &manifest.contributes.panel {
                let id = plugin.id.clone();
                let label = crate::i18n::tf(cx, "plugins.palette_panel", &[("name", &panel.title)]);
                plugin_items.push((label, None, Rc::new(move |this, _, cx| this.open_plugin_panel(&id, cx))));
            }
            for command in manifest.contributes.commands.iter().filter(|c| c.palette) {
                let (id, command_id) = (plugin.id.clone(), command.id.clone());
                plugin_items.push((
                    command.title.clone(),
                    Some(manifest.name.clone()),
                    Rc::new(move |this, _, cx| this.run_plugin_command(&id, &command_id, None, cx)),
                ));
            }
        }
        for (label, detail, run) in plugin_items {
            push(&label, plugin_category, detail.as_deref(), run);
        }

        let workspace_category = t(cx, "palette.workspace");
        for (index, ws) in self.workspaces.iter().enumerate() {
            let title = self.workspace_title(ws, cx);
            push(
                &title,
                workspace_category,
                Some(&crate::ui::tilde(&ws.cwd)),
                Rc::new(move |this, window, cx| this.activate_workspace(index, window, cx)),
            );
        }

        let theme_category = t(cx, "settings.theme");
        let themes: Vec<String> = cx.global::<SettingsStore>().themes.iter().map(|t| t.name.clone()).collect();
        for name in themes {
            let chosen = name.clone();
            push(
                &name,
                theme_category,
                None,
                Rc::new(move |_, _, cx| {
                    let chosen = chosen.clone();
                    update_settings(cx, move |s| s.theme = chosen);
                }),
            );
        }
        items
    }

    fn run_custom_command(&mut self, command: &CustomCommand, dir: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let spec = LaunchSpec::shell_command(command.command.clone(), command.name.clone(), dir.to_path_buf());
        match command.target {
            CommandTarget::Tab => self.open_tab(spec, window, cx),
            CommandTarget::Workspace => self.create_workspace(spec, window, cx),
            CommandTarget::Split | CommandTarget::SplitDown => {
                let axis = if command.target == CommandTarget::Split { super::Axis::Horizontal } else { super::Axis::Vertical };
                self.split_with(spec, axis, window, cx);
            }
            CommandTarget::Current => {
                if let Some(pane) = self.active_pane() {
                    let line = format!("{}\r", command.command);
                    pane.update(cx, |view, _| view.write(line.into_bytes()));
                    self.focus_pane(&pane, window, cx);
                }
            }
        }
    }

    pub(super) fn render_palette(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(palette) = self.palette.as_ref() else { return div().into_any_element() };
        let query = palette.input.read(cx).text().to_string();
        let visible = filtered(&palette.items, &query);
        let mut list = div().id("palette-list").flex().flex_col().max_h(px(420.)).overflow_y_scroll().p_1();
        if visible.is_empty() {
            list = list.child(hint(t(cx, "ext.empty")));
        }
        for (index, item) in visible.into_iter().take(80).enumerate() {
            let selected = index == palette.selected;
            let run = item.run.clone();
            list = list.child(
                div()
                    .id(("palette-item", index))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if selected { hex(Chrome::ACCENT) } else { hex_alpha(0, 0.) })
                    .hover(|s| s.bg(hex(if selected { Chrome::ACCENT } else { Chrome::HOVER })))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.close_palette(window, cx);
                        run(this, window, cx);
                    }))
                    .child(
                        div()
                            .w(px(84.))
                            .flex_shrink_0()
                            .truncate()
                            .t_small()
                            .text_color(if selected { hex(0xdddddd) } else { hex(Chrome::MUTED) })
                            .child(item.category.clone()),
                    )
                    .child(div().flex_1().min_w_0().truncate().t_body().text_color(hex(Chrome::BRIGHT)).child(item.label.clone()))
                    .children(item.hint.clone().map(|h| {
                        div()
                            .flex_shrink_0()
                            .max_w(px(220.))
                            .truncate()
                            .t_small()
                            .text_color(if selected { hex(0xdddddd) } else { hex(Chrome::MUTED) })
                            .child(h)
                    })),
            );
        }

        div()
            .id("palette-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(70.))
            .bg(hex_alpha(0x000000, 0.3))
            .occlude()
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.close_palette(window, cx)))
            .child(
                div()
                    .id("palette")
                    .w(px(620.))
                    .flex()
                    .flex_col()
                    .bg(hex(Chrome::OVERLAY))
                    .border_1()
                    .border_color(hex(Chrome::OVERLAY_BORDER))
                    .rounded_lg()
                    .shadow_lg()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .m_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(hex(Chrome::ACCENT))
                            .bg(hex(0x1e1e1e))
                            .t_body()
                            .text_color(hex(Chrome::BRIGHT))
                            .child(palette.input.clone()),
                    )
                    .child(list)
                    .child(
                        div()
                            .px_3()
                            .py_1p5()
                            .border_t_1()
                            .border_color(hex(Chrome::BORDER))
                            .t_small()
                            .text_color(hex(Chrome::MUTED))
                            .font_weight(FontWeight::NORMAL)
                            .child(t(cx, "palette.footer")),
                    ),
            )
            .into_any_element()
    }
}

fn filtered(items: &[Item], query: &str) -> Vec<Item> {
    let mut scored: Vec<(i32, usize, &Item)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let label = fuzzy_score(query, &item.label);
            let category = fuzzy_score(query, &format!("{} {}", item.category, item.label)).map(|s| s - 2);
            label.max(category).map(|score| (score, i, item))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, item)| item.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_prefers_word_starts_and_runs() {
        assert!(fuzzy_score("spr", "Split Right").is_some());
        assert!(fuzzy_score("xyz", "Split Right").is_none());
        let word_start = fuzzy_score("nt", "New Terminal").unwrap();
        let scattered = fuzzy_score("nt", "Cancel it").unwrap();
        assert!(word_start > scattered);
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }

    #[test]
    fn finds_project_commands() {
        let dir = std::env::temp_dir().join(format!("agentty-cmds-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src/deep")).unwrap();
        std::fs::write(dir.join("agentty.json"), r#"{"commands":[{"name":"Dev","command":"npm run dev","target":"split"}]}"#).unwrap();
        let found = custom_commands(Some(&dir.join("src/deep")));
        let (command, root) = found.iter().find(|(c, _)| c.name == "Dev").unwrap();
        assert_eq!(command.target, CommandTarget::Split);
        assert_eq!(root, &dir);
        std::fs::remove_dir_all(dir).ok();
    }
}

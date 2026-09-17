//! "Where should this start?" folder browser shown before opening a new tab or workspace.
//! Shortcuts (home, current pane, recent, session folders) → click to enter a folder → browse
//! subfolders → "Select" starts there. A new folder can be created and started in directly.

use super::{LaunchTarget, Workbench};
use crate::i18n::{t, tf};
use crate::launch::{home_dir, PaneKind};
use crate::settings::settings;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{action_button, hint, icon, icon_only, tilde, IconSize, TypeScale as _};
use gpui::{div, prelude::*, px, ClickEvent, Context, Entity, Focusable, FontWeight, PathPromptOptions, Subscription, Window};
use std::path::{Path, PathBuf};

const MAX_SHORTCUTS: usize = 14;
const MAX_CHILDREN: usize = 500;

pub struct Picker {
    choice: crate::launch::LaunchChoice,
    target: LaunchTarget,
    input: Entity<TextInput>,
    shortcuts: Vec<(PathBuf, &'static str)>,
    /// Folder being browsed; `None` shows the shortcut list.
    current: Option<PathBuf>,
    children: Vec<PathBuf>,
    selected: usize,
    error: Option<&'static str>,
    new_folder: Option<(Entity<TextInput>, Subscription)>,
    _subscription: Subscription,
}

/// Agent tool sandboxes and other scratch directories aren't useful starting points.
fn is_temporary(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.starts_with("/private/var/folders")
        || text.starts_with("/var/folders")
        || text.starts_with("/tmp")
        || text.starts_with("/private/tmp")
}

fn expand(input: &str) -> Option<PathBuf> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    Some(match input.strip_prefix('~') {
        Some(rest) => home_dir().join(rest.trim_start_matches('/')),
        None => PathBuf::from(input),
    })
}

fn looks_like_path(input: &str) -> bool {
    let input = input.trim();
    input.starts_with('/') || input.starts_with('~')
}

/// Visible subfolders, sorted case-insensitively (hidden folders excluded).
fn list_children(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .filter(|e| e.path().is_dir())
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    dirs.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
    dirs.truncate(MAX_CHILDREN);
    dirs
}

/// Folder names users can create: no separators, no traversal, no control characters.
fn valid_folder_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.chars().any(char::is_control) && name.len() <= 255
}

impl Picker {
    pub fn add_session_dirs(&mut self, sessions: &[agentty_bridge::model::SessionInfo]) {
        for cwd in sessions.iter().take(40).filter_map(|s| s.cwd.as_ref()) {
            if self.shortcuts.len() >= MAX_SHORTCUTS {
                break;
            }
            let path = PathBuf::from(cwd);
            if !is_temporary(&path) && path.is_dir() && !self.shortcuts.iter().any(|(p, _)| p == &path) {
                self.shortcuts.push((path, "picker.session"));
            }
        }
    }

    /// Rows currently shown: shortcuts, or subfolders of the folder being browsed.
    fn rows(&self, query: &str) -> Vec<(PathBuf, &'static str)> {
        let query = if looks_like_path(query) { String::new() } else { query.trim().to_lowercase() };
        let matches = |path: &Path| {
            query.is_empty()
                || path.file_name().map(|n| n.to_string_lossy().to_lowercase().contains(&query)).unwrap_or(false)
                || (self.current.is_none() && path.to_string_lossy().to_lowercase().contains(&query))
        };
        match &self.current {
            None => self.shortcuts.iter().filter(|(p, _)| matches(p)).cloned().collect(),
            Some(_) => self.children.iter().filter(|p| matches(p)).map(|p| (p.clone(), "")).collect(),
        }
    }

    fn navigate(&mut self, dir: PathBuf, cx: &mut Context<Workbench>) {
        self.children = list_children(&dir);
        self.current = Some(dir);
        self.selected = 0;
        self.error = None;
        self.input.update(cx, |input, cx| input.set_text("", cx));
    }
}

impl Workbench {
    pub(super) fn open_picker(
        &mut self,
        choice: crate::launch::LaunchChoice,
        target: LaunchTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Home first (the default), then where the user is, recent folders and session folders.
        let mut shortcuts: Vec<(PathBuf, &'static str)> = vec![(home_dir(), "picker.home")];
        let push = |path: PathBuf, label: &'static str, list: &mut Vec<(PathBuf, &'static str)>| {
            if path.is_dir() && !list.iter().any(|(p, _)| p == &path) {
                list.push((path, label));
            }
        };
        if let Some(pane) = self.active_pane() {
            push(pane.read(cx).current_dir(), "picker.current", &mut shortcuts);
        }
        for dir in settings(cx).recent_dirs.clone() {
            push(dir, "picker.recent", &mut shortcuts);
        }
        shortcuts.truncate(MAX_SHORTCUTS);

        let input = cx.new(|cx| TextInput::localized("", "picker.placeholder", window, cx));
        let subscription = cx.subscribe_in(&input, window, |this, input, event: &TextInputEvent, window, cx| {
            let query = input.read(cx).text().to_string();
            let Some(picker) = this.picker.as_mut() else { return };
            let count = picker.rows(&query).len();
            match event {
                TextInputEvent::Changed => {
                    picker.selected = 0;
                    picker.error = None;
                }
                TextInputEvent::Up => picker.selected = picker.selected.saturating_sub(1),
                TextInputEvent::Down => picker.selected = (picker.selected + 1).min(count.saturating_sub(1)),
                TextInputEvent::Confirmed => {
                    // Enter opens a typed path or the highlighted folder; "Select" starts.
                    let typed = looks_like_path(&query).then(|| expand(&query)).flatten();
                    let target = typed.or_else(|| picker.rows(&query).get(picker.selected).map(|(p, _)| p.clone()));
                    match target.filter(|p| p.is_dir()) {
                        Some(dir) => picker.navigate(dir, cx),
                        None => picker.error = Some("picker.invalid"),
                    }
                }
                TextInputEvent::Cancelled => return this.close_picker(window, cx),
                TextInputEvent::Blurred => {}
            }
            cx.notify();
        });
        window.focus(&input.focus_handle(cx));
        let mut picker = Picker {
            choice,
            target,
            input,
            shortcuts,
            current: None,
            children: Vec::new(),
            selected: 0,
            error: None,
            new_folder: None,
            _subscription: subscription,
        };
        picker.add_session_dirs(&self.sessions);
        self.picker = Some(picker);
        if self.sessions.is_empty() {
            self.refresh_sessions(cx);
        }
        cx.notify();
    }

    pub(super) fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Starts in the folder being browsed, or the highlighted shortcut.
    fn select_in_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.as_ref() else { return };
        let query = picker.input.read(cx).text().to_string();
        let dir = match &picker.current {
            Some(dir) => Some(dir.clone()),
            None => looks_like_path(&query)
                .then(|| expand(&query))
                .flatten()
                .or_else(|| picker.rows(&query).get(picker.selected).map(|(p, _)| p.clone())),
        };
        match dir.filter(|d| d.is_dir()) {
            Some(dir) => self.start_from_picker(dir, window, cx),
            None => {
                if let Some(picker) = self.picker.as_mut() {
                    picker.error = Some("picker.invalid");
                }
                cx.notify();
            }
        }
    }

    fn start_from_picker(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.take() else { return };
        self.launch(picker.choice.clone(), picker.target, dir, window, cx);
    }

    fn picker_up(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.as_mut() else { return };
        match picker.current.as_ref().and_then(|c| c.parent().map(Path::to_path_buf)) {
            Some(parent) => picker.navigate(parent, cx),
            None => {
                picker.current = None;
                picker.selected = 0;
            }
        }
        cx.notify();
    }

    fn picker_shortcuts(&mut self, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker.as_mut() {
            picker.current = None;
            picker.selected = 0;
            picker.new_folder = None;
        }
        cx.notify();
    }

    fn begin_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.as_mut() else { return };
        if picker.current.is_none() {
            let base = picker.shortcuts.get(picker.selected).map(|(p, _)| p.clone()).unwrap_or_else(home_dir);
            picker.navigate(base, cx);
        }
        let input = cx.new(|cx| TextInput::localized("", "picker.new_folder_name", window, cx));
        let subscription = cx.subscribe_in(&input, window, |this, input, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Confirmed => {
                let name = input.read(cx).text().to_string();
                this.create_folder_and_start(&name, window, cx);
            }
            TextInputEvent::Cancelled => {
                if let Some(picker) = this.picker.as_mut() {
                    picker.new_folder = None;
                    window.focus(&picker.input.focus_handle(cx));
                }
                cx.notify();
            }
            _ => {}
        });
        window.focus(&input.focus_handle(cx));
        if let Some(picker) = self.picker.as_mut() {
            picker.new_folder = Some((input, subscription));
        }
        cx.notify();
    }

    fn create_folder_and_start(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.as_mut() else { return };
        let Some(parent) = picker.current.clone() else { return };
        if !valid_folder_name(name) {
            picker.error = Some("picker.bad_folder_name");
            return cx.notify();
        }
        let dir = parent.join(name.trim());
        if dir.exists() {
            picker.error = Some("picker.folder_exists");
            return cx.notify();
        }
        match std::fs::create_dir(&dir) {
            Ok(()) => self.start_from_picker(dir, window, cx),
            Err(_) => {
                picker.error = Some("picker.folder_failed");
                cx.notify();
            }
        }
    }

    fn browse_folder(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = paths.await {
                if let Some(path) = paths.pop() {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(picker) = this.picker.as_mut() {
                            picker.navigate(path, cx);
                        }
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub(super) fn render_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(picker) = self.picker.as_ref() else { return div().into_any_element() };
        let title = match (picker.choice.label(), picker.choice.kind()) {
            (Some(label), _) => tf(cx, "picker.title.other", &[("name", &label)]),
            (None, PaneKind::Shell) => t(cx, "picker.title.shell").to_string(),
            (None, PaneKind::Claude) => t(cx, "picker.title.claude").to_string(),
            (None, PaneKind::Codex) => t(cx, "picker.title.codex").to_string(),
        };
        let query = picker.input.read(cx).text().to_string();
        let rows = picker.rows(&query);

        let location = div()
            .mx_3()
            .flex()
            .items_center()
            .gap_1()
            .child(icon_only("picker-shortcuts", "history", cx.listener(|this, _: &ClickEvent, _, cx| this.picker_shortcuts(cx))))
            .when(picker.current.is_some(), |d| {
                d.child(icon_only("picker-up", "chevron-up", cx.listener(|this, _: &ClickEvent, _, cx| this.picker_up(cx))))
            })
            .child(div().flex_1().min_w_0().truncate().t_body().font_weight(FontWeight::MEDIUM).text_color(hex(Chrome::BRIGHT)).child(
                match &picker.current {
                    Some(dir) => tilde(dir),
                    None => t(cx, "picker.shortcuts").to_string(),
                },
            ));

        let mut list = div().id("picker-list").flex().flex_col().h(px(320.)).overflow_y_scroll().p_1();
        if rows.is_empty() {
            list = list.child(hint(t(cx, if picker.current.is_some() { "picker.no_subfolders" } else { "ext.empty" })));
        }
        for (index, (path, label)) in rows.iter().enumerate() {
            let selected = index == picker.selected;
            let dir = path.clone();
            let name = match &picker.current {
                Some(_) => path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| tilde(path)),
                None => tilde(path),
            };
            list = list.child(
                div()
                    .id(("picker-entry", index))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if selected { hex_alpha(Chrome::ACCENT, 0.35) } else { hex_alpha(0, 0.) })
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(picker) = this.picker.as_mut() {
                            picker.navigate(dir.clone(), cx);
                        }
                        cx.notify();
                    }))
                    .child(icon("folder", IconSize::BUTTON, hex(if selected { Chrome::BRIGHT } else { Chrome::MUTED })))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .child(div().flex_shrink_0().t_body().text_color(hex(Chrome::BRIGHT)).child(name.clone()))
                            .when(name == "~", |d| {
                                d.child(div().truncate().t_small().text_color(hex(Chrome::MUTED)).child(path.display().to_string()))
                            }),
                    )
                    .when(!label.is_empty(), |d| {
                        d.child(div().flex_shrink_0().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, label)))
                    })
                    .child(icon("chevron-right", IconSize::INLINE, hex(Chrome::MUTED))),
            );
        }

        let new_folder = picker.new_folder.as_ref().map(|(input, _)| {
            div()
                .mx_3()
                .flex()
                .items_center()
                .gap_2()
                .child(icon("folder-plus", IconSize::BUTTON, hex(Chrome::BLUE)))
                .child(
                    div()
                        .flex_1()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(hex(Chrome::ACCENT))
                        .bg(hex(0x1e1e1e))
                        .t_body()
                        .text_color(hex(Chrome::BRIGHT))
                        .child(input.clone()),
                )
                .child(action_button(
                    "picker-create",
                    t(cx, "picker.create_and_start"),
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        let name = this
                            .picker
                            .as_ref()
                            .and_then(|p| p.new_folder.as_ref())
                            .map(|(i, _)| i.read(cx).text().to_string())
                            .unwrap_or_default();
                        this.create_folder_and_start(&name, window, cx);
                    }),
                ))
        });

        let select_label = match &picker.current {
            Some(dir) => format!(
                "{} · {}",
                t(cx, "picker.select"),
                dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| tilde(dir))
            ),
            None => t(cx, "picker.select").to_string(),
        };

        div()
            .id("picker-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(72.))
            .bg(hex_alpha(0x000000, 0.35))
            .occlude()
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.close_picker(window, cx)))
            .child(
                div()
                    .id("picker")
                    .w(px(600.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_3()
                    .bg(hex(Chrome::OVERLAY))
                    .border_1()
                    .border_color(hex(Chrome::OVERLAY_BORDER))
                    .rounded_lg()
                    .shadow_lg()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .px_4()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(crate::brand::avatar(crate::brand::kind_id(picker.choice.kind()), 16.))
                            .child(div().t_title().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(title)),
                    )
                    .child(
                        div()
                            .mx_3()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(if picker.error.is_some() { hex(Chrome::ERROR) } else { hex(Chrome::BORDER) })
                            .bg(hex(0x1e1e1e))
                            .t_body()
                            .text_color(hex(Chrome::BRIGHT))
                            .child(picker.input.clone()),
                    )
                    .child(location)
                    .children(picker.error.map(|key| div().px_4().t_small().text_color(hex(Chrome::ERROR)).child(t(cx, key))))
                    .child(list)
                    .children(new_folder)
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .px_3()
                            .py_2()
                            .border_t_1()
                            .border_color(hex(Chrome::BORDER))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(action_button(
                                        "picker-new-folder",
                                        t(cx, "picker.new_folder"),
                                        cx.listener(|this, _: &ClickEvent, window, cx| this.begin_new_folder(window, cx)),
                                    ))
                                    .child(action_button(
                                        "picker-browse",
                                        t(cx, "picker.browse"),
                                        cx.listener(|this, _: &ClickEvent, _, cx| this.browse_folder(cx)),
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(action_button(
                                        "picker-cancel",
                                        t(cx, "picker.cancel"),
                                        cx.listener(|this, _: &ClickEvent, window, cx| this.close_picker(window, cx)),
                                    ))
                                    .child(
                                        div()
                                            .id("picker-select")
                                            .px_3()
                                            .py_1()
                                            .rounded_md()
                                            .cursor_pointer()
                                            .bg(hex(Chrome::ACCENT))
                                            .hover(|s| s.bg(hex(0x1a8ae8)))
                                            .t_small()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(hex(Chrome::BRIGHT))
                                            .max_w(px(260.))
                                            .truncate()
                                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.select_in_picker(window, cx)))
                                            .child(select_label),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_tilde() {
        assert_eq!(expand("~"), Some(home_dir()));
        assert_eq!(expand("~/code"), Some(home_dir().join("code")));
        assert_eq!(expand("/tmp"), Some(PathBuf::from("/tmp")));
        assert_eq!(expand("  "), None);
    }

    #[test]
    fn folder_names_cannot_escape() {
        assert!(valid_folder_name("my-app"));
        assert!(valid_folder_name("한글 폴더"));
        for bad in ["", " ", ".", "..", "a/b", "../x", "x\ny"] {
            assert!(!valid_folder_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn lists_visible_subfolders_sorted() {
        let dir = std::env::temp_dir().join(format!("agentty-picker-{}", std::process::id()));
        for name in ["beta", "Alpha", ".hidden"] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
        }
        std::fs::write(dir.join("file.txt"), "x").unwrap();
        let names: Vec<_> = list_children(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, ["Alpha", "beta"]);
        std::fs::remove_dir_all(dir).ok();
    }
}

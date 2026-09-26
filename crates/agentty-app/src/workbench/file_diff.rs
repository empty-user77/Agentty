//! The diff of a changed file, shown where the editor is.
//!
//! Picking a file under the files panel's "changes" tab asks what changed about it, not what is in
//! it: the editor showed the whole file and left the reader to spot the difference themselves.
//! The diff is drawn the way AgentGit draws it, and a switch in its header opens the file itself.

use super::Workbench;
use crate::i18n::t;
use crate::theme::{hex, Chrome};
use crate::ui::{icon, middle_ellipsis, IconSize, TypeScale};
use agentty_bridge::git::{DiffLine, FileChange};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, SharedString, Window};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// What the editor area shows instead of a file: the diff of one changed file.
pub(super) struct FileDiff {
    pub repo: PathBuf,
    pub path: PathBuf,
    /// `None` while git is still being asked.
    pub lines: Option<Rc<Vec<DiffLine>>>,
    pub error: Option<String>,
    pub scroll: gpui::UniformListScrollHandle,
}

impl Workbench {
    /// Shows what changed about `path`, reading the diff in the background.
    pub(super) fn open_file_diff(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(repo) = agentty_bridge::git::repo_root(path.parent().unwrap_or(path)) else {
            return;
        };
        let Ok(relative) = path.strip_prefix(&repo) else { return };
        // The kind matters: an untracked file has no diff to ask git for, it is all new.
        let change = self.files_panel.as_ref().and_then(|panel| panel.change_for(path)).unwrap_or(FileChange {
            path: relative.to_string_lossy().into_owned(),
            original: None,
            kind: 'M',
            staged: false,
        });
        self.file_diff = Some(FileDiff {
            repo: repo.clone(),
            path: path.to_path_buf(),
            lines: None,
            error: None,
            scroll: gpui::UniformListScrollHandle::new(),
        });
        self.page = None;
        self.session_viewer = None;
        self.launcher_open = false;
        self.editor_shown = true;
        if let Some(panel) = self.files_panel.as_mut() {
            panel.select(path);
        }
        let shown = path.to_path_buf();
        let task = cx.background_spawn(async move { agentty_bridge::git::working_diff(&repo, &change) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                // The user may have moved on to another file while git was answering.
                let Some(diff) = this.file_diff.as_mut().filter(|d| d.path == shown) else { return };
                match result {
                    Ok(lines) => diff.lines = Some(Rc::new(lines)),
                    Err(err) => {
                        diff.lines = Some(Rc::default());
                        diff.error = Some(format!("{err:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn close_file_diff(&mut self, cx: &mut Context<Self>) {
        if self.file_diff.take().is_some() {
            self.editor_shown = self.editor.as_ref().is_some_and(|e| !e.read(cx).is_empty());
            cx.notify();
        }
    }

    /// The diff in place of the editor, when a changed file was picked.
    pub(super) fn render_file_diff(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let diff = self.file_diff.as_ref()?;
        let name = diff.path.strip_prefix(&diff.repo).unwrap_or(&diff.path).to_string_lossy().into_owned();
        let open = diff.path.clone();
        let header = div()
            .h(px(34.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::TAB_INACTIVE))
            .t_small()
            .child(icon("git-commit-horizontal", IconSize::INLINE, hex(Chrome::MUTED)))
            .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::BRIGHT)).child(middle_ellipsis(&name, 70)))
            // The file itself is one click away: a diff answers "what changed", not "what is it".
            .child(
                div()
                    .id("file-diff-open")
                    .flex_shrink_0()
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(hex(Chrome::MUTED))
                    .hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT)))
                    .child(t(cx, "files.open_file"))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        let path = open.clone();
                        this.file_diff = None;
                        this.edit_path(&path, window, cx);
                    })),
            )
            .child(
                div()
                    .id("file-diff-close")
                    .flex_shrink_0()
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .child(icon("x", IconSize::INLINE, hex(Chrome::MUTED)))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close_file_diff(cx))),
            );
        let body: AnyElement = match &diff.lines {
            None => crate::ui::loading_row(t(cx, "files.diff_loading")).into_any_element(),
            Some(lines) if lines.is_empty() => {
                let text = diff.error.clone().unwrap_or_else(|| t(cx, "files.diff_none").to_string());
                div().flex_1().p_6().t_body().text_color(hex(Chrome::MUTED)).child(text).into_any_element()
            }
            Some(lines) => {
                let rows = lines.clone();
                let font: SharedString = crate::settings::terminal_font(cx).into();
                let gutter = rows.iter().filter_map(|l| l.old.max(l.new)).max().unwrap_or(0).to_string().len().max(3) as f32 * 8. + 12.;
                let handle = diff.scroll.clone();
                let base = handle.0.borrow().base_handle.clone();
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .group(crate::ui::SCROLL_GROUP)
                    .child(
                        gpui::uniform_list("file-diff", rows.len(), move |range, _, _| {
                            range.map(|i| crate::git_view::diff_row(&rows[i], gutter, font.clone())).collect::<Vec<_>>()
                        })
                        .track_scroll(handle)
                        .size_full(),
                    )
                    .child(crate::ui::scrollbar(base))
                    .into_any_element()
            }
        };
        Some(div().size_full().flex().flex_col().min_w_0().bg(hex(Chrome::EDITOR)).child(header).child(body).into_any_element())
    }

    /// Opens the file itself, leaving the diff behind.
    fn edit_path(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let project = self
            .files_panel
            .as_ref()
            .map(|p| p.root().to_path_buf())
            .filter(|root| path.starts_with(root))
            .or_else(|| agentty_bridge::git::repo_root(path.parent().unwrap_or(path)))
            .unwrap_or_default();
        self.open_in_editor(path, &project, window, cx);
    }
}

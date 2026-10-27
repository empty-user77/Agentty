//! The sidebar's workspace options (the settings button in the list's header): how the list is
//! laid out, grouped, ordered and filtered; and the unified header, which puts the tabs and the
//! sidebar button into the title bar beside the traffic lights.

use super::settings_page::toggle;
use super::Workbench;
use crate::i18n::t;
use crate::settings::{settings, update_settings, Settings, WorkspaceGrouping, WorkspaceSort};
use crate::theme::{hex, Chrome};
use crate::ui::{icon, popover, IconSize, TypeScale};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, SharedString, Window};

/// How far the unified header starts in, clear of the traffic lights (at 12 pt, 52 pt wide).
pub const TRAFFIC_LIGHTS_WIDTH: f32 = 76.;

/// Branch names taken for "the repository's main branch" by the default-branch filter. Read from
/// the name alone: asking git for each card's default branch on every frame would cost too much.
const DEFAULT_BRANCHES: [&str; 2] = ["main", "master"];

/// One option row's segmented choice: every value side by side, the current one raised.
fn segmented<T: Copy + PartialEq + 'static>(
    id: &'static str,
    current: T,
    choices: &[(T, &'static str)],
    set: fn(&mut Settings, T),
    cx: &mut Context<Workbench>,
) -> gpui::Div {
    let mut row = div().flex().p_0p5().gap_0p5().rounded_md().border_1().border_color(hex(Chrome::OVERLAY_BORDER));
    for (i, (value, key)) in choices.iter().copied().enumerate() {
        let on = value == current;
        row = row.child(
            div()
                .id(SharedString::from(format!("{id}-{i}")))
                .flex_1()
                .py_1()
                .rounded_sm()
                .flex()
                .justify_center()
                .t_small()
                .cursor_pointer()
                .text_color(hex(if on { Chrome::BRIGHT } else { Chrome::MUTED }))
                .when(on, |d| d.bg(hex(Chrome::SELECTED)))
                .when(!on, |d| d.hover(crate::ui::hover_fill))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| update_settings(cx, move |s| set(s, value))))
                .child(t(cx, key)),
        );
    }
    row
}

fn section_title(text: &str) -> gpui::Div {
    div().px_1().pt_2().pb_1().t_small().text_color(hex(Chrome::MUTED)).child(text.to_string())
}

fn switch_row(glyph: &'static str, label: &str, control: impl IntoElement) -> gpui::Div {
    div()
        .px_1()
        .py_1()
        .flex()
        .items_center()
        .gap_2()
        .t_body()
        .text_color(hex(Chrome::FOREGROUND))
        .child(icon(glyph, IconSize::INLINE, hex(Chrome::MUTED)))
        .child(div().flex_1().min_w_0().child(label.to_string()))
        .child(control)
}

impl Workbench {
    /// Tabs in the title bar: macOS (where Agentty draws its own title bar under the traffic
    /// lights) with the setting on, and not for a plugin's workspace, which has a tab row of its own.
    pub(super) fn unified_header(&self, cx: &gpui::App) -> bool {
        cfg!(target_os = "macos") && settings(cx).unified_header
    }

    /// With the unified header the bell sits in the sidebar's own header while the sidebar shows,
    /// leaving the title bar to the tabs; it goes back to the title bar when the sidebar is hidden.
    pub(super) fn notices_in_sidebar(&self, cx: &gpui::App) -> bool {
        self.unified_header(cx) && self.sidebar_open && self.page.is_none_or(Self::page_keeps_sidebar)
    }

    /// The branch a workspace's card shows, for the default-branch filter.
    fn card_branch(&self, ws: &super::Workspace, cx: &gpui::App) -> Option<String> {
        let folder = self.workspace_folder(ws, cx);
        ws.tabs
            .get(ws.active_tab)
            .and_then(|t| t.active.read(cx).git_branch.clone())
            .or_else(|| self.folder_branch(&folder).map(str::to_string))
            .or_else(|| ws.dormant.as_ref().and_then(|d| d.branch.clone()))
    }

    /// `members` with the options' filters and order applied. The workspace in front is never
    /// filtered out: the list must always show where you are.
    pub(super) fn list_order(&self, members: Vec<usize>, cx: &gpui::App) -> Vec<usize> {
        let options = &settings(cx).workspace_list;
        let mut kept: Vec<usize> = members
            .into_iter()
            .filter(|&i| {
                let ws = &self.workspaces[i];
                if i == self.active_workspace {
                    return true;
                }
                !(options.hide_sleeping && ws.tabs.is_empty()
                    || options.hide_plugin_workspaces && ws.plugin.is_some()
                    || options.hide_default_branch && self.card_branch(ws, cx).is_some_and(|b| DEFAULT_BRANCHES.contains(&b.as_str())))
            })
            .collect();
        match options.sort {
            WorkspaceSort::Manual => {}
            WorkspaceSort::Name => kept.sort_by_cached_key(|&i| self.workspace_label(&self.workspaces[i], cx).to_lowercase()),
            WorkspaceSort::Activity => kept.sort_by_cached_key(|&i| {
                let ws = &self.workspaces[i];
                let live = ws.tabs.iter().flat_map(|t| t.root.leaves()).map(|p| p.read(cx).last_activity_ms).max();
                std::cmp::Reverse(live.or_else(|| self.dormant_info(ws).last_activity_ms).unwrap_or(0))
            }),
        }
        kept
    }

    /// The project a workspace belongs to, for grouping by project: the repository's main folder
    /// (a worktree counts as its repository), else the workspace's own folder.
    pub(super) fn project_of(&self, index: usize, cx: &gpui::App) -> String {
        let folder = self.workspace_folder(&self.workspaces[index], cx);
        let root = agentty_bridge::worktree::main_tree(&folder).unwrap_or(folder);
        root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| root.display().to_string())
    }

    /// `members` split into one part per project, in the order each project first appears.
    pub(super) fn by_project(&self, members: &[usize], cx: &gpui::App) -> Vec<(String, Vec<usize>)> {
        let mut parts: Vec<(String, Vec<usize>)> = Vec::new();
        for &i in members {
            let project = self.project_of(i, cx);
            match parts.iter_mut().find(|(name, _)| *name == project) {
                Some((_, list)) => list.push(i),
                None => parts.push((project, vec![i])),
            }
        }
        parts
    }

    /// The settings button in the workspace list's header, with its menu below it.
    pub(super) fn render_workspace_options_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let open = self.workspace_options_open;
        div()
            .relative()
            .child(
                crate::ui::icon_only(
                    "sidebar-options",
                    "settings",
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        if this.just_dismissed("workspace-options") {
                            return;
                        }
                        this.workspace_options_open = !this.workspace_options_open;
                        cx.notify();
                    }),
                )
                .when(open, |d| d.bg(hex(Chrome::SELECTED)))
                .tooltip(crate::ui::Tooltip::text(t(cx, "workspace_options.title"), None)),
            )
            .when(open, |d| {
                // Out to the right, over the terminals: the list it changes stays in sight.
                d.child(div().absolute().top(px(26.)).left_0().child(gpui::deferred(self.render_workspace_options(cx)).with_priority(3)))
            })
            .into_any_element()
    }

    fn render_workspace_options(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = settings(cx).clone();
        let options = prefs.workspace_list.clone();
        popover()
            .id("workspace-options")
            .w(px(320.))
            .p_2()
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if std::mem::take(&mut this.workspace_options_open) {
                    this.note_dismissed("workspace-options");
                }
                cx.notify();
            }))
            .child(
                div()
                    .px_1()
                    .pb_1()
                    .t_title()
                    .font_weight(crate::theme::EMPHASIS)
                    .text_color(hex(Chrome::BRIGHT))
                    .child(t(cx, "workspace_options.title")),
            )
            .child(section_title(t(cx, "workspace_options.layout")))
            .child(segmented(
                "wo-layout",
                prefs.compact_workspaces,
                &[(false, "workspace_options.layout_cards"), (true, "workspace_options.layout_compact")],
                |s, v| s.compact_workspaces = v,
                cx,
            ))
            .when(!prefs.compact_workspaces, |d| {
                d.child(switch_row(
                    "bot",
                    t(cx, "workspace_options.card_subagents"),
                    toggle("wo-card-subagents", prefs.card_subagents, |s| s.card_subagents = !s.card_subagents, cx),
                ))
            })
            .child(section_title(t(cx, "workspace_options.group_by")))
            .child(segmented(
                "wo-group",
                options.grouping,
                &[
                    (WorkspaceGrouping::None, "workspace_options.group_none"),
                    (WorkspaceGrouping::Groups, "workspace_options.group_groups"),
                    (WorkspaceGrouping::Project, "workspace_options.group_project"),
                ],
                |s, v| s.workspace_list.grouping = v,
                cx,
            ))
            .child(section_title(t(cx, "workspace_options.sort_by")))
            .child(segmented(
                "wo-sort",
                options.sort,
                &[
                    (WorkspaceSort::Manual, "workspace_options.sort_manual"),
                    (WorkspaceSort::Activity, "workspace_options.sort_activity"),
                    (WorkspaceSort::Name, "workspace_options.sort_name"),
                ],
                |s, v| s.workspace_list.sort = v,
                cx,
            ))
            .child(div().mt_2().h(px(1.)).bg(hex(Chrome::OVERLAY_BORDER)))
            .child(section_title(t(cx, "workspace_options.filters")))
            .child(switch_row(
                "moon",
                t(cx, "workspace_options.hide_sleeping"),
                toggle("wo-hide-sleeping", options.hide_sleeping, |s| s.workspace_list.hide_sleeping = !s.workspace_list.hide_sleeping, cx),
            ))
            .child(switch_row(
                "git-branch",
                t(cx, "workspace_options.hide_default_branch"),
                toggle(
                    "wo-hide-default-branch",
                    options.hide_default_branch,
                    |s| s.workspace_list.hide_default_branch = !s.workspace_list.hide_default_branch,
                    cx,
                ),
            ))
            .child(switch_row(
                "puzzle",
                t(cx, "workspace_options.hide_plugins"),
                toggle(
                    "wo-hide-plugins",
                    options.hide_plugin_workspaces,
                    |s| s.workspace_list.hide_plugin_workspaces = !s.workspace_list.hide_plugin_workspaces,
                    cx,
                ),
            ))
            .child(div().mt_2().h(px(1.)).bg(hex(Chrome::OVERLAY_BORDER)))
            .child(section_title(t(cx, "workspace_options.window")))
            .child(switch_row(
                "search",
                t(cx, "settings.workspace_search_bar"),
                toggle("wo-search-bar", prefs.workspace_search_bar, |s| s.workspace_search_bar = !s.workspace_search_bar, cx),
            ))
            .when(cfg!(target_os = "macos"), |d| {
                d.child(switch_row(
                    "app-window",
                    t(cx, "workspace_options.unified_header"),
                    toggle("wo-unified-header", prefs.unified_header, |s| s.unified_header = !s.unified_header, cx),
                ))
            })
    }

    /// The sidebar button in the unified header: shows or hides the sidebar, in the same place
    /// whether it is open or not.
    pub(super) fn render_sidebar_toggle(&self, cx: &mut Context<Self>) -> AnyElement {
        let open = self.sidebar_open;
        crate::ui::icon_only(
            "title-sidebar-toggle",
            if open { "panel-left-close" } else { "panel-left-open" },
            cx.listener(|this, _: &ClickEvent, window, cx| {
                this.sidebar_open = !this.sidebar_open;
                if !this.sidebar_open {
                    this.focus_active(window, cx);
                }
                cx.notify();
            }),
        )
        .tooltip(crate::ui::Tooltip::text(t(cx, if open { "tooltip.hide_sidebar" } else { "tooltip.show_sidebar" }), Some("⌘B")))
        .into_any_element()
    }

    /// The unified header's row: room for the traffic lights, the sidebar button, then the tabs.
    pub(super) fn render_unified_header(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let fullscreen = window.is_fullscreen();
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            // Full screen hides the traffic lights: nothing to leave room for.
            .pl(px(if fullscreen { 8. } else { TRAFFIC_LIGHTS_WIDTH }))
            .child(div().flex_shrink_0().mr_1().child(self.render_sidebar_toggle(cx)))
            .child(div().flex_1().min_w_0().h_full().child(self.render_tab_strip_in(true, cx)))
            .into_any_element()
    }
}

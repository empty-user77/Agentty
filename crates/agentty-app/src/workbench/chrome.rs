//! Title bar, activity bar, side bar (workspaces / local sessions), tab strip, launcher and status bar.

use super::{other_agent, LaunchTarget, Page, RenameTarget, SessionFilter, SidePanel, Workbench, Workspace};
use crate::i18n::{t, tf};
use crate::launch::{LaunchChoice, PaneKind};
use crate::settings::settings;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use crate::ui::{action_button, chip, hint, icon, icon_only, icon_only_sized, menu_item, now_ms, popover, relative_time, tilde, IconSize};
use agentty_bridge::model::Agent;
use gpui::{
    div, prelude::*, px, AnyElement, ClickEvent, Context, CursorStyle, FontWeight, MouseButton, MouseDownEvent, SharedString, Window,
};

type WindowAction = Box<dyn Fn(&mut Workbench, &mut Window, &mut Context<Workbench>)>;
type ViewAction = Box<dyn Fn(&mut Workbench, &mut Context<Workbench>)>;

pub const TITLE_BAR_HEIGHT: f32 = 36.;
/// Links shown under the start page.
const RELEASES_URL: &str = "https://github.com/empty-user77/agentty-releases/releases";
const GITHUB_URL: &str = "https://github.com/empty-user77/agentty-releases";
const AUTHOR_URL: &str = "https://raylee.app";
/// Where to read how to install the agent CLIs Agentty is built around.
const CLAUDE_INSTALL_URL: &str = "https://code.claude.com/docs/en/quickstart";
const CODEX_INSTALL_URL: &str = "https://learn.chatgpt.com/docs/codex/cli#getting-started";
const COPYRIGHT_YEAR: &str = "2026";
pub const ACTIVITY_BAR_WIDTH: f32 = 48.;
const TAB_HEIGHT: f32 = 35.;
pub const STATUS_BAR_HEIGHT: f32 = 26.;
/// The pull request on a card opens on ⌘-click only; a plain click belongs to the card.
const PR_LINK_HINT: &str = "⌘ click → open";
/// How close to the sidebar's edge a dragged card has to come for the list to scroll, and how far
/// it scrolls per tick at the very edge.
const DRAG_EDGE: f32 = 44.;
const DRAG_SCROLL_STEP: f32 = 12.;
const DRAG_SCROLL_TICK: std::time::Duration = std::time::Duration::from_millis(30);
const SESSION_ROW_HEIGHT: f32 = 96.;
/// The start page: a column this wide, with as many launch cards to a row as fit (four at most).
const WELCOME_WIDTH: f32 = 920.;
const WELCOME_CARD_MIN_WIDTH: f32 = 190.;
const WELCOME_RECENT: usize = 6;

/// What a colour swatch row colours.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ColorTarget {
    Workspace(u64),
    Group(u64),
}

#[derive(Clone)]
pub struct DraggedWorkspace {
    pub id: u64,
    pub title: SharedString,
}

/// A tab of the active workspace being dragged (to reorder it or move it to another workspace).
#[derive(Clone)]
pub struct DraggedTab {
    pub workspace: u64,
    pub index: usize,
    pub title: SharedString,
}

#[derive(Clone)]
pub struct DraggedGroup {
    pub id: u64,
    pub title: SharedString,
}

pub struct DragPreview {
    pub title: SharedString,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1p5()
            .rounded_md()
            .bg(hex(Chrome::ACCENT))
            .t_body()
            .text_color(hex(Chrome::BRIGHT))
            .shadow_lg()
            .child(self.title.clone())
    }
}

/// Icon-only button in the tab strip, with a delayed name + shortcut tooltip.
fn header_icon(
    id: &'static str,
    glyph: &'static str,
    active: bool,
    tooltip: (&str, Option<&'static str>),
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .relative()
        .tooltip(crate::ui::Tooltip::text(tooltip.0.to_string(), tooltip.1))
        .flex_shrink_0()
        .my_auto()
        .size(px(crate::ui::ICON_BUTTON))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .cursor_pointer()
        .when(active, |d| d.bg(hex(Chrome::SELECTED)))
        .hover(|s| s.bg(hex(Chrome::HOVER)))
        .on_click(on_click)
        .child(icon(glyph, IconSize::BUTTON, hex(if active { Chrome::BRIGHT } else { Chrome::FOREGROUND })))
}

/// Strip along the bottom of the start page holding its links.
const FOOTER_HEIGHT: f32 = 44.0;

/// Group headers are a single line of text, so their buttons are smaller than a toolbar's.
const GROUP_ICON_BUTTON: f32 = 20.0;
const GROUP_ICON: f32 = 12.0;

/// Aggregated AI state of a workspace for its sidebar row. The card shows no status word any
/// more, so only what it counts is left: panes waiting for the user, and panes in total.
struct WorkspaceSummary {
    attention: usize,
    panes: usize,
}

impl Workbench {
    fn summarize(&self, ws: &Workspace, cx: &gpui::App) -> WorkspaceSummary {
        let panes: Vec<_> = ws.tabs.iter().flat_map(|t| t.root.leaves()).collect();
        let attention = panes.iter().filter(|p| p.read(cx).attention).count();
        WorkspaceSummary { attention, panes: panes.len() }
    }

    /// Agentty's own title bar. macOS draws the traffic lights over it; Windows and Linux use the
    /// system title bar, except Linux compositors without server-side decorations (GNOME on
    /// Wayland), where this bar also moves the window and carries its buttons.
    pub(super) fn render_title_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let client_side = !cfg!(target_os = "macos") && matches!(window.window_decorations(), gpui::Decorations::Client { .. });
        if !cfg!(target_os = "macos") && !client_side {
            return div().into_any_element();
        }
        let title = self.workspaces.get(self.active_workspace).map(|ws| self.workspace_title(ws, cx)).unwrap_or_default();
        div()
            .id("title-bar")
            .h(px(TITLE_BAR_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .relative()
            .bg(hex(Chrome::ACTIVITY_BAR))
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .t_small()
            .text_color(hex(Chrome::MUTED))
            .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, window, _| {
                if event.click_count == 2 {
                    window.titlebar_double_click();
                } else if client_side {
                    window.start_window_move();
                }
            })
            .child(if title.is_empty() { "Agentty".to_string() } else { format!("{title} — Agentty") })
            // Notifications live at the far right of the title bar, clear of everything else.
            .when(!client_side, |bar| {
                bar.child(
                    div()
                        .absolute()
                        .right(px(8.))
                        .top_0()
                        .h_full()
                        .flex()
                        .items_center()
                        // The bell must not start a window move.
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(self.render_notices_button(cx)),
                )
            })
            .when(client_side, |bar| {
                let slot = self.slot;
                bar.child(
                    div()
                        .absolute()
                        .right(px(6.))
                        .top_0()
                        .h_full()
                        .flex()
                        .items_center()
                        .gap_1()
                        // Buttons must not start a window move.
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(icon_only("window-minimize", "minus", |_, window, _| window.minimize_window()))
                        .child(icon_only("window-zoom", "square", |_, window, _| window.zoom_window()))
                        .child(icon_only(
                            "window-close",
                            "x",
                            cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.persist(cx);
                                // Without a menu bar item, closing the main window ends Agentty.
                                if slot == 0 {
                                    cx.defer(crate::request_quit);
                                } else if !this.ask_about_unsaved_files(crate::editor::AfterDiscard::CloseWindow, window, cx) {
                                    window.remove_window();
                                }
                            }),
                        )),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_activity_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let target = self.tour_target();
        let item =
            |id: &'static str, glyph: &'static str, active: bool, tooltip: &'static str, on_click: ViewAction, cx: &mut Context<Self>| {
                // Panel titles are upper-case headings; the tooltip uses the menu names.
                let (key, shortcut) = match tooltip {
                    "panel.workspaces" => ("menu.show_workspaces", "⇧⌘E"),
                    "panel.sessions" => ("menu.show_sessions", "⇧⌘S"),
                    "page.git" => (tooltip, "⇧⌘G"),
                    "page.flow" => (tooltip, "⇧⌘F"),
                    "page.monitoring" => (tooltip, "⌥⌘U"),
                    "page.extensions" => (tooltip, "⇧⌘X"),
                    "page.plugins" => (tooltip, ""),
                    "page.settings" => (tooltip, "⌘,"),
                    _ => (tooltip, ""),
                };
                div()
                    .id(id)
                    .group(id)
                    .tooltip(crate::ui::Tooltip::text(t(cx, key), (!shortcut.is_empty()).then_some(shortcut)))
                    .w_full()
                    .h(px(48.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .border_l_2()
                    .border_color(if active { hex(Chrome::BRIGHT) } else { hex_alpha(0, 0.) })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| on_click(this, cx)))
                    .child(
                        icon(glyph, IconSize::ACTIVITY, if active { hex(Chrome::BRIGHT) } else { hex(0x858585) })
                            .group_hover(id, |s| s.text_color(hex(Chrome::BRIGHT))),
                    )
                    // The onboarding tour points here.
                    .when(target == Some(id), |d| d.relative().child(crate::ui::pulse_ring(id, false)))
            };
        // One active item at a time: an open page wins over the side panel's item.
        let sidebar = |panel| self.page.is_none() && self.sidebar_open && self.panel == panel;
        div()
            .w(px(ACTIVITY_BAR_WIDTH))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .justify_between()
            .bg(hex(Chrome::ACTIVITY_BAR))
            .border_r_1()
            .border_color(hex(Chrome::BORDER))
            .child(
                div()
                    .id("activity-items")
                    .flex_1()
                    .min_h_0()
                    // More plugins than fit: the bar scrolls rather than pushing Settings off it.
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .child(item(
                        "activity-workspaces",
                        "layout-panel-left",
                        sidebar(SidePanel::Workspaces),
                        "panel.workspaces",
                        Box::new(|this, cx| this.show_panel(SidePanel::Workspaces, cx)),
                        cx,
                    ))
                    .child(item(
                        "activity-sessions",
                        "history",
                        sidebar(SidePanel::Sessions),
                        "panel.sessions",
                        Box::new(|this, cx| this.show_panel(SidePanel::Sessions, cx)),
                        cx,
                    ))
                    .child(item(
                        "activity-flow",
                        "workflow",
                        self.page == Some(Page::Flow),
                        "page.flow",
                        Box::new(|this, cx| this.open_page(Page::Flow, cx)),
                        cx,
                    ))
                    .child(item(
                        "activity-git",
                        "git-branch",
                        self.page == Some(Page::Git),
                        "page.git",
                        Box::new(|this, cx| this.open_page(Page::Git, cx)),
                        cx,
                    ))
                    .child(item(
                        "activity-usage",
                        "chart-column",
                        matches!(self.page, Some(Page::Usage | Page::Processes | Page::Proxy)),
                        "page.monitoring",
                        Box::new(|this, cx| this.open_page(Page::Usage, cx)),
                        cx,
                    ))
                    // Extensions has no icon here any more: it is reached from the command palette
                    // (⇧⌘X) and from where an extension is actually needed.
                    .child(item(
                        "activity-plugins",
                        "puzzle",
                        self.page == Some(Page::Plugins),
                        "page.plugins",
                        Box::new(|this, cx| this.open_page(Page::Plugins, cx)),
                        cx,
                    ))
                    .children(self.render_plugin_activity_items(cx)),
            )
            .child(div().flex_shrink_0().flex().flex_col().child(item(
                "activity-settings",
                "settings",
                self.page == Some(Page::Settings),
                "page.settings",
                Box::new(|this, cx| this.open_page(Page::Settings, cx)),
                cx,
            )))
    }

    pub(super) fn render_side_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = settings(cx).sidebar_width;
        let (title, body): (&str, AnyElement) = match self.panel {
            SidePanel::Workspaces => (t(cx, "panel.workspaces"), self.render_workspaces_panel(window, cx).into_any_element()),
            SidePanel::Sessions => (t(cx, "panel.sessions"), self.render_sessions_panel(cx).into_any_element()),
        };
        let collapse = icon_only(
            "sidebar-collapse",
            "panel-left-close",
            cx.listener(|this, _: &ClickEvent, window, cx| {
                this.sidebar_open = false;
                this.focus_active(window, cx);
                cx.notify();
            }),
        )
        .tooltip(crate::ui::Tooltip::text(t(cx, "tooltip.hide_sidebar"), Some("⌘B")));
        let actions = match self.panel {
            SidePanel::Workspaces => div()
                .flex()
                .gap_0p5()
                // How the cards are drawn, where they are: the start page has its own tab now, so
                // this is the one thing the list's own header should carry.
                .child({
                    let compact = settings(cx).compact_workspaces;
                    icon_only(
                        "sidebar-density",
                        // `rows-2` reads as the roomy card list, `list` as the thin one: the icon
                        // shows what a click would switch to.
                        if compact { "rows-2" } else { "list" },
                        cx.listener(|_, _: &ClickEvent, _, cx| {
                            crate::settings::update_settings(cx, |s| s.compact_workspaces = !s.compact_workspaces);
                        }),
                    )
                    .tooltip(crate::ui::Tooltip::text(t(cx, if compact { "workspaces.as_cards" } else { "workspaces.as_list" }), None))
                })
                .child(
                    icon_only(
                        "sidebar-new-group",
                        "folder-plus",
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.create_group(window, cx);
                        }),
                    )
                    .tooltip(crate::ui::Tooltip::text(t(cx, "new.group"), None)),
                )
                .child(
                    icon_only(
                        "sidebar-new-workspace",
                        "plus",
                        cx.listener(|this, _: &ClickEvent, window, cx| this.open_new_workspace_page(window, cx)),
                    )
                    .tooltip(crate::ui::Tooltip::text(t(cx, "new.workspace"), Some("⌘N"))),
                )
                .child(collapse),
            SidePanel::Sessions => div()
                .flex()
                .gap_0p5()
                .child(if self.sessions_loading {
                    div()
                        .size(px(crate::ui::ICON_BUTTON))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(crate::ui::spinner(IconSize::BUTTON, hex(Chrome::FOREGROUND)))
                        .into_any_element()
                } else {
                    icon_only("sidebar-refresh", "refresh-cw", cx.listener(|this, _: &ClickEvent, _, cx| this.refresh_sessions(cx)))
                        .tooltip(crate::ui::Tooltip::text(t(cx, "tooltip.refresh"), None))
                        .into_any_element()
                })
                .child(collapse),
        };
        // Title and session content, debounced the same way the sessions panel searches: title
        // matches show at once, content matches trail in from the background.
        let workspace_search = (self.panel == SidePanel::Workspaces && settings(cx).workspace_search_bar).then(|| {
            let query = self.workspace_query(cx);
            let searching = self.workspace_content_hits.as_ref().is_none_or(|(q, _)| *q != query) && query.chars().count() >= 2;
            div()
                .flex_shrink_0()
                .px_2()
                .pb_2()
                // Without this, a click here still focuses the text field first (it is the
                // deeper element) but then bubbles up to the sidebar's own mouse-down handler,
                // which immediately hands focus to the workspace list instead — the box takes
                // the click but never keeps the caret, so nothing typed goes anywhere.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(hex(Chrome::BORDER))
                        .bg(hex(0x1a1a1a))
                        .child(icon("search", IconSize::INLINE, hex(Chrome::MUTED)))
                        .child(div().flex_1().min_w_0().t_body().text_color(hex(Chrome::BRIGHT)).child(self.workspace_search.clone()))
                        .when(searching, |d| d.child(div().t_caption().text_color(hex(Chrome::MUTED)).child("…"))),
                )
        });

        div()
            .id("side-bar")
            .key_context("Sidebar")
            .track_focus(&self.sidebar_focus)
            // Clicking the list gives it keyboard focus (⌘N then makes a workspace).
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, _| window.focus(&this.sidebar_focus)))
            .relative()
            .w(px(width))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(hex(Chrome::SIDE_BAR))
            .border_r_1()
            .border_color(hex(Chrome::BORDER))
            .child(
                div()
                    .h(px(35.))
                    .flex_shrink_0()
                    .pl_4()
                    .pr_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .t_small()
                    .text_color(hex(Chrome::MUTED))
                    .child(title)
                    .child(actions),
            )
            .children(workspace_search)
            .child(match self.panel {
                // Sessions manage their own virtualized scrolling.
                SidePanel::Sessions => div().flex_1().min_h_0().child(body).into_any_element(),
                SidePanel::Workspaces => div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("sidebar-scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.sidebar_scroll)
                            .pb_4()
                            // A held pointer can't scroll the list: nearing an edge scrolls it, so
                            // a card can be dropped below what the sidebar happens to show.
                            .on_drag_move(cx.listener(|this, event: &gpui::DragMoveEvent<DraggedWorkspace>, _, cx| {
                                this.drag_autoscroll(event.event.position, cx)
                            }))
                            .on_drag_move(cx.listener(|this, event: &gpui::DragMoveEvent<DraggedGroup>, _, cx| {
                                this.drag_autoscroll(event.event.position, cx)
                            }))
                            .on_drag_move(cx.listener(|this, event: &gpui::DragMoveEvent<DraggedTab>, _, cx| {
                                this.drag_autoscroll(event.event.position, cx)
                            }))
                            .child(body),
                    )
                    .group(crate::ui::SCROLL_GROUP)
                    .child(crate::ui::scrollbar(self.sidebar_scroll.clone()))
                    .into_any_element(),
            })
            .child(
                div()
                    .id("sidebar-resize")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right(px(-3.))
                    .w(px(6.))
                    .cursor(CursorStyle::ResizeLeftRight)
                    .hover(|s| s.bg(hex_alpha(Chrome::ACCENT, 0.6)))
                    .when(self.sidebar_resizing, |d| d.bg(hex(Chrome::ACCENT)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.sidebar_resizing = true;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            )
    }

    fn render_workspaces_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().flex().flex_col().px_2().pt_1().gap_px();
        if self.workspaces.is_empty() && self.groups.is_empty() {
            return list.child(hint(t(cx, "hint.no_workspaces")));
        }

        // A blank box matches everything, so filtering costs nothing when the user isn't searching.
        // Computed once, up front: the render calls below need `cx` mutably, so nothing here can
        // hold a borrow of it.
        let query = self.workspace_query(cx);
        let searching = !query.is_empty();
        let is_match: Vec<bool> = (0..self.workspaces.len()).map(|i| !searching || self.workspace_matches(i, cx)).collect();
        let mut any_match = false;

        // Workspaces linked into one session sit together under the one the link started from,
        // wherever they are in the list; unlinking puts them straight back.
        let linked = self.linked_workspaces(cx);
        let ungrouped: Vec<usize> = (0..self.workspaces.len())
            .filter(|i| self.workspaces[*i].group.is_none() && !linked.followers.contains(i) && is_match[*i])
            .collect();
        any_match = any_match || !ungrouped.is_empty();
        // The "ungrouped" header matters once groups exist and something is (or is being dragged)
        // outside them; while searching, an empty ungrouped section is left out entirely.
        let dragging = cx.has_active_drag();
        let mut ungrouped_open = true;
        if !self.groups.is_empty() && (!ungrouped.is_empty() || (!searching && dragging)) {
            ungrouped_open = !self.ungrouped_collapsed;
            let label = t(cx, "ungrouped").into();
            list = list.child(self.render_group_header(None, label, self.ungrouped_collapsed, ungrouped.len(), window, cx));
        }
        if ungrouped_open {
            for index in ungrouped {
                list = list.child(self.render_workspace_entry(index, &linked, window, cx));
            }
        }
        for group in &self.groups {
            let all_members: Vec<usize> = (0..self.workspaces.len())
                .filter(|i| self.workspaces[*i].group == Some(group.id) && !linked.followers.contains(i))
                .collect();
            let members: Vec<usize> = all_members.iter().copied().filter(|i| is_match[*i]).collect();
            if searching && members.is_empty() {
                // Nothing in this group matches: skip it rather than show an empty frame.
                continue;
            }
            any_match = true;
            // A group with a colour is drawn inside a frame of it, header included, so it reads as
            // one block; the header keeps its own rounded top.
            let accent = super::accent_color(group.color);
            let mut block = div().flex().flex_col().gap_px().map(|d| match accent {
                Some(color) => d.rounded_md().border_1().border_color(hex_alpha(color, 0.55)).p_px(),
                None => d,
            });
            let count = if searching { members.len() } else { all_members.len() };
            block = block.child(self.render_group_header(Some(group.id), group.name.clone(), group.collapsed, count, window, cx));
            if group.collapsed {
                list = list.child(block);
                continue;
            }
            if members.is_empty() && !searching {
                let gid = group.id;
                block = block.child(
                    div()
                        .id(SharedString::from(format!("group-empty-add-{gid}")))
                        .ml_3()
                        .px_2()
                        .py_1p5()
                        .rounded_md()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .t_small()
                        .cursor_pointer()
                        .text_color(hex(Chrome::MUTED))
                        .hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT)))
                        .child(icon("plus", IconSize::INLINE, hex(Chrome::MUTED)))
                        .child(t(cx, "group.add_workspace"))
                        .on_click(
                            cx.listener(move |this, _: &ClickEvent, window, cx| this.open_new_workspace_page_in(Some(gid), window, cx)),
                        ),
                );
            }
            for index in members {
                block = block.child(div().pl_3().child(self.render_workspace_entry(index, &linked, window, cx)));
            }
            list = list.child(block);
        }
        if searching && !any_match {
            list = list.child(hint(t(cx, "workspaces.no_matches")));
        }
        list
    }

    /// A workspace row, with the workspaces linked to it gathered underneath when there are any.
    fn render_workspace_entry(
        &self,
        index: usize,
        linked: &super::flow::LinkedWorkspaces,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(followers) = linked.members.get(&index).filter(|f| !f.is_empty()) else {
            return self.render_workspace_row(index, window, cx).into_any_element();
        };
        let mut block = div()
            .flex()
            .flex_col()
            .gap_px()
            .rounded_md()
            .border_1()
            .border_color(hex_alpha(Chrome::BLUE, 0.45))
            .p_px()
            // Why these cards are together, in one line above them.
            .child(
                div()
                    .px_2()
                    .pt_0p5()
                    .flex()
                    .items_center()
                    .gap_1()
                    .t_caption()
                    .text_color(hex_alpha(Chrome::BLUE, 0.9))
                    .child(icon("link", 10., hex_alpha(Chrome::BLUE, 0.9)))
                    .child(tf(cx, "collab.linked_group", &[("n", &(followers.len() + 1).to_string())])),
            )
            .child(self.render_workspace_row(index, window, cx));
        for member in followers {
            block = block.child(div().pl_3().child(self.render_workspace_row(*member, window, cx)));
        }
        block.into_any_element()
    }

    fn render_group_header(
        &self,
        group: Option<u64>,
        name: String,
        collapsed: bool,
        count: usize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = SharedString::from(format!("group-header-{}", group.unwrap_or(0)));
        let drag_title: SharedString = name.clone().into();
        div()
            .id(id)
            .relative()
            .group("group-header")
            .mt_1p5()
            .px_2()
            .py_0p5()
            .flex()
            .items_center()
            .gap_1p5()
            .rounded_md()
            .t_small()
            .text_color(hex(Chrome::MUTED))
            .cursor_pointer()
            .drag_over::<DraggedWorkspace>(|style, _, _, _| style.bg(hex_alpha(Chrome::ACCENT, 0.3)))
            .on_drop(cx.listener(move |this, dragged: &DraggedWorkspace, _, cx| this.move_to_group(dragged.id, group, cx)))
            .when_some(group, |d, gid| {
                // Groups reorder by dragging their headers onto each other.
                d.on_drag(DraggedGroup { id: gid, title: drag_title.clone() }, |dragged, _, _, cx| {
                    cx.new(|_| DragPreview { title: dragged.title.clone() })
                })
                .drag_over::<DraggedGroup>(|style, _, _, _| style.border_t_2().border_color(hex(Chrome::ACCENT)))
                .on_drop(cx.listener(move |this, dragged: &DraggedGroup, _, cx| this.move_group(dragged.id, gid, cx)))
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                let Some(gid) = group else {
                    // "Ungrouped" has no name to rename, but it folds away like the rest.
                    this.ungrouped_collapsed = !this.ungrouped_collapsed;
                    this.persist(cx);
                    return cx.notify();
                };
                if event.click_count() == 2 {
                    return this.start_rename(RenameTarget::Group(gid), window, cx);
                }
                if let Some(g) = this.groups.iter_mut().find(|g| g.id == gid) {
                    g.collapsed = !g.collapsed;
                }
                this.persist(cx);
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    let Some(gid) = group else { return };
                    cx.stop_propagation();
                    this.group_menu = if this.group_menu == Some(gid) { None } else { Some(gid) };
                    cx.notify();
                }),
            )
            .child(if collapsed { "▸" } else { "▾" })
            // Shown as it was typed: lower case, emoji and punctuation are all a valid name.
            .child(div().flex_1().truncate().t_body().font_weight(crate::theme::EMPHASIS).child(name.clone()))
            .when_some(group, |d, gid| {
                d.child(
                    div()
                        .flex()
                        .invisible()
                        .group_hover("group-header", |s| s.visible())
                        .child(
                            icon_only_sized(
                                SharedString::from(format!("group-add-{gid}")),
                                "plus",
                                GROUP_ICON_BUTTON,
                                GROUP_ICON,
                                cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    this.open_new_workspace_page_in(Some(gid), window, cx);
                                }),
                            )
                            .tooltip(crate::ui::Tooltip::text(t(cx, "group.add_workspace"), None)),
                        )
                        .child(icon_only_sized(
                            SharedString::from(format!("group-rename-{gid}")),
                            "pencil",
                            GROUP_ICON_BUTTON,
                            GROUP_ICON,
                            cx.listener(move |this, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                this.start_rename(RenameTarget::Group(gid), window, cx);
                            }),
                        ))
                        .child(icon_only_sized(
                            SharedString::from(format!("group-delete-{gid}")),
                            "x",
                            GROUP_ICON_BUTTON,
                            GROUP_ICON,
                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.delete_group(gid, cx);
                            }),
                        )),
                )
            })
            // Last, so the count sits at the right edge of every header: a group's buttons take
            // their space even while hidden, and would otherwise push the number off the edge.
            .child(div().child(count.to_string()))
            .children(group.filter(|gid| self.group_menu == Some(*gid)).map(|gid| {
                div().absolute().top(px(26.)).left(px(8.)).child(gpui::deferred(self.render_group_menu(gid, cx)).with_priority(3))
            }))
    }

    /// Right-click menu of a group: rename it, colour it, remove it.
    fn render_group_menu(&self, gid: u64, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.groups.iter().find(|g| g.id == gid).and_then(|g| g.color);
        popover()
            .id("group-menu")
            .w(px(220.))
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.group_menu = None;
                cx.notify();
            }))
            .child(menu_item(
                "gm-rename",
                t(cx, "rename"),
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.group_menu = None;
                    this.start_rename(RenameTarget::Group(gid), window, cx);
                }),
            ))
            .child(self.render_color_row("gm", ColorTarget::Group(gid), current, cx))
            .child(div().my_1().h(px(1.)).bg(hex(Chrome::OVERLAY_BORDER)))
            .child(menu_item(
                "gm-delete",
                t(cx, "group.delete"),
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.group_menu = None;
                    this.delete_group(gid, cx);
                }),
            ))
    }

    fn render_workspace_row(&self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = &self.workspaces[index];
        let id = ws.id;
        let active = index == self.active_workspace && self.page.is_none_or(Workbench::page_keeps_sidebar) && self.session_viewer.is_none();
        let title = self.workspace_title(ws, cx);
        // A workspace with no name of its own falls back to a path. The card shows the path on its
        // own line below, so the name only needs the folder it ends in.
        let title = match title.rsplit_once('/') {
            Some((_, last)) if !last.is_empty() => last.to_string(),
            _ => title,
        };
        let summary = self.summarize(ws, cx);
        let renaming = matches!(&self.rename, Some(r) if r.target == RenameTarget::Workspace(id));
        let ws_colored = ws.color.is_some();
        // The fill this card is actually drawn in, and the ink that stays readable on it. A colour
        // the user picked can be anything the colour panel offers: pale amber wants dark letters,
        // deep blue light ones. Measured, not assumed — see `theme::ink_on`.
        let fill = super::accent_color(ws.color).map(|color| if active { crate::theme::lighten_rgb(color, 0.14) } else { color });
        let ink = fill.map_or(Chrome::BRIGHT, crate::theme::ink_on);
        // A step behind the name: smaller, and dimmer than the title on both kinds of card.
        let sub_color = if ws_colored { hex_alpha(ink, 0.7) } else { hex(Chrome::MUTED) };
        let panes: Vec<_> = ws.tabs.iter().flat_map(|t| t.root.leaves()).collect();
        // A workspace with nothing running answers from its saved layout instead of from its
        // panes, so the card says the same thing before and after it is opened.
        let dormant = ws.tabs.is_empty().then(|| self.dormant_info(ws));
        // Tools in use, AI agents first: shown as overlapping logos.
        let mut tools: Vec<String> = Vec::new();
        for pane in &panes {
            let tool = pane.read(cx).tool_id().to_string();
            if !tools.contains(&tool) {
                tools.push(tool);
            }
        }
        tools.sort_by_key(|t| t.as_str() == "shell");
        if let Some(dormant) = &dormant {
            tools = dormant.tools.clone();
        }
        if tools.is_empty() {
            tools.push("shell".to_string());
        }
        let last_activity = match &dormant {
            Some(dormant) => dormant.last_activity_ms,
            None => panes.iter().map(|p| p.read(cx).last_activity_ms).max(),
        };
        let counts = {
            let (tabs, pane_count) = match &dormant {
                Some(dormant) => (dormant.tabs, dormant.panes),
                None => (ws.tabs.len(), summary.panes),
            };
            let mut parts = Vec::new();
            if tabs > 1 {
                parts.push(tf(cx, "count.tabs", &[("n", &tabs.to_string())]));
            }
            if pane_count > tabs.max(1) {
                parts.push(tf(cx, "count.panes", &[("n", &pane_count.to_string())]));
            }
            parts.join(" · ")
        };
        // A workspace that has not been opened yet has no pane to ask: its saved layout says where
        // it works, and its branch comes from that folder.
        let cwd = match &dormant {
            Some(dormant) => dormant.cwd.clone(),
            None => ws.tabs.get(ws.active_tab).map(|t| t.active.read(cx).display_cwd()).unwrap_or_else(|| ws.cwd.clone()),
        };
        let path = tilde(&cwd);
        // The path is shortened on its own; the counts are short and go after it whole, so a cut
        // never lands in the middle of "탭 2개".
        let detail = {
            let path = crate::ui::middle_ellipsis(&path, 24);
            if counts.is_empty() {
                path
            } else {
                format!("{path} · {counts}")
            }
        };
        // What the workspace is on, like cmux's cards: branch, pull request, and the last thing
        // said in the pane the user worked in.
        let last_pane = ws.tabs.get(ws.active_tab).map(|t| t.active.clone()).or_else(|| panes.first().cloned());
        // Folded away, the folder is read again in the background; until that answer arrives the
        // branch saved when the workspace closed stands in for it.
        let branch = last_pane
            .as_ref()
            .and_then(|p| p.read(cx).git_branch.clone())
            .or_else(|| self.folder_branch(&cwd).map(str::to_string))
            .or_else(|| dormant.as_ref().and_then(|d| d.branch.clone()));
        let pull_request = branch
            .as_ref()
            .and_then(|branch| Some((agentty_bridge::git::repo_root(&cwd)?, branch.clone())))
            .and_then(|(repo, branch)| self.pull_request_of(&repo, &branch).cloned());
        let working = panes.iter().any(|p| p.read(cx).status.in_turn());
        // The logo is only worth the room when there is more than one kind of agent to tell apart:
        // with a single CLI on the machine every card carried the same mark. Without it the name
        // starts at the edge, and the lines under it follow — nothing is left indented under a
        // logo that is not there. Whether a card is working still shows, since that differs.
        let show_logo = self.installed.as_ref().map_or(2, crate::agents::Installed::agent_count) > 1;
        let indent = px(if show_logo { 24. } else { 2. });
        let compact = settings(cx).compact_workspaces;

        // A bar down the left edge, for the one thing a colour cannot say: a pane in here is
        // waiting for the user. A card that already carries a colour would only clash with it, so
        // there the mark sits next to the name instead.
        let attention_bar = (summary.attention > 0 && ws.color.is_none()).then_some(Chrome::ATTENTION);
        let attention_dot = summary.attention > 0 && ws.color.is_some();
        // Opaque, never a wash: a translucent fill over the dark chrome comes out muddy. The
        // selected card is the same colour a shade lighter, with a rim. (`fill` is already the
        // lightened tone when the card is selected.)
        let background = match (fill, active) {
            (Some(color), _) => hex(color),
            (None, true) => hex_alpha(Chrome::ACCENT, 0.16),
            (None, false) => hex_alpha(0, 0.),
        };
        let row = div()
            .id(("workspace", id as usize))
            .group("workspace-row")
            .relative()
            .pl(px(10.))
            .pr_2p5()
            .map(|d| if compact { d.py_0p5() } else { d.py_1p5() })
            .rounded_md()
            .cursor_pointer()
            .border_1()
            // A colour the user gave a workspace fills its whole card, so the sidebar can be read
            // as blocks of colour. Selected is the same fill, brighter, with a light rim; an
            // uncoloured card keeps the quiet tint that does not wash its own text out.
            .border_color(if active { hex_alpha(Chrome::BRIGHT, 0.55) } else { hex_alpha(0, 0.) })
            .bg(background)
            .children(
                attention_bar.map(|color| div().absolute().left(px(2.)).top(px(5.)).bottom(px(5.)).w(px(3.)).rounded_full().bg(hex(color))),
            )
            // Hover lightens rather than washes out: a translucent fill would darken a pale
            // colour back towards the chrome, and the ink was chosen for the opaque tone.
            .when(!active, |d| {
                d.hover(|s| {
                    s.bg(match fill {
                        Some(color) => crate::theme::lighten(color, 0.10),
                        None => hex(Chrome::HOVER),
                    })
                })
            })
            .on_drag(DraggedWorkspace { id, title: title.clone().into() }, |dragged, _, _, cx| {
                cx.new(|_| DragPreview { title: dragged.title.clone() })
            })
            // Dropping another workspace here puts it just above this one (and in this group).
            .drag_over::<DraggedWorkspace>(|style, _, _, _| style.border_t_2().border_color(hex(Chrome::BLUE)))
            .on_drop(cx.listener(move |this, dragged: &DraggedWorkspace, _, cx| this.move_workspace(dragged.id, id, cx)))
            .drag_over::<DraggedTab>(
                move |style, dragged, _, _| {
                    if dragged.workspace == id {
                        style
                    } else {
                        style.bg(hex_alpha(Chrome::ACCENT, 0.3))
                    }
                },
            )
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                let source = this.workspaces.get(this.active_workspace).map(|w| w.id);
                if dragged.workspace != id && source == Some(dragged.workspace) {
                    this.move_tab(dragged.index, Some(id), window, cx);
                }
            }))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if let Some(i) = this.workspaces.iter().position(|w| w.id == id) {
                    if event.click_count() >= 2 || matches!(&this.rename, Some(r) if r.target == RenameTarget::Workspace(id)) {
                        return;
                    }
                    if this.page == Some(Page::Git) {
                        // AgentGit stays open and switches to this workspace's repositories.
                        this.select_workspace_for_page(i, cx);
                        window.focus(&this.sidebar_focus);
                    } else {
                        // The terminal takes the keyboard: switching workspace and then having to
                        // click the pane before typing was the thing people kept tripping over.
                        this.activate_workspace(i, window, cx);
                    }
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    // One logo: which agent it is, without a pile of icons down the sidebar. It
                    // breathes while a turn is running.
                    .map(|d| {
                        if show_logo {
                            d.child(crate::brand::avatar_working(tools.first().map_or("shell", String::as_str), 16., working, id, ink))
                        } else if working {
                            d.child(crate::ui::dot_spinner(("card-working", id as usize), 14., hex_alpha(ink, 0.9)))
                        } else {
                            d
                        }
                    })
                    .when(attention_dot, |d| d.child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(hex_alpha(ink, 0.95))))
                    .child({
                        let _ = renaming;
                        div()
                            .id(("workspace-title", id as usize))
                            .flex_1()
                            .min_w(px(60.))
                            .truncate()
                            // Heavier and a size up from the lines under it: the name is what a
                            // card is found by, and at one size and weight they all read as one
                            // block. The chrome's own emphasis weight, not full bold — the size
                            // difference already does most of the separating.
                            .t_title()
                            .font_weight(crate::theme::EMPHASIS)
                            .text_color(hex(ink))
                            // Double-click the name to rename (handled on mouse down so the first
                            // click's focus change can't end the edit).
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    if event.click_count >= 2 {
                                        cx.stop_propagation();
                                        this.start_rename(RenameTarget::Workspace(id), window, cx);
                                    }
                                }),
                            )
                            .child(title)
                    })
                    // The list view drops the branch/path line below the title to fit more
                    // workspaces on screen; the branch is still worth a glance without opening
                    // the card, so a short form of it rides along the title row instead.
                    .when(compact, |d| {
                        d.children(
                            branch
                                .clone()
                                .map(|name| div().flex_shrink_0().max_w(px(96.)).truncate().t_caption().text_color(sub_color).child(name)),
                        )
                    })
                    // Last activity, like the session history; the menu button takes its place on hover.
                    .child(
                        div()
                            .relative()
                            .flex_shrink_0()
                            .min_w(px(26.))
                            .h(px(20.))
                            .flex()
                            .items_center()
                            .justify_end()
                            .children(last_activity.map(|at| {
                                div()
                                    .group_hover("workspace-row", |s| s.invisible())
                                    .t_caption()
                                    .text_color(sub_color)
                                    .child(relative_time(now_ms(), at))
                            }))
                            .child(
                                div()
                                    .absolute()
                                    .top(px(-3.))
                                    .right(px(-4.))
                                    .invisible()
                                    .group_hover("workspace-row", |s| s.visible())
                                    .child(crate::ui::icon_only_in(
                                        ("workspace-menu", id as usize),
                                        "ellipsis",
                                        ink,
                                        cx.listener(move |this, event: &ClickEvent, _, cx| {
                                            cx.stop_propagation();
                                            if this.just_dismissed("workspace-menu") {
                                                return;
                                            }
                                            this.workspace_menu_at = f32::from(event.position().y);
                                            this.workspace_menu = if this.workspace_menu == Some(id) { None } else { Some(id) };
                                            cx.notify();
                                        }),
                                    )),
                            ),
                    ),
            )
            // A line each: a branch name and a path never fit side by side in a sidebar, and both
            // matter too much to be cut short so they can share one.
            .when(!compact, |d| {
                d.children(branch.map(|name| {
                    div()
                        .pl(indent)
                        .pt_0p5()
                        .flex()
                        .items_center()
                        .gap_1()
                        .t_caption()
                        .text_color(sub_color)
                        .overflow_hidden()
                        .child(icon("git-branch", 10., sub_color))
                        .child(div().min_w_0().truncate().child(name))
                }))
                .child(div().pl(indent).truncate().t_caption().text_color(sub_color).child(detail))
            })
            .children(pull_request.filter(|_| !compact).map(|pr| {
                let (label, state_color) = if pr.is_merged() {
                    (tf(cx, "pr.merged", &[("n", &pr.number.to_string())]), Chrome::PURPLE)
                } else if pr.is_open() {
                    (tf(cx, "pr.open", &[("n", &pr.number.to_string())]), Chrome::GREEN)
                } else {
                    (tf(cx, "pr.closed", &[("n", &pr.number.to_string())]), Chrome::MUTED)
                };
                // A card the user coloured keeps to that colour: a green line inside an amber card
                // is two colours fighting, and the state is in the words anyway.
                let color = if ws_colored { hex_alpha(ink, 0.9) } else { hex(state_color) };
                let url = pr.url.clone();
                div()
                    .id(("workspace-pr", id as usize))
                    .pl(indent)
                    .pt_0p5()
                    .flex()
                    .items_center()
                    .gap_1()
                    .t_caption()
                    .cursor_pointer()
                    .text_color(color)
                    .hover(|s| s.opacity(0.8))
                    .tooltip(crate::ui::Tooltip::text(pr.title.clone(), Some(PR_LINK_HINT)))
                    // Only on ⌘-click: a plain click anywhere on the card opens the workspace, and
                    // a pull request page is not where that click should land.
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        if crate::keymap::link_modifier(&event.modifiers()) {
                            cx.stop_propagation();
                            this.open_link(url.clone(), cx);
                        }
                    }))
                    .child(icon("git-pull-request", 10., color))
                    .child(div().truncate().child(label))
            }))
            .children((!compact).then(|| self.render_port_chips(ws, fill, active, indent, cx)).flatten());

        let menu_open = self.workspace_menu == Some(id);
        // A card near the bottom of the sidebar would have its menu cut off by the window edge, so
        // there the menu grows upwards from the card instead of downwards.
        let upwards = menu_open && self.workspace_menu_at + self.workspace_menu_height() > f32::from(window.viewport_size().height) - 8.;
        // Deferred so the menu paints above the rows below it (e.g. the selected workspace).
        div().relative().child(row).when(menu_open, |d| {
            d.child(
                div()
                    .absolute()
                    .map(|d| if upwards { d.bottom(px(28.)) } else { d.top(px(28.)) })
                    .right(px(4.))
                    .child(gpui::deferred(self.render_workspace_menu(id, window, cx)).with_priority(3)),
            )
        })
    }

    /// `:3000 :5173` chips for servers started in the workspace; a click opens them. On a card the
    /// user coloured the chips are cut from its own ink, so they read on a pale fill as on a dark one.
    fn render_port_chips(
        &self,
        ws: &Workspace,
        fill: Option<u32>,
        active: bool,
        indent: gpui::Pixels,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let ports = self.ports_of(ws.tabs.iter().flat_map(|t| t.root.leaves()).map(|p| p.read(cx).pane_id));
        if ports.is_empty() {
            return None;
        }
        // A chip has its own padding, so it lines up with the text above it a touch further in.
        let mut row = div().pl(indent + px(4.)).pt_0p5().flex().flex_wrap().gap_1();
        for port in ports.into_iter().take(6) {
            row = row.child(
                div()
                    .id(SharedString::from(format!("port-{}-{port}", ws.id)))
                    .px_1p5()
                    .rounded_sm()
                    .t_caption()
                    .cursor_pointer()
                    .map(|d| match fill.map(crate::theme::ink_on) {
                        Some(ink) => d.bg(hex_alpha(ink, 0.16)).text_color(hex(ink)).hover(|s| s.bg(hex_alpha(ink, 0.3))),
                        None => d
                            .bg(if active { hex_alpha(0xffffff, 0.18) } else { hex(0x2d2d30) })
                            .text_color(hex(if active { Chrome::BRIGHT } else { Chrome::FOREGROUND }))
                            .hover(|s| s.bg(hex(Chrome::SELECTED))),
                    })
                    .child(format!(":{port}"))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_link(format!("http://localhost:{port}"), cx);
                    })),
            );
        }
        Some(row.into_any_element())
    }

    /// "Recently closed tabs" of the active workspace: reopening one brings its splits back.
    pub(super) fn render_reopen_tabs(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let ws = self.workspaces.get(self.active_workspace)?;
        if ws.closed_tabs.is_empty() {
            return None;
        }
        let workspace = ws.id;
        let mut list = div()
            .flex()
            .flex_col()
            // The rule goes above the heading: this section is last in the menu.
            .child(div().my_1().h(px(1.)).bg(hex(Chrome::OVERLAY_BORDER)))
            .child(div().px_3().pt_1().pb_1().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "tabs.recently_closed")));
        for (index, tab) in ws.closed_tabs.iter().take(6).enumerate() {
            let (title, panes) = closed_tab_label(&tab.layout);
            let label = if panes > 1 { format!("{title} ({panes})") } else { title };
            list = list.child(
                menu_item(
                    SharedString::from(format!("reopen-tab-{index}")),
                    label,
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.launcher_open = false;
                        this.tab_menu = None;
                        this.reopen_closed_tab(workspace, index, window, cx);
                    }),
                )
                .into_any_element(),
            );
        }
        Some(list)
    }

    /// Colour swatches for a workspace or a group: the eight quick ones, "none", and a palette of
    /// every shade behind "more", so a long sidebar can still give each workspace its own colour.
    fn render_color_row(&self, prefix: &'static str, target: ColorTarget, current: Option<u32>, cx: &mut Context<Self>) -> gpui::Div {
        let swatch = |key: &str, color: Option<u32>, size: f32, cx: &mut Context<Self>| {
            let selected = current == color;
            div()
                .id(SharedString::from(format!("{prefix}-color-{key}")))
                .size(px(size + 6.))
                .rounded_full()
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .border_2()
                .border_color(if selected { hex(Chrome::BRIGHT) } else { hex_alpha(0, 0.) })
                .map(|d| match color {
                    Some(color) => d.child(div().size(px(size)).rounded_full().bg(hex(color))),
                    // "None": an empty ring, so clearing a colour is as easy as setting one.
                    None => d.child(div().size(px(size)).rounded_full().border_1().border_color(hex(Chrome::MUTED))),
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_color(target, color, cx)))
        };
        let mut row = div().px_3().py_1().flex().flex_wrap().gap_1p5().child(swatch("none", None, 12., cx));
        for color in super::ACCENTS {
            row = row.child(swatch(&format!("{color:06x}"), Some(color), 12., cx));
        }
        let open = self.color_palette_open == Some(target);
        row = row.child(
            div()
                .id(SharedString::from(format!("{prefix}-color-more")))
                .size(px(18.))
                .rounded_full()
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(hex(if open { Chrome::BRIGHT } else { Chrome::MUTED }))
                .tooltip(crate::ui::Tooltip::text(t(cx, "color.more"), None))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.color_palette_open = (!open).then_some(target);
                    cx.notify();
                }))
                .child(icon("ellipsis", 11., hex(Chrome::MUTED))),
        );
        // The system colour panel: the wheel, the sliders, the crayons and a hex field. What it
        // shows is applied live, so the card is the preview.
        row = row.child(
            div()
                .id(SharedString::from(format!("{prefix}-color-pick")))
                .size(px(18.))
                .rounded_full()
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(hex(Chrome::MUTED))
                .tooltip(crate::ui::Tooltip::text(t(cx, "color.custom"), None))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.pick_custom_color(target, cx)))
                .child(icon("pencil", 10., hex(Chrome::MUTED))),
        );
        let mut palette = div().px_3().pb_2().flex().flex_wrap().gap_1();
        if open {
            for color in super::PALETTE {
                palette = palette.child(swatch(&format!("p{color:06x}"), Some(color), 14., cx));
            }
        }
        div()
            .flex()
            .flex_col()
            .child(div().px_3().pt_2().pb_0p5().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "color.label")))
            .child(row)
            .when(open, |d| d.child(palette))
    }

    /// Roughly how tall the workspace menu comes out, to decide which way it opens. Measured from
    /// what it is built of below: rows, the colour block, and the palette when it is unfolded.
    fn workspace_menu_height(&self) -> f32 {
        const ROW: f32 = 28.;
        const COLOUR_BLOCK: f32 = 52.;
        const PALETTE: f32 = 116.;
        let groups = self.groups.len() as f32;
        let palette = if matches!(self.color_palette_open, Some(ColorTarget::Workspace(_))) { PALETTE } else { 0. };
        let group_section = if groups > 0. { 24. + groups * ROW } else { 0. };
        // rename + colour + groups + "new group" + ungroup + rule + close, inside the popover's padding.
        12. + ROW + COLOUR_BLOCK + palette + group_section + ROW * 2. + 9. + ROW
    }

    fn render_workspace_menu(&self, id: u64, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_group = self.workspaces.iter().find(|w| w.id == id).and_then(|w| w.group);
        let mut menu = popover()
            .id("workspace-menu")
            .w(px(220.))
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.workspace_menu.take().is_some() {
                    this.note_dismissed("workspace-menu");
                }
                cx.notify();
            }))
            .child(menu_item(
                "wm-rename",
                t(cx, "rename"),
                cx.listener(move |this, _: &ClickEvent, window, cx| this.start_rename(RenameTarget::Workspace(id), window, cx)),
            ))
            .child(self.render_color_row(
                "wm",
                ColorTarget::Workspace(id),
                self.workspaces.iter().find(|w| w.id == id).and_then(|w| w.color),
                cx,
            ));
        if !self.groups.is_empty() {
            menu = menu.child(div().px_3().pt_2().pb_1().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "move.to.group")));
            for group in &self.groups {
                let gid = group.id;
                let label = if current_group == Some(gid) { format!("✓ {}", group.name) } else { group.name.clone() };
                menu = menu.child(menu_item(
                    SharedString::from(format!("wm-group-{gid}")),
                    label,
                    cx.listener(move |this, _: &ClickEvent, _, cx| this.move_to_group(id, Some(gid), cx)),
                ));
            }
        }
        menu.child(menu_item(
            "wm-new-group",
            format!("+ {}", t(cx, "new.group")),
            cx.listener(move |this, _: &ClickEvent, window, cx| {
                let gid = this.create_group(window, cx);
                this.move_to_group(id, Some(gid), cx);
            }),
        ))
        .when(current_group.is_some(), |d| {
            d.child(menu_item(
                "wm-ungroup",
                t(cx, "remove.from.group"),
                cx.listener(move |this, _: &ClickEvent, _, cx| this.move_to_group(id, None, cx)),
            ))
        })
        .child(div().my_1().h(px(1.)).bg(hex(Chrome::OVERLAY_BORDER)))
        .child(menu_item(
            "wm-close",
            t(cx, "close.workspace"),
            cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.workspace_menu = None;
                this.request_close(super::confirm::CloseTarget::Workspace(id), window, cx)
            }),
        ))
    }

    /// Sessions visible under the current filter, favorites first.
    pub(super) fn visible_sessions(&self, cx: &gpui::App) -> Vec<usize> {
        let favorites = &settings(cx).favorite_sessions;
        let query = self.session_query(cx);
        let hits = self.session_content_hits.as_ref().filter(|(q, _)| *q == query).map(|(_, hits)| hits);
        let mut indices: Vec<usize> = (0..self.sessions.len())
            .filter(|&i| match self.session_filter {
                SessionFilter::All => true,
                SessionFilter::Only(agent) => self.sessions[i].agent == agent,
            })
            .filter(|&i| {
                let session = &self.sessions[i];
                query.is_empty()
                    || session.title.to_lowercase().contains(&query)
                    || session.cwd.as_deref().is_some_and(|c| c.to_lowercase().contains(&query))
                    || session.id.to_lowercase().starts_with(&query)
                    || hits.is_some_and(|h| h.contains(&session.path))
            })
            .collect();
        // Stable sort keeps recency order within favorites and non-favorites.
        indices.sort_by_key(|&i| !favorites.contains(&session_key(&self.sessions[i])));
        indices
    }

    fn render_sessions_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let filter_chip = |id: &'static str, label: &str, filter: SessionFilter, cx: &mut Context<Self>| {
            chip(
                id,
                label.to_string(),
                self.session_filter == filter,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.session_filter = filter;
                    cx.notify();
                }),
            )
        };
        let chips = div()
            .flex_shrink_0()
            .flex()
            .gap_1()
            .px_3()
            .pb_2()
            .flex_wrap()
            .child(filter_chip("filter-all", t(cx, "filter.all"), SessionFilter::All, cx))
            // One chip per agent that has local sessions.
            .children(
                Agent::ALL
                    .into_iter()
                    .filter(|a| self.sessions.iter().any(|s| s.agent == *a))
                    .map(|agent| filter_chip(agent.id(), agent.short_name(), SessionFilter::Only(agent), cx)),
            );
        let searching = self.session_content_hits.as_ref().is_none_or(|(q, _)| *q != self.session_query(cx))
            && self.session_query(cx).chars().count() >= 2;
        let search = div()
            .flex_shrink_0()
            .px_3()
            .pb_2()
            // Same fix as the workspace search box: without this, a click focuses the field (it
            // is the deeper element) and then bubbles up to the sidebar's own mouse-down handler,
            // which immediately hands focus back to the list.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(hex(Chrome::BORDER))
                    .bg(hex(0x1a1a1a))
                    .child(icon("search", IconSize::INLINE, hex(Chrome::MUTED)))
                    .child(div().flex_1().min_w_0().t_body().text_color(hex(Chrome::BRIGHT)).child(self.session_search.clone()))
                    .when(searching, |d| d.child(div().t_caption().text_color(hex(Chrome::MUTED)).child("…"))),
            );

        let visible = self.visible_sessions(cx);
        let body: AnyElement = if self.sessions_loading && self.sessions.is_empty() {
            hint(t(cx, "sessions.scanning")).into_any_element()
        } else if visible.is_empty() {
            hint(t(cx, "sessions.empty")).into_any_element()
        } else {
            // Virtualized: only rows in view are built, so long histories scroll smoothly.
            let handle = self.sessions_scroll.clone();
            let base = handle.0.borrow().base_handle.clone();
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    gpui::uniform_list(
                        "sessions-list",
                        visible.len(),
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            let rows = this.visible_sessions(cx);
                            range.filter_map(|i| rows.get(i).copied()).map(|index| this.render_session_row(index, cx)).collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(handle)
                    .size_full()
                    .px_2(),
                )
                .group(crate::ui::SCROLL_GROUP)
                .child(crate::ui::scrollbar(base))
                .into_any_element()
        };
        div().size_full().flex().flex_col().child(search).child(chips).child(body)
    }

    fn render_session_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let session = &self.sessions[index];
        let now = now_ms();
        let to = other_agent(session.agent);
        let migrate_from = session.agent;
        let cwd = session.cwd.as_ref().map(|c| tilde(std::path::Path::new(c))).unwrap_or_default();
        let key = session_key(session);
        let favorite = settings(cx).favorite_sessions.contains(&key);
        let (resume, migrate, view, delete) = (session.clone(), session.clone(), session.clone(), session.clone());
        // Already open in this window: continuing it again would only make a second copy.
        let open_here = self.pane_for_session(&session.id, cx).is_some();
        let selected = self.session_viewer.as_ref().is_some_and(|v| v.session.path == session.path);
        let summary = super::resume_hint::session_summary(session);
        div()
            .id(("session", index))
            .group("session-row")
            .h(px(SESSION_ROW_HEIGHT))
            .px_3()
            .py_2()
            .mb_1()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |d| d.bg(hex(Chrome::SELECTED)))
            .when(!selected, |d| d.hover(|s| s.bg(hex(Chrome::HOVER))))
            // A click shows the conversation; the buttons below continue it.
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_session_viewer(view.clone(), cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(crate::brand::avatar(session.agent.id(), 16.))
                    .child(div().flex_1().min_w_0().truncate().t_body().text_color(hex(Chrome::FOREGROUND)).child(session.title.clone()))
                    .child(div().flex_shrink_0().t_small().text_color(hex(Chrome::MUTED)).child(relative_time(now, session.updated_at)))
                    .child(
                        div()
                            .id(("session-favorite", index))
                            .flex_shrink_0()
                            .size(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .when(!favorite, |d| d.invisible().group_hover("session-row", |s| s.visible()))
                            .hover(|s| s.bg(hex_alpha(0xffffff, 0.08)))
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                let key = key.clone();
                                crate::settings::update_settings(cx, move |s| {
                                    if let Some(i) = s.favorite_sessions.iter().position(|k| k == &key) {
                                        s.favorite_sessions.remove(i);
                                    } else {
                                        s.favorite_sessions.insert(0, key);
                                    }
                                });
                            }))
                            .child(icon("star", IconSize::INLINE, hex(if favorite { Chrome::FAVORITE } else { Chrome::MUTED }))),
                    ),
            )
            .child(div().pl(px(24.)).truncate().t_small().text_color(hex(Chrome::MUTED)).child(cwd))
            .child(div().pl(px(24.)).truncate().t_small().text_color(hex(0xa8a8a8)).child(summary.unwrap_or_default()))
            .child(
                div()
                    .pl(px(24.))
                    .pt_1()
                    .flex()
                    .gap_1()
                    .items_center()
                    .child(action_button(
                        ("session-resume", index),
                        if open_here { t(cx, "sessions.go_to") } else { t(cx, "sessions.resume") },
                        cx.listener(move |this, _: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            this.resume_session(&resume, window, cx)
                        }),
                    ))
                    // Handing the conversation to the other agent: an icon pair, with a question
                    // before anything runs — it starts a session and costs tokens.
                    .child(
                        div()
                            .id(("session-migrate", index))
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_1p5()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(hex(Chrome::HOVER)))
                            .tooltip(crate::ui::Tooltip::text(tf(cx, "sessions.migrate", &[("name", to.display_name())]), None))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.ask_migrate_session(migrate.clone(), cx);
                            }))
                            .child(crate::brand::avatar(migrate_from.id(), 14.))
                            .child(icon("arrow-right", IconSize::INLINE, hex(Chrome::MUTED)))
                            .child(crate::brand::avatar(to.id(), 14.)),
                    )
                    .child(div().flex_1())
                    // A session running in a pane cannot be deleted: the agent is still writing to
                    // the file, and the pane would be left pointing at nothing.
                    .children((!open_here).then(|| {
                        div()
                            .id(("session-delete", index))
                            .size(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .invisible()
                            .group_hover("session-row", |s| s.visible())
                            .hover(|s| s.bg(hex_alpha(Chrome::ERROR, 0.18)))
                            .tooltip(crate::ui::Tooltip::text(t(cx, "sessions.delete"), None))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.ask_delete_session(delete.clone(), cx);
                            }))
                            .child(icon("trash-2", IconSize::INLINE, hex(Chrome::MUTED)))
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // Tabs take the room they need (scrolling when crowded); the spacer gets the rest.
        let mut tabs = div().id("tabs").flex().flex_shrink().min_w_0().h_full().overflow_x_scroll();
        // The start page is always the first tab, whatever else is open: it is how everything else
        // is reached, and hunting for it depended on which screen you happened to be on.
        let home_active = self.welcome && self.page.is_none();
        tabs = tabs.child(
            div()
                .id("page-tab-home")
                .h_full()
                .flex()
                .flex_shrink_0()
                .items_center()
                .px_3()
                .border_t_1()
                .border_r_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(if home_active { Chrome::EDITOR } else { Chrome::TAB_INACTIVE }))
                .cursor_pointer()
                .hover(|s| s.bg(hex(Chrome::EDITOR)))
                .tooltip(crate::ui::Tooltip::text(t(cx, "welcome.open"), None))
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.open_welcome(window, cx)))
                .child(icon("house", IconSize::INLINE, hex(if home_active { Chrome::BRIGHT } else { Chrome::MUTED }))),
        );
        if let Some(page) = self.page {
            // Monitoring: AI usage, AI processes, the capture proxy and what the agents can use
            // (skills, subagents, MCP servers) are tabs of one page. The extension tabs carry the
            // category they open, since they are all the same page underneath.
            let category = self.extensions_category(cx);
            let monitoring = || {
                let mut tabs = vec![
                    (Page::Usage, None, t(cx, "page.usage")),
                    (Page::Processes, None, t(cx, "page.processes")),
                    (Page::Proxy, None, t(cx, "page.proxy")),
                    (Page::Extensions, Some("skills"), t(cx, "ext.skills")),
                    (Page::Extensions, Some("agents"), t(cx, "ext.agents")),
                    (Page::Extensions, Some("mcp"), t(cx, "ext.mcp")),
                ];
                // Everything else the extensions page can show (all of them, commands, plugins,
                // connectors) keeps one tab of its own, so no category is left unreachable.
                if page == Page::Extensions && !matches!(category, "skills" | "agents" | "mcp") {
                    tabs.push((Page::Extensions, Some(category), t(cx, "page.extensions")));
                }
                tabs
            };
            let pages: Vec<(Page, Option<&'static str>, &str)> = match page {
                Page::Usage | Page::Processes | Page::Proxy | Page::Extensions => monitoring(),
                Page::Git => vec![(page, None, t(cx, "page.git"))],
                Page::Flow => vec![(page, None, t(cx, "page.flow"))],
                Page::Settings => vec![(page, None, t(cx, "page.settings"))],
                Page::Plugins => vec![(page, None, t(cx, "page.plugins"))],
                Page::Idea => vec![(page, None, t(cx, "page.idea"))],
                Page::Database => vec![(page, None, t(cx, "page.database"))],
            };
            for (tab_page, tab_category, label) in pages {
                let active = tab_page == page && tab_category.is_none_or(|c| c == category);
                tabs = tabs.child(
                    div()
                        .id(SharedString::from(format!("page-tab-{label}")))
                        .h_full()
                        .flex()
                        .items_center()
                        .gap_2()
                        .pl_3()
                        .pr(if active { px(8.) } else { px(12.) })
                        .border_t_1()
                        .border_r_1()
                        .border_color(hex(Chrome::BORDER))
                        .t_body()
                        .when(active, |d| d.bg(hex(Chrome::EDITOR)).text_color(hex(Chrome::BRIGHT)))
                        .when(!active, |d| {
                            d.bg(hex(Chrome::TAB_INACTIVE))
                                .text_color(hex(Chrome::MUTED))
                                .cursor_pointer()
                                .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| match tab_category {
                                    Some(category) => this.open_extensions(category, window, cx),
                                    None => {
                                        this.page = Some(tab_page);
                                        cx.notify();
                                    }
                                }))
                        })
                        .child(label.to_string())
                        .when(active, |d| {
                            d.child(icon_only(
                                "page-close",
                                "x",
                                cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.page = None;
                                    this.focus_active(window, cx);
                                    cx.notify();
                                }),
                            ))
                        }),
                );
            }
        } else if let Some(ws) = self.workspaces.get(self.active_workspace) {
            // While a file is shown in the editor, its tab is the active one.
            let editing = self.editor_visible(cx);
            for (index, tab) in ws.tabs.iter().enumerate() {
                let view = tab.active.read(cx);
                let active = index == ws.active_tab && !editing;
                let leaves = tab.root.leaves();
                let attention = leaves.iter().any(|p| p.read(cx).attention);
                // Session links in this tab (from the Session Flow), shown as a link mark.
                let ids: Vec<u64> = leaves.iter().map(|p| p.read(cx).pane_id).collect();
                let linked =
                    self.flow.edges().iter().filter(|e| ids.contains(&e.from) || ids.contains(&e.to)).map(|e| e.live).reduce(|a, b| a || b);
                tabs = tabs.child(
                    div()
                        .id(("tab", index))
                        .group("tab")
                        .h_full()
                        .flex()
                        .items_center()
                        .gap_2()
                        .pl_3()
                        .pr_1()
                        .max_w(px(240.))
                        .flex_shrink_0()
                        .cursor_pointer()
                        .border_r_1()
                        .border_color(hex(Chrome::BORDER))
                        .bg(if active { hex(Chrome::EDITOR) } else { hex(Chrome::TAB_INACTIVE) })
                        .when(active, |d| d.border_t_1().border_color(hex(Chrome::ACCENT)))
                        .t_body()
                        .text_color(if active { hex(Chrome::BRIGHT) } else { hex(Chrome::MUTED) })
                        .on_mouse_down(
                            MouseButton::Middle,
                            cx.listener(move |this, _, window, cx| this.request_close_tabs(&[index], window, cx)),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.open_tab_menu(index, event.position, cx);
                            }),
                        )
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.activate_tab(index, window, cx)))
                        // Drag a tab onto another to reorder, or onto a workspace in the sidebar to move it.
                        .on_drag(DraggedTab { workspace: ws.id, index, title: view.display_title().into() }, move |dragged, _, _, cx| {
                            super::drop_split::note_tab_drag(dragged.workspace, dragged.index);
                            cx.new(|_| DragPreview { title: dragged.title.clone() })
                        })
                        // Holding a dragged tab over another one opens it, so the tab can be dropped
                        // into the split of that tab (dropping on the tab itself merges it too).
                        .on_drag_move(cx.listener(move |this, event: &gpui::DragMoveEvent<DraggedTab>, window, cx| {
                            if event.bounds.contains(&event.event.position) {
                                this.preview_tab_during_drag(index, window, cx);
                            }
                        }))
                        .drag_over::<DraggedTab>(move |style, dragged, _, _| {
                            if dragged.index == index {
                                style
                            } else if dragged.index > index {
                                style.border_l_2().border_color(hex(Chrome::ACCENT))
                            } else {
                                style.border_r_2().border_color(hex(Chrome::ACCENT))
                            }
                        })
                        .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                            // Dropped on a tab of another workspace, or on itself: just reorder.
                            if this.workspaces.get(this.active_workspace).is_some_and(|ws| ws.id == dragged.workspace) {
                                this.reorder_tab(dragged.workspace, dragged.index, index, cx)
                            } else {
                                this.move_tab_into_tab(dragged, index, window, cx)
                            }
                        }))
                        // No logo here: the tab strip sits right under the workspace card that
                        // already says which agent this is, and a row of them only drew the eye.
                        // What is left is the one thing a tab alone can say — it is working.
                        .when(view.status.in_turn(), |d| {
                            d.child(crate::ui::dot_spinner(("tab-working", view.pane_id as usize), 12., hex_alpha(Chrome::BRIGHT, 0.9)))
                        })
                        .child(div().truncate().child(view.display_title()))
                        .when(leaves.len() > 1, |d| {
                            d.child(div().t_small().text_color(hex(Chrome::MUTED)).child(format!("⊞{}", leaves.len())))
                        })
                        .when_some(linked, |d, live| d.child(icon("link", 12., hex(if live { Chrome::ATTENTION } else { Chrome::BLUE }))))
                        .when(attention, |d| d.child(div().size(px(7.)).rounded_full().bg(hex(Chrome::ATTENTION))))
                        .child(
                            div()
                                .id(("tab-close", index))
                                .size(px(20.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .when(!active, |d| d.invisible().group_hover("tab", |s| s.visible()))
                                .hover(|s| s.bg(hex_alpha(0xffffff, 0.1)))
                                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    this.request_close_tabs(&[index], window, cx);
                                }))
                                .child(icon("x", IconSize::INLINE, hex(Chrome::FOREGROUND))),
                        ),
                );
            }
        }

        // Files open in the editor, after the terminals.
        if self.page.is_none() {
            tabs = tabs.children(self.render_file_tabs(cx));
        }

        let _header_button =
            |id: &'static str, glyph: &'static str, label: String, active: bool, on_click: WindowAction, cx: &mut Context<Self>| {
                div()
                    .id(id)
                    .h_full()
                    .px_2p5()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .child(icon(glyph, IconSize::BUTTON, hex(if active { Chrome::BRIGHT } else { Chrome::FOREGROUND })))
                    .t_small()
                    .text_color(if active { hex(Chrome::BRIGHT) } else { hex(Chrome::FOREGROUND) })
                    .when(active, |d| d.bg(hex(Chrome::SELECTED)))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| on_click(this, window, cx)))
                    .when(!label.is_empty(), |d| d.child(label))
            };

        // The control the onboarding tour asks for gets a pulsing ring.
        let target = self.tour_target();
        let ringed = |button: gpui::Stateful<gpui::Div>, id: &'static str| {
            if target == Some(id) {
                button.child(crate::ui::pulse_ring(id, false))
            } else {
                button
            }
        };

        div()
            .id("tab-strip")
            .h(px(TAB_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .pr_1()
            .gap_0p5()
            .bg(hex(Chrome::TAB_INACTIVE))
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .child(tabs)
            // New tab / agent / workspace: right after the last tab, and pinned at the end of the
            // strip once the tabs scroll.
            .child(
                div()
                    .id("launcher-toggle")
                    .flex_shrink_0()
                    .ml_1()
                    .size(px(crate::ui::ICON_BUTTON))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .when(self.launcher_open, |d| d.bg(hex(Chrome::SELECTED)))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .tooltip(crate::ui::Tooltip::text(t(cx, "tooltip.new"), Some("⌘T")))
                    .child(icon("plus", IconSize::BUTTON, hex(Chrome::FOREGROUND)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                        if this.just_dismissed("launcher") {
                            return;
                        }
                        this.launcher_open = !this.launcher_open;
                        this.launcher_target = LaunchTarget::NewTab;
                        // The menu opens below the button.
                        let position = event.position();
                        this.launcher_at = Some(gpui::point(position.x - px(12.), position.y + px(20.)));
                        this.notices_open = false;
                        if this.launcher_open {
                            this.detect_agents(cx);
                        }
                        cx.notify();
                    })),
            )
            // Empty strip space: double-click opens a new tab, like VS Code.
            .child(
                div()
                    .id("tab-strip-empty")
                    .flex_1()
                    .min_w(px(24.))
                    .h_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            if event.click_count == 2 {
                                this.request_launch(PaneKind::Shell, LaunchTarget::NewTab, window, cx);
                            }
                        }),
                    )
                    // Right-click: the same menu as the + button, where the click was.
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.launcher_at = Some(event.position);
                            this.launcher_open = true;
                            this.launcher_target = LaunchTarget::NewTab;
                            this.notices_open = false;
                            this.detect_agents(cx);
                            cx.notify();
                        }),
                    ),
            )
            // Plugins with a panel, then cmux-style quick actions: icons only.
            .children(self.render_plugin_header_buttons(cx))
            .child(ringed(
                header_icon(
                    "header-browser",
                    "globe",
                    self.browser.is_some(),
                    (t(cx, "tooltip.browser"), Some("⇧⌘B")),
                    cx.listener(|this, _: &ClickEvent, window, cx| this.toggle_browser(window, cx)),
                ),
                "header-browser",
            ))
            .when(crate::platform::HAS_MINI_MODE, |d| {
                d.child(ringed(
                    header_icon(
                        "header-mini",
                        "picture-in-picture-2",
                        false,
                        (t(cx, "mini.enter"), Some("⌃⌘M")),
                        cx.listener(|this, _: &ClickEvent, window, cx| this.toggle_mini(window, cx)),
                    ),
                    "header-mini",
                ))
            })
            .child(ringed(
                header_icon(
                    "header-split-right",
                    "columns-2",
                    false,
                    (t(cx, "split.right"), Some("⌘D")),
                    cx.listener(|this, _: &ClickEvent, window, cx| this.split(super::Axis::Horizontal, window, cx)),
                ),
                "header-split-right",
            ))
            .child(header_icon(
                "header-split-down",
                "rows-2",
                false,
                (t(cx, "split.down"), Some("⇧⌘D")),
                cx.listener(|this, _: &ClickEvent, window, cx| this.split(super::Axis::Vertical, window, cx)),
            ))
            // The files panel docks at the right edge, so its button is the last one. A tree, not a
            // framed panel: next to the split buttons a frame reads as one more way to split.
            .child(ringed(
                header_icon(
                    "header-files",
                    "list-tree",
                    self.files_panel.is_some(),
                    (t(cx, "files.title"), Some("⌥⌘B")),
                    cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_files_panel(cx)),
                ),
                "header-files",
            ))
    }

    pub(super) fn render_launcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let entry = |id: SharedString,
                     logo: Option<&'static str>,
                     label: String,
                     shortcut: &'static str,
                     action: WindowAction,
                     cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1p5()
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(hex(Chrome::ACCENT)))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| action(this, window, cx)))
                // The launcher is where a tool is chosen: its colour is the point here.
                .child(match logo {
                    Some(logo) => crate::brand::avatar_brand(logo, 18.),
                    None => div().flex_shrink_0().size(px(18.)).flex().items_center().justify_center().child(icon(
                        "square-plus",
                        IconSize::INLINE,
                        hex(Chrome::MUTED),
                    )),
                })
                .child(div().flex_1().min_w_0().truncate().t_body().child(label))
                .child(div().t_small().text_color(hex(Chrome::MUTED)).child(crate::keymap::display(shortcut).into_owned()))
        };
        let installed = self.installed.clone().unwrap_or_default();
        let detected = self.installed.is_some();

        let mut menu = popover()
            .id("launcher")
            .w(px(300.))
            .max_h(px(560.))
            .overflow_y_scroll()
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if std::mem::take(&mut this.launcher_open) {
                    this.note_dismissed("launcher");
                }
                cx.notify();
            }))
            // Where what is picked below opens. A split starts in the tab's folder — and an agent
            // there gets its own working tree when another one already works in the project.
            .child({
                let has_tab = self.active_pane().is_some();
                let target = |id: &'static str, glyph: &'static str, label: &'static str, value: LaunchTarget, cx: &mut Context<Self>| {
                    let active = self.launcher_target == value;
                    div()
                        .id(id)
                        .flex_1()
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .rounded_md()
                        .cursor_pointer()
                        .t_small()
                        .text_color(hex(if active { Chrome::BRIGHT } else { Chrome::MUTED }))
                        .when(active, |d| d.bg(hex(Chrome::SELECTED)))
                        .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                        .child(icon(glyph, 12., hex(if active { Chrome::BRIGHT } else { Chrome::MUTED })))
                        .child(t(cx, label))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.launcher_target = value;
                            cx.notify();
                        }))
                };
                div()
                    .mx_2()
                    .mb_1()
                    .p_0p5()
                    .flex()
                    .gap_0p5()
                    .rounded_md()
                    .bg(hex(Chrome::PANEL))
                    .child(target("launch-target-tab", "plus", "launcher.target_tab", LaunchTarget::NewTab, cx))
                    .when(has_tab, |d| {
                        d.child(target("launch-target-right", "columns-2", "launcher.target_right", LaunchTarget::SplitRight, cx))
                            .child(target("launch-target-down", "rows-2", "launcher.target_down", LaunchTarget::SplitDown, cx))
                    })
            })
            .child(entry(
                "launch-shell".into(),
                Some("shell"),
                t(cx, "welcome.terminal").into(),
                "⌘T",
                Box::new(|this, w, cx| this.request_launch(PaneKind::Shell, this.launcher_target, w, cx)),
                cx,
            ));
        if self.is_installed("claude") {
            menu = menu.child(entry(
                "launch-claude".into(),
                Some("claude"),
                "Claude Code".into(),
                "⌥⌘C",
                Box::new(|this, w, cx| this.request_launch(PaneKind::Claude, this.launcher_target, w, cx)),
                cx,
            ));
        }
        if self.is_installed("codex") {
            menu = menu.child(entry(
                "launch-codex".into(),
                Some("codex"),
                "Codex".into(),
                "⌥⌘X",
                Box::new(|this, w, cx| this.request_launch(PaneKind::Codex, this.launcher_target, w, cx)),
                cx,
            ));
        }
        let agent_entry = |agent: &'static crate::agents::AgentCli, cx: &mut Context<Self>| {
            let (title, command) = (agent.name.to_string(), agent.binary.to_string());
            entry(
                SharedString::from(format!("launch-agent-{}", agent.id)),
                Some(agent.id),
                agent.name.into(),
                "",
                Box::new(move |this, w, cx| {
                    this.request_launch(
                        LaunchChoice::Command { title: title.clone(), command: command.clone() },
                        this.launcher_target,
                        w,
                        cx,
                    )
                }),
                cx,
            )
        };
        for agent in installed.other_agents().filter(|a| a.primary) {
            menu = menu.child(agent_entry(agent, cx));
        }
        let more: Vec<_> = installed.other_agents().filter(|a| !a.primary).collect();
        if !more.is_empty() || !installed.ollama_models.is_empty() {
            let open = self.launcher_more;
            menu = menu.child(
                div()
                    .id("launch-more")
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .child(icon(if open { "chevron-down" } else { "chevron-right" }, IconSize::INLINE, hex(Chrome::MUTED)))
                    .child(div().flex_1().t_body().text_color(hex(Chrome::FOREGROUND)).child(t(cx, "launcher.more_models")))
                    .child(
                        div()
                            .t_small()
                            .text_color(hex(Chrome::MUTED))
                            .child((more.len() + installed.ollama_models.len().min(8)).to_string()),
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.launcher_more = !this.launcher_more;
                        cx.notify();
                    })),
            );
            if open {
                let mut list = div().pl_4().flex().flex_col();
                for agent in more {
                    list = list.child(agent_entry(agent, cx));
                }
                for model in installed.ollama_models.iter().take(8) {
                    let (title, command) = (format!("Ollama · {model}"), format!("ollama run {}", crate::launch::shell_quote(model)));
                    list = list.child(entry(
                        SharedString::from(format!("launch-ollama-{model}")),
                        Some("ollama"),
                        format!("Ollama · {model}"),
                        "",
                        Box::new(move |this, w, cx| {
                            this.request_launch(
                                LaunchChoice::Command { title: title.clone(), command: command.clone() },
                                this.launcher_target,
                                w,
                                cx,
                            )
                        }),
                        cx,
                    ));
                }
                menu = menu.child(list);
            }
        }
        if detected && !installed.has("claude") && !installed.has("codex") && installed.other_agents().next().is_none() {
            menu = menu.child(hint(t(cx, "launcher.no_agents")));
        }
        let menu = menu
            .child(div().my_1().h(px(1.)).bg(hex(Chrome::OVERLAY_BORDER)))
            .child(entry(
                "launch-workspace".into(),
                None,
                t(cx, "new.workspace").into(),
                "⌘N",
                Box::new(|this, w, cx| this.open_new_workspace_page(w, cx)),
                cx,
            ))
            // Last: what opens something new comes first, putting something back comes after it.
            .children(self.render_reopen_tabs(cx));
        // Right-clicked on the tab strip: open where the click was; else under the + button.
        match self.launcher_at {
            Some(position) => gpui::deferred(
                gpui::anchored().position(position).snap_to_window_with_margin(px(8.)).child(crate::ui::fade_in("launcher-fade", menu)),
            )
            .with_priority(3)
            .into_any_element(),
            None => div()
                .absolute()
                .top(px(TAB_HEIGHT + 4.))
                .right(px(8.))
                .child(gpui::deferred(crate::ui::fade_in("launcher-fade", menu)).with_priority(3))
                .into_any_element(),
        }
    }

    /// "Group: none / A / B" chips on the new-workspace page (only when groups exist).
    fn render_group_choice(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.groups.is_empty() {
            return None;
        }
        let chip = |id: SharedString, label: String, selected: bool, group: Option<u64>, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded_md()
                .border_1()
                .t_small()
                .cursor_pointer()
                .border_color(hex(if selected { Chrome::ACCENT } else { Chrome::BORDER }))
                .bg(if selected { hex_alpha(Chrome::ACCENT, 0.2) } else { hex_alpha(0, 0.) })
                .text_color(hex(if selected { Chrome::BRIGHT } else { Chrome::FOREGROUND }))
                .hover(|s| s.border_color(hex_alpha(Chrome::ACCENT, 0.7)))
                .child(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.new_workspace_group = group;
                    cx.notify();
                }))
        };
        let mut row = div().pt_1().flex().flex_wrap().items_center().gap_1();
        row = row.child(div().t_small().text_color(hex(Chrome::MUTED)).mr_1().child(t(cx, "welcome.group_label")));
        row = row.child(chip("new-ws-group-none".into(), t(cx, "ungrouped").to_string(), self.new_workspace_group.is_none(), None, cx));
        for group in &self.groups {
            row = row.child(chip(
                SharedString::from(format!("new-ws-group-{}", group.id)),
                group.name.clone(),
                self.new_workspace_group == Some(group.id),
                Some(group.id),
                cx,
            ));
        }
        Some(row)
    }

    /// First-run screen, and the start page of a new workspace (with a name field and cancel):
    /// what to start, what ran recently, and what else Agentty does.
    pub(super) fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let starting = self.new_workspace.as_ref().map(|(input, _)| input.clone());
        let tile = |id: SharedString, logo: &'static str, label: String, body: String| {
            div()
                .id(id)
                .flex_1()
                .min_w(px(WELCOME_CARD_MIN_WIDTH))
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .rounded_lg()
                .cursor_pointer()
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(Chrome::OVERLAY))
                .hover(|s| s.bg(hex(Chrome::HOVER)).border_color(hex_alpha(Chrome::ACCENT, 0.7)))
                .child(crate::brand::tile(logo, 36.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(div().truncate().t_body().font_weight(FontWeight::MEDIUM).text_color(hex(Chrome::BRIGHT)).child(label))
                        .child(div().truncate().t_small().text_color(hex(Chrome::MUTED)).child(body)),
                )
        };
        let card = |id: SharedString, logo: &'static str, label: String, body: String, choice: LaunchChoice, cx: &mut Context<Self>| {
            tile(id, logo, label, body).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.request_launch(choice.clone(), LaunchTarget::NewWorkspace, window, cx)
            }))
        };
        // Claude Code and Codex are what Agentty is for: they are always offered, and picking one
        // that isn't installed explains how to get it — and only that: click listeners add up, so
        // this card must not carry the launch listener as well. Other CLIs only show up once they
        // are there.
        let missing = |id: &'static str, name: &'static str, url: &'static str, cx: &mut Context<Self>| {
            let not_installed = t(cx, "welcome.not_installed").to_string();
            tile(SharedString::from(format!("welcome-{id}")), id, name.to_string(), not_installed).on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    this.install_hint = Some((id, name, url));
                    cx.notify();
                },
            ))
        };
        let mut cards = div().w_full().flex().flex_wrap().gap_3();
        let terminal_body = t(cx, "welcome.terminal_body").to_string();
        cards =
            cards.child(card("welcome-shell".into(), "shell", t(cx, "welcome.terminal").into(), terminal_body, PaneKind::Shell.into(), cx));
        cards = cards.child(if self.is_installed("claude") {
            let body = tf(cx, "welcome.maker_body", &[("maker", "Anthropic")]);
            card("welcome-claude".into(), "claude", "Claude Code".into(), body, PaneKind::Claude.into(), cx)
        } else {
            missing("claude", "Claude Code", CLAUDE_INSTALL_URL, cx)
        });
        cards = cards.child(if self.is_installed("codex") {
            let body = tf(cx, "welcome.maker_body", &[("maker", "OpenAI")]);
            card("welcome-codex".into(), "codex", "Codex".into(), body, PaneKind::Codex.into(), cx)
        } else {
            missing("codex", "Codex", CODEX_INSTALL_URL, cx)
        });
        for agent in self.installed.iter().flat_map(|i| i.other_agents()).filter(|a| a.primary) {
            let choice = LaunchChoice::Command { title: agent.name.to_string(), command: agent.binary.to_string() };
            let body = if agent.maker.is_empty() {
                t(cx, "welcome.agent_body").to_string()
            } else {
                tf(cx, "welcome.maker_body", &[("maker", agent.maker)])
            };
            cards =
                cards.child(card(SharedString::from(format!("welcome-{}", agent.id)), agent.id, agent.name.to_string(), body, choice, cx));
        }
        // Invisible cards keep the last row on the same columns as the rows above it. They carry a
        // card's padding and border width too: free space is shared on top of those.
        for _ in 0..3 {
            cards = cards.child(div().flex_1().min_w(px(WELCOME_CARD_MIN_WIDTH)).h_0().px(px(17.)));
        }
        let more = self.installed.as_ref().is_some_and(|i| i.other_agents().any(|a| !a.primary) || !i.ollama_models.is_empty());
        let quiet_button = |id: &'static str, label: String| {
            div()
                .id(id)
                .flex_shrink_0()
                .px_3()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .t_small()
                .text_color(hex(Chrome::MUTED))
                .hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT)))
                .child(label)
        };
        let header = div()
            .w_full()
            .flex()
            .items_center()
            .gap_4()
            .child(gpui::img("brand/logo.png").size(px(52.)).flex_shrink_0())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(26.))
                            .font_weight(FontWeight::LIGHT)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(if starting.is_some() { t(cx, "new.workspace").to_string() } else { "Agentty".to_string() }),
                    )
                    .child(div().truncate().t_body().text_color(hex(Chrome::MUTED)).child(t(cx, "tagline"))),
            )
            .when(starting.is_some() && !self.workspaces.is_empty(), |d| {
                d.child(quiet_button("welcome-cancel", t(cx, "confirm.cancel").to_string()).on_click(cx.listener(
                    |this, _: &ClickEvent, window, cx| {
                        this.new_workspace = None;
                        this.focus_active(window, cx);
                        cx.notify();
                    },
                )))
            })
            // Opened from the sidebar with workspaces already running: a way back to them.
            .when(self.welcome && starting.is_none() && !self.workspaces.is_empty(), |d| {
                d.child(
                    quiet_button("welcome-back", t(cx, "welcome.back").to_string())
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.close_welcome(window, cx))),
                )
            });
        let heading = |label: String| div().t_body().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::FOREGROUND)).child(label);
        // A short window scrolls rather than stacking the page on top of its own footer.
        let page = div()
            .w_full()
            .max_w(px(WELCOME_WIDTH))
            .mx_auto()
            .flex()
            .flex_col()
            .gap_6()
            .px_6()
            .pt(px(48.))
            .pb_8()
            // Something important is missing: say so above everything else.
            .when(starting.is_none(), |d| d.children(self.render_setup_banner(cx)))
            .child(header)
            .when_some(starting.clone(), |d, input| {
                d.child(
                    div()
                        .w(px(360.))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "welcome.name_label")))
                        .child(
                            div()
                                .px_2()
                                .py_1p5()
                                .rounded_md()
                                .border_1()
                                .border_color(hex(Chrome::ACCENT))
                                .bg(hex(0x1a1a1a))
                                .t_body()
                                .text_color(hex(Chrome::BRIGHT))
                                .child(input),
                        )
                        .children(self.render_group_choice(cx)),
                )
            })
            .when(starting.is_none() && settings(cx).idea_mode, |d| d.child(self.render_idea_card(cx)))
            .child(div().w_full().flex().flex_col().gap_3().child(heading(t(cx, "welcome.start").to_string())).child(cards).when(
                more,
                |d| {
                    d.child(
                        div()
                            .id("welcome-more")
                            .flex()
                            .items_center()
                            .gap_2()
                            .py_1()
                            .cursor_pointer()
                            .t_small()
                            .text_color(hex(Chrome::MUTED))
                            .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                            .child(t(cx, "launcher.more_models"))
                            .child(icon("chevron-down", IconSize::INLINE, hex(Chrome::MUTED)))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.launcher_open = true;
                                this.launcher_more = true;
                                this.detect_agents(cx);
                                cx.notify();
                            })),
                    )
                },
            ))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_wrap()
                    .gap_6()
                    .child(self.render_welcome_recent(heading(t(cx, "welcome.recent").to_string()), cx))
                    .child(self.render_welcome_explore(heading(t(cx, "welcome.explore").to_string()), cx)),
            );
        div()
            .size_full()
            .relative()
            .bg(hex(Chrome::EDITOR))
            .child(
                div()
                    .id("welcome-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.welcome_scroll)
                    // Room for the footer, which sits over the bottom of the page.
                    .pb(px(FOOTER_HEIGHT))
                    .child(page),
            )
            .group(crate::ui::SCROLL_GROUP)
            .child(crate::ui::scrollbar(self.welcome_scroll.clone()))
            .child(self.render_welcome_footer(cx))
    }

    /// The sessions touched last, one click from running again.
    fn render_welcome_recent(&self, heading: gpui::Div, cx: &mut Context<Self>) -> impl IntoElement {
        let now = now_ms();
        let mut list = div().flex().flex_col();
        for (index, session) in self.sessions.iter().take(WELCOME_RECENT).enumerate() {
            let resume = session.clone();
            let folder = session.cwd.as_ref().map(|cwd| tilde(std::path::Path::new(cwd))).unwrap_or_default();
            list = list.child(
                div()
                    .id(("welcome-recent", index))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.resume_session(&resume, window, cx)))
                    .child(crate::brand::avatar(session.agent.id(), 18.))
                    .child(div().flex_1().min_w_0().truncate().t_body().text_color(hex(Chrome::FOREGROUND)).child(session.title.clone()))
                    .child(div().flex_shrink_0().max_w(px(170.)).truncate().t_small().text_color(hex(Chrome::MUTED)).child(folder))
                    .child(
                        div()
                            .flex_shrink_0()
                            .w(px(30.))
                            .flex()
                            .justify_end()
                            .t_small()
                            .text_color(hex(Chrome::MUTED))
                            .child(relative_time(now, session.updated_at)),
                    ),
            );
        }
        div().flex_1().min_w(px(320.)).flex().flex_col().gap_2().child(heading).child(if self.sessions.is_empty() {
            div().px_2().py_1p5().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "welcome.recent_empty")).into_any_element()
        } else {
            list.into_any_element()
        })
    }

    /// What else Agentty does besides terminals, each one click away.
    fn render_welcome_explore(&self, heading: gpui::Div, cx: &mut Context<Self>) -> impl IntoElement {
        let row =
            |id: &'static str, glyph: &'static str, color: u32, label: String, body: String, open: ViewAction, cx: &mut Context<Self>| {
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| open(this, cx)))
                    .child(crate::brand::tinted_tile(color, 28.).child(icon(glyph, 15., hex(color))))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().t_body().text_color(hex(Chrome::FOREGROUND)).child(label))
                            .child(div().truncate().t_small().text_color(hex(Chrome::MUTED)).child(body)),
                    )
            };
        let launch = (t(cx, "launch.menu").to_string(), t(cx, "launch.menu_body").to_string());
        let flow = (t(cx, "page.flow").to_string(), t(cx, "welcome.flow_body").to_string());
        let usage = (t(cx, "page.monitoring").to_string(), t(cx, "welcome.usage_body").to_string());
        let plugins = (t(cx, "page.plugins").to_string(), t(cx, "welcome.plugins_body").to_string());
        div().flex_1().min_w(px(280.)).flex().flex_col().gap_2().child(heading).child(
            div()
                .flex()
                .flex_col()
                .child(row("welcome-launch", "rocket", Chrome::ORANGE, launch.0, launch.1, Box::new(|this, cx| this.open_launch(cx)), cx))
                .child(row(
                    "welcome-flow",
                    "network",
                    Chrome::PURPLE,
                    flow.0,
                    flow.1,
                    Box::new(|this, cx| this.open_page(Page::Flow, cx)),
                    cx,
                ))
                .child(row(
                    "welcome-usage",
                    "zap",
                    Chrome::GREEN,
                    usage.0,
                    usage.1,
                    Box::new(|this, cx| this.open_page(Page::Usage, cx)),
                    cx,
                ))
                .child(row(
                    "welcome-plugins",
                    "puzzle",
                    Chrome::BLUE,
                    plugins.0,
                    plugins.1,
                    Box::new(|this, cx| this.open_plugins_page(None, cx)),
                    cx,
                )),
        )
    }

    /// Start page card for "Build my idea": the one thing on the page that is not a terminal, so it
    /// gets the full width — and room for its one-line description.
    fn render_idea_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("welcome-idea")
            .group("welcome-idea")
            .w_full()
            .flex()
            .items_center()
            .gap_4()
            .p_4()
            .rounded_lg()
            .cursor_pointer()
            .border_1()
            .border_color(hex_alpha(Chrome::ORANGE, 0.5))
            .bg(hex_alpha(Chrome::ORANGE, 0.08))
            .hover(|s| s.bg(hex_alpha(Chrome::ORANGE, 0.16)).border_color(hex(Chrome::ORANGE)))
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.open_idea_page(window, cx)))
            .child(crate::brand::tinted_tile(Chrome::ORANGE, 40.).child(icon("lightbulb", 20., hex(Chrome::ORANGE))))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .truncate()
                            .t_title()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(t(cx, "idea.menu")),
                    )
                    .child(div().truncate().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "welcome.idea_body"))),
            )
            .child(div().flex_shrink_0().opacity(0.6).group_hover("welcome-idea", |s| s.opacity(1.)).child(icon(
                "chevron-right",
                IconSize::BUTTON,
                hex(Chrome::ORANGE),
            )))
    }

    /// Links and copyright at the bottom of the start page, like the website's footer.
    fn render_welcome_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let link = |id: &'static str, label: String, url: &'static str| {
            div()
                .id(id)
                .px_1()
                .cursor_pointer()
                .text_color(hex(Chrome::MUTED))
                .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                .child(label)
                .on_click(move |_: &ClickEvent, _, cx| cx.open_url(url))
        };
        div()
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .h(px(FOOTER_HEIGHT))
            .bg(hex(Chrome::EDITOR))
            .flex()
            .items_center()
            .justify_center()
            .gap_3()
            .t_caption()
            .text_color(hex(Chrome::MUTED))
            .child(link("welcome-about", t(cx, "welcome.about").to_string(), super::update::WEBSITE))
            .child(link("welcome-releases", t(cx, "welcome.releases").to_string(), RELEASES_URL))
            .child(link("welcome-github", "GitHub".to_string(), GITHUB_URL))
            .child(div().opacity(0.6).child(format!("© {COPYRIGHT_YEAR} Agentty ·")))
            .child(link("welcome-author", "raylee.app".to_string(), AUTHOR_URL))
    }

    /// Scrolls the workspace list while a card is dragged near its top or bottom edge, and keeps
    /// scrolling while the pointer stays there (drag events stop arriving once it holds still).
    pub(super) fn drag_autoscroll(&mut self, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let bounds = self.sidebar_scroll.bounds();
        let (top, height) = (f32::from(bounds.origin.y), f32::from(bounds.size.height));
        if height <= 0. {
            return;
        }
        let y = f32::from(position.y);
        let delta = if y < top + DRAG_EDGE {
            ((top + DRAG_EDGE - y) / DRAG_EDGE).clamp(0., 1.) * DRAG_SCROLL_STEP
        } else if y > top + height - DRAG_EDGE {
            -((y - (top + height - DRAG_EDGE)) / DRAG_EDGE).clamp(0., 1.) * DRAG_SCROLL_STEP
        } else {
            0.
        };
        self.drag_scroll_delta = delta;
        if delta == 0. {
            self.drag_scroll = None;
            return;
        }
        if self.drag_scroll.is_some() {
            return;
        }
        self.drag_scroll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(DRAG_SCROLL_TICK).await;
                let running = this.update(cx, |this, cx| this.apply_drag_scroll(cx)).unwrap_or(false);
                if !running {
                    break;
                }
            }
            let _ = this.update(cx, |this, _| this.drag_scroll = None);
        }));
    }

    /// One auto-scroll tick; `false` once there is nothing left to scroll or no drag is running.
    fn apply_drag_scroll(&mut self, cx: &mut Context<Self>) -> bool {
        let delta = self.drag_scroll_delta;
        if delta == 0. {
            return false;
        }
        let max = f32::from(self.sidebar_scroll.max_offset().height);
        let offset = self.sidebar_scroll.offset();
        let next = (f32::from(offset.y) + delta).clamp(-max, 0.);
        if next == f32::from(offset.y) {
            return false;
        }
        self.sidebar_scroll.set_offset(gpui::point(offset.x, gpui::px(next)));
        cx.notify();
        true
    }

    pub(super) fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_pane().map(|p| p.read(cx));
        let kind = active.map(|v| match v.display_kind() {
            PaneKind::Shell => t(cx, "status.shell").to_string(),
            PaneKind::Claude => with_version("Claude Code", self.installed.as_ref().and_then(|i| i.version("claude"))),
            PaneKind::Codex => with_version("Codex", self.installed.as_ref().and_then(|i| i.version("codex"))),
        });
        // Where the active terminal is. It is the only place the path is shown now: the pane's own
        // bar had it too, and one of them was always cut short.
        let cwd = active.map(|v| v.display_cwd());
        div()
            .h(px(STATUS_BAR_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_4()
            .px_3()
            .bg(hex(Chrome::STATUS_BAR))
            .border_t_1()
            .border_color(hex(Chrome::BORDER))
            .t_small()
            .text_color(hex(Chrome::MUTED))
            .child(div().text_color(hex(Chrome::SUCCESS)).child(format!("● Agentty v{}", super::update::CURRENT_VERSION)))
            .children(kind)
            .children(self.render_advisor_chip(cx))
            .children(cwd.map(|cwd| {
                let reveal = cwd.clone();
                div()
                    .id("status-folder")
                    .flex_shrink()
                    .min_w_0()
                    .truncate()
                    .cursor_pointer()
                    .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                    .tooltip(crate::ui::Tooltip::text(tilde(&cwd), Some(super::layout::REVEAL_HINT)))
                    .on_click(move |event: &ClickEvent, _, _: &mut gpui::App| {
                        if crate::keymap::link_modifier(&event.modifiers()) {
                            crate::platform::reveal(&reveal);
                        }
                    })
                    // Cut in the middle past 50 characters: a worktree path is mostly its ends, and
                    // the whole thing pushed everything else off the bar.
                    .child(crate::ui::middle_ellipsis(&tilde(&cwd), 50))
            }))
            .child(div().flex_1().min_w_0().truncate().children(self.status.clone()))
            .children(self.render_plugin_status_items(cx))
            .children(self.render_docker_chip(cx))
            .children(self.render_db_chip(cx))
            .children(self.render_status_icons(cx))
    }
}

/// "Claude Code v2.1.274", or just the name while the CLI version is unknown.
fn with_version(name: &str, version: Option<&str>) -> String {
    version.map_or_else(|| name.to_string(), |v| format!("{name} v{v}"))
}

/// Stable identifier for favorites: `claude:<id>` / `codex:<id>`.
fn session_key(session: &agentty_bridge::model::SessionInfo) -> String {
    format!("{}:{}", session.agent.id(), session.id)
}

/// Title of a closed tab (its first pane) and how many panes it had.
fn closed_tab_label(node: &super::persist::NodeSnapshot) -> (String, usize) {
    match node {
        super::persist::NodeSnapshot::Pane(pane) => (pane.title.clone(), 1),
        super::persist::NodeSnapshot::Split { children, .. } => {
            let parts: Vec<(String, usize)> = children.iter().map(closed_tab_label).collect();
            let title = parts.first().map(|(t, _)| t.clone()).unwrap_or_default();
            (title, parts.iter().map(|(_, n)| n).sum())
        }
    }
}

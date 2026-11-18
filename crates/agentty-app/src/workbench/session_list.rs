//! The local sessions as rows of the side panel: under the project they ran in (a worktree with
//! the repository it came from), favorites first, a plugin's own runs and whatever ran in a
//! temporary folder last and folded; and sessions that hold the very same conversation (a script
//! asking the same thing over and over) as one row that says how many there are.

use super::Workbench;
use crate::i18n::{t, tf};
use crate::settings::settings;
use crate::theme::{hex, Chrome};
use crate::ui::{icon, tilde, IconSize, TypeScale};
use agentty_bridge::model::SessionInfo;
use agentty_bridge::session_groups::{GroupKind, SessionGroup};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight, SharedString};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum Heading {
    Favorites,
    Group(SessionGroup),
}

impl Heading {
    pub(super) fn key(&self) -> String {
        match self {
            Heading::Favorites => "favorites".into(),
            Heading::Group(group) => group.key(),
        }
    }

    /// Whether it starts folded: every project does, so the panel opens as a short list of
    /// projects; the favorites are there to be at hand.
    fn starts_folded(&self) -> bool {
        matches!(self, Heading::Group(_))
    }

    /// Runs nobody sat in front of (a plugin's, a script's): after the user's projects.
    fn is_automated(&self) -> bool {
        matches!(self, Heading::Group(group) if group.is_automated())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum SessionRow {
    Heading {
        heading: Heading,
        sessions: usize,
        folded: bool,
    },
    Session {
        /// Index in `Workbench::sessions`.
        index: usize,
        /// The group it is listed under, when the list is by project.
        group: Option<SessionGroup>,
        /// How many more sessions hold the same conversation: this row stands for them.
        alike: usize,
        /// Those are listed after it, one by one.
        alike_shown: bool,
    },
}

pub(super) struct ListInput<'a> {
    pub sessions: &'a [SessionInfo],
    pub groups: &'a [SessionGroup],
    /// What the filter and the search leave, favorites first, newest first.
    pub visible: &'a [usize],
    /// How many of `visible`, from its start, are favorites.
    pub favorites: usize,
    pub by_project: bool,
    /// While searching everything found shows: nothing folded, nothing standing for another.
    pub searching: bool,
    pub toggled: &'a HashSet<String>,
    pub stacks_open: &'a HashSet<String>,
}

/// What one session is remembered by (favorites, opened stacks): `claude:<id>`.
pub(super) fn session_key(session: &SessionInfo) -> String {
    format!("{}:{}", session.agent.id(), session.id)
}

/// What makes two sessions one row: the agent, the place, the title and the last thing said each way.
type SameConversation<'a> = (&'a str, &'a SessionGroup, &'a str, Option<&'a str>, Option<&'a str>);

pub(super) fn rows(input: &ListInput) -> Vec<SessionRow> {
    let unknown = SessionGroup { kind: GroupKind::Unknown, root: None, name: String::new() };
    let group_of = |index: usize| input.groups.get(index).unwrap_or(&unknown);
    let mut buckets: Vec<(Option<Heading>, Vec<usize>)> = Vec::new();
    if input.by_project {
        let (favorites, others) = input.visible.split_at(input.favorites.min(input.visible.len()));
        if !favorites.is_empty() {
            buckets.push((Some(Heading::Favorites), favorites.to_vec()));
        }
        // A project stands where its newest session does.
        let mut at: HashMap<&SessionGroup, usize> = HashMap::new();
        let first_group = buckets.len();
        for &index in others {
            let group = group_of(index);
            let bucket = *at.entry(group).or_insert_with(|| {
                buckets.push((Some(Heading::Group(group.clone())), Vec::new()));
                buckets.len() - 1
            });
            buckets[bucket].1.push(index);
        }
        // Stable: projects keep their order, the automated groups theirs after them.
        buckets[first_group..].sort_by_key(|(heading, _)| heading.as_ref().is_some_and(Heading::is_automated));
    } else {
        buckets.push((None, input.visible.to_vec()));
    }

    let mut rows = Vec::new();
    for (heading, indices) in buckets {
        let group = match &heading {
            Some(Heading::Group(group)) => Some(group.clone()),
            _ => None,
        };
        if let Some(heading) = heading {
            let folded = !input.searching && (heading.starts_folded() != input.toggled.contains(&heading.key()));
            rows.push(SessionRow::Heading { heading, sessions: indices.len(), folded });
            if folded {
                continue;
            }
        }
        // Sessions with the same conversation, in the same place: the newest stands for the rest.
        let mut stacks: Vec<Vec<usize>> = Vec::new();
        let mut stack_at: HashMap<SameConversation, usize> = HashMap::new();
        for &index in &indices {
            let session = &input.sessions[index];
            if input.searching {
                stacks.push(vec![index]);
                continue;
            }
            let same = (
                session.agent.id(),
                group_of(index),
                session.title.as_str(),
                session.last_prompt.as_deref(),
                session.last_reply.as_deref(),
            );
            let stack = *stack_at.entry(same).or_insert_with(|| {
                stacks.push(Vec::new());
                stacks.len() - 1
            });
            stacks[stack].push(index);
        }
        for stack in stacks {
            let alike_shown = stack.len() > 1 && input.stacks_open.contains(&session_key(&input.sessions[stack[0]]));
            rows.push(SessionRow::Session { index: stack[0], group: group.clone(), alike: stack.len() - 1, alike_shown });
            if alike_shown {
                rows.extend(stack[1..].iter().map(|&index| SessionRow::Session {
                    index,
                    group: group.clone(),
                    alike: 0,
                    alike_shown: false,
                }));
            }
        }
    }
    rows
}

impl Workbench {
    pub(super) fn session_rows(&self, cx: &gpui::App) -> Vec<SessionRow> {
        let visible = self.visible_sessions(cx);
        let favorites = &settings(cx).favorite_sessions;
        let favorite_count = visible.iter().take_while(|&&i| favorites.contains(&session_key(&self.sessions[i]))).count();
        rows(&ListInput {
            sessions: &self.sessions,
            groups: &self.session_groups,
            visible: &visible,
            favorites: favorite_count,
            by_project: settings(cx).group_sessions,
            searching: !self.session_query(cx).is_empty(),
            toggled: &self.session_groups_toggled,
            stacks_open: &self.session_stacks_open,
        })
    }

    /// Tells the list's state what it is about to lay out: rows came, went or changed places.
    pub(super) fn sync_session_list(&self, rows: &[SessionRow]) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for row in rows {
            match row {
                SessionRow::Heading { heading, folded, .. } => (0u8, heading.key(), folded).hash(&mut hasher),
                SessionRow::Session { index, group, alike, .. } => {
                    (1u8, session_key(&self.sessions[*index]), group.is_some(), alike).hash(&mut hasher)
                }
            }
        }
        let shape = hasher.finish();
        let listed = self.sessions_list.item_count();
        if self.session_rows_shape.replace(shape) == shape && listed == rows.len() {
            return;
        }
        // The end grows or shrinks and every row is measured again, which keeps the place the
        // list is scrolled to: a refresh in the background must not throw it back to the top.
        if rows.len() >= listed {
            self.sessions_list.splice(listed..listed, rows.len() - listed);
        } else {
            self.sessions_list.splice(rows.len()..listed, 0);
        }
        self.sessions_list.remeasure();
    }

    /// Forgets folds and opened stacks of what is no longer listed.
    pub(super) fn prune_session_list_state(&mut self) {
        let groups: HashSet<String> = self.session_groups.iter().map(SessionGroup::key).chain(["favorites".to_string()]).collect();
        self.session_groups_toggled.retain(|key| groups.contains(key));
        let sessions: HashSet<String> = self.sessions.iter().map(session_key).collect();
        self.session_stacks_open.retain(|key| sessions.contains(key));
    }

    /// Whether any project of the list is unfolded (the favorites aside): what the button that
    /// folds or unfolds them all does next.
    pub(super) fn any_session_group_open(&self) -> bool {
        self.session_groups_toggled.iter().any(|key| key != "favorites")
    }

    /// Folds every project, or unfolds them all. The favorites stay as they are.
    pub(super) fn fold_session_groups(&mut self, fold: bool, cx: &mut Context<Self>) {
        let favorites_folded = self.session_groups_toggled.contains("favorites");
        self.session_groups_toggled.clear();
        if !fold {
            self.session_groups_toggled.extend(self.session_groups.iter().map(SessionGroup::key));
        }
        if favorites_folded {
            self.session_groups_toggled.insert("favorites".into());
        }
        cx.notify();
    }

    pub(super) fn toggle_session_stack(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.session_stacks_open.remove(&key) {
            self.session_stacks_open.insert(key);
        }
        cx.notify();
    }

    pub(super) fn render_session_list_row(&self, row: &SessionRow, cx: &mut Context<Self>) -> AnyElement {
        match row {
            SessionRow::Heading { heading, sessions, folded } => self.render_session_heading(heading, *sessions, *folded, cx),
            SessionRow::Session { index, group, alike, alike_shown } => {
                let Some(session) = self.sessions.get(*index) else { return div().into_any_element() };
                // Under a project the line below the title says only what the heading does not:
                // which worktree or folder of it. Without headings it is the folder, as before.
                let place = match group {
                    None => Some(session.cwd.as_ref().map(|c| tilde(std::path::Path::new(c))).unwrap_or_default()),
                    Some(group) => session.cwd.as_deref().map(std::path::Path::new).and_then(|cwd| match group.root {
                        Some(_) => agentty_bridge::session_groups::place_in_group(cwd, group),
                        None => Some(tilde(cwd)),
                    }),
                };
                div().px_2().child(self.render_session_row(*index, place, *alike, *alike_shown, cx)).into_any_element()
            }
        }
    }

    fn render_session_heading(&self, heading: &Heading, sessions: usize, folded: bool, cx: &mut Context<Self>) -> AnyElement {
        let (glyph, name, path) = match heading {
            Heading::Favorites => ("star", t(cx, "sessions.group_favorites").to_string(), None),
            Heading::Group(group) => match group.kind {
                GroupKind::Project => ("folder", group.name.clone(), group.root.as_deref().map(tilde)),
                GroupKind::Plugin => ("puzzle", group.name.clone(), None),
                GroupKind::Temporary => ("clock", t(cx, "sessions.group_temporary").to_string(), None),
                GroupKind::Unknown => ("folder", t(cx, "sessions.group_unknown").to_string(), None),
            },
        };
        let key = heading.key();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        div()
            .px_2()
            .pt_1()
            .child(
                div()
                    .id(("session-group", hasher.finish()))
                    .h(px(28.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if !this.session_groups_toggled.remove(&key) {
                            this.session_groups_toggled.insert(key.clone());
                        }
                        cx.notify();
                    }))
                    .child(icon(if folded { "chevron-right" } else { "chevron-down" }, IconSize::INLINE, hex(Chrome::MUTED)))
                    .child(icon(glyph, IconSize::INLINE, hex(Chrome::MUTED)))
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(gpui::relative(0.6))
                            .truncate()
                            .t_small()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hex(Chrome::FOREGROUND))
                            .child(SharedString::from(name)),
                    )
                    .child(div().flex_1().min_w_0().truncate().t_caption().text_color(hex(Chrome::MUTED)).child(path.unwrap_or_default()))
                    .child(div().flex_shrink_0().t_caption().text_color(hex(Chrome::MUTED)).child(sessions.to_string())),
            )
            .into_any_element()
    }

    /// The pill on a row that stands for sessions with the same conversation: how many more there
    /// are; a click lists them, another folds them back.
    pub(super) fn render_alike_pill(&self, index: usize, alike: usize, shown: bool, cx: &mut Context<Self>) -> AnyElement {
        let key = session_key(&self.sessions[index]);
        div()
            .id(("session-alike", index))
            .flex_shrink_0()
            .px_1p5()
            .rounded_md()
            .flex()
            .items_center()
            .gap_0p5()
            .t_caption()
            .text_color(hex(Chrome::MUTED))
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .cursor_pointer()
            .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
            .tooltip(crate::ui::Tooltip::text(tf(cx, "sessions.alike", &[("n", &(alike + 1).to_string())]), None))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                this.toggle_session_stack(key.clone(), cx);
            }))
            .child(format!("+{alike}"))
            .child(icon(if shown { "chevron-up" } else { "chevron-down" }, 10., hex(Chrome::MUTED)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentty_bridge::model::Agent;
    use std::path::PathBuf;

    fn session(id: &str, title: &str, exchange: Option<(&str, &str)>) -> SessionInfo {
        SessionInfo {
            agent: Agent::Claude,
            id: id.into(),
            title: title.into(),
            cwd: None,
            updated_at: 0,
            path: PathBuf::from(id),
            resume_args: Vec::new(),
            last_prompt: exchange.map(|(prompt, _)| prompt.into()),
            last_reply: exchange.map(|(_, reply)| reply.into()),
        }
    }

    fn project(name: &str) -> SessionGroup {
        SessionGroup { kind: GroupKind::Project, root: Some(PathBuf::from("/work").join(name)), name: name.into() }
    }

    fn temporary() -> SessionGroup {
        SessionGroup { kind: GroupKind::Temporary, root: None, name: String::new() }
    }

    /// The rows as short words: `[app 2]` a heading and its sessions, `>` folded, `a+2` a session
    /// standing for two more.
    fn drawn(rows: &[SessionRow], sessions: &[SessionInfo]) -> String {
        rows.iter()
            .map(|row| match row {
                SessionRow::Heading { heading, sessions, folded } => {
                    let name = match heading {
                        Heading::Favorites => "favorites".to_string(),
                        Heading::Group(group) if group.kind == GroupKind::Temporary => "temporary".to_string(),
                        Heading::Group(group) => group.name.clone(),
                    };
                    format!("[{name} {sessions}]{}", if *folded { ">" } else { "" })
                }
                SessionRow::Session { index, alike: 0, .. } => sessions[*index].id.clone(),
                SessionRow::Session { index, alike, .. } => format!("{}+{alike}", sessions[*index].id),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn sessions_are_listed_under_their_project_with_scripted_runs_last_and_folded() {
        let sessions = [
            session("a", "fix login", None),
            session("ok1", "ok", Some(("ok", "ok"))),
            session("b", "release notes", None),
            session("ok2", "ok", Some(("ok", "ok"))),
            session("c", "refactor", None),
            session("ok3", "ok", Some(("ok", "ok"))),
        ];
        let groups = [project("app"), temporary(), project("site"), temporary(), project("app"), temporary()];
        let visible: Vec<usize> = (0..sessions.len()).collect();
        let (mut toggled, mut stacks_open) = (HashSet::new(), HashSet::new());
        let list = |by_project, searching, favorites, toggled: &HashSet<String>, stacks_open: &HashSet<String>| {
            let input = ListInput {
                sessions: &sessions,
                groups: &groups,
                visible: &visible,
                favorites,
                by_project,
                searching,
                toggled,
                stacks_open,
            };
            drawn(&rows(&input), &sessions)
        };
        // By project: every project folded, the temporary folder's runs after them.
        assert_eq!(list(true, false, 0, &toggled, &stacks_open), "[app 2]> [site 1]> [temporary 3]>");
        // Unfolded, the three with the same conversation are one row; opened, they are all there.
        toggled.insert("temporary".into());
        assert_eq!(list(true, false, 0, &toggled, &stacks_open), "[app 2]> [site 1]> [temporary 3] ok1+2");
        stacks_open.insert("claude:ok1".into());
        assert_eq!(list(true, false, 0, &toggled, &stacks_open), "[app 2]> [site 1]> [temporary 3] ok1+2 ok2 ok3");
        // An unfolded project; favorites lead under a heading of their own, unfolded.
        toggled.insert(project("app").key());
        assert_eq!(list(true, false, 1, &toggled, &HashSet::new()), "[favorites 1] a [site 1]> [app 1] c [temporary 3] ok1+2");
    }

    #[test]
    fn a_search_shows_every_session_it_found_and_one_list_keeps_the_order() {
        let sessions =
            [session("ok1", "ok", Some(("ok", "ok"))), session("a", "fix login", None), session("ok2", "ok", Some(("ok", "ok")))];
        let groups = [temporary(), project("app"), temporary()];
        let visible = [0, 1, 2];
        let none = HashSet::new();
        let list = |by_project, searching| {
            let input = ListInput {
                sessions: &sessions,
                groups: &groups,
                visible: &visible,
                favorites: 0,
                by_project,
                searching,
                toggled: &none,
                stacks_open: &none,
            };
            drawn(&rows(&input), &sessions)
        };
        assert_eq!(list(true, true), "[app 1] a [temporary 2] ok1 ok2");
        // Not by project: by time as before, the repeated conversation still one row.
        assert_eq!(list(false, false), "ok1+1 a");
        assert_eq!(list(false, true), "ok1 a ok2");
    }
}

//! Starting an orchestrator: one goal, and a Claude Code session that plans a team of agents for
//! it, shows the plan, runs it in waves and has the result checked — drawn as it goes on the
//! Orchestrator page.
//!
//! As in Orca, the agent stays the user's own Claude Code; what Agentty adds is the first message
//! that teaches it to orchestrate (and `agentty tasks` for work that wants its own branches). From
//! omo: the plan is shown and approved before anything runs, every role has a model (set by a
//! profile), work goes in waves, and a reviewer checks the result against the goal at the end.

use super::Workbench;
use crate::i18n::{t, tf};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, TypeScale};
use agentty_bridge::plugins::{PromptRequest, PromptTarget};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, SharedString, Subscription, Window};
use std::path::PathBuf;

/// Which models the team runs on (kept in the settings as the default for new ones).
pub use crate::settings::OrchestrationProfile as Profile;

impl Profile {
    /// (role, model) for the plan: Claude Code's Agent tool takes `haiku`, `sonnet` and `opus`.
    fn models(self) -> [(&'static str, &'static str); 4] {
        match self {
            Profile::Balanced => {
                [("quick lookups", "haiku"), ("research, building, writing", "sonnet"), ("planning hard parts", "opus"), ("review", "opus")]
            }
            Profile::Quality => {
                [("quick lookups", "sonnet"), ("research, building, writing", "opus"), ("planning hard parts", "opus"), ("review", "opus")]
            }
            Profile::Economy => [
                ("quick lookups", "haiku"),
                ("research, building, writing", "haiku"),
                ("planning hard parts", "sonnet"),
                ("review", "sonnet"),
            ],
        }
    }

    /// The orchestrator's own model: the user's default, except when quality is asked for.
    fn lead_model(self) -> Option<String> {
        (self == Profile::Quality).then(|| "opus".to_string())
    }

    pub(super) fn label_key(self) -> &'static str {
        match self {
            Profile::Balanced => "orch.profile_balanced",
            Profile::Quality => "orch.profile_quality",
            Profile::Economy => "orch.profile_economy",
        }
    }

    pub(super) fn hint_key(self) -> &'static str {
        match self {
            Profile::Balanced => "orch.profile_balanced_hint",
            Profile::Quality => "orch.profile_quality_hint",
            Profile::Economy => "orch.profile_economy_hint",
        }
    }
}

/// Another AI coding agent the team can bring in: its command, its name, what it is good at, and
/// how it runs without a window of its own.
pub struct Helper {
    pub binary: &'static str,
    pub name: &'static str,
    strength: &'static str,
    run: &'static str,
}

/// The agents an orchestration can use besides Claude Code, when they are installed.
pub const HELPERS: &[Helper] = &[
    Helper {
        binary: "codex",
        name: "Codex",
        strength: "precise code changes, debugging and fixing failing tests, and an independent review of a diff",
        run: "`{cmd} exec --skip-git-repo-check \"<task>\" < /dev/null` (add `--sandbox workspace-write` when it has to write files in this folder)",
    },
    Helper {
        binary: "gemini",
        name: "Gemini CLI",
        strength: "reading very large inputs at once (a whole codebase, long documents) and research grounded in Google Search",
        run: "`{cmd} -p \"<task>\" < /dev/null`",
    },
    Helper {
        binary: "agy",
        name: "Antigravity CLI",
        strength: "a second, independent take from another model family, on building or on reviewing",
        run: "`{cmd} -p \"<task>\" < /dev/null` (`--mode plan` to only read and review, `--mode accept-edits` when it has to change files in this folder)",
    },
];

/// The first message: the goal, then how to orchestrate it. English whatever the app's language;
/// the last line says which language to talk in. `helpers` are the other agents the team may use.
/// `helpers`: the other agents the team may use, each with the command that runs it (its whole path
/// when known).
pub fn prompt(goal: &str, profile: Profile, helpers: &[(&Helper, String)], confirm_plan: bool, language: &str) -> String {
    let go = if confirm_plan {
        "End with \"Start?\" and wait for the user's answer; change the plan if they ask."
    } else {
        "Then start it right away, without waiting for an answer."
    };
    let models = profile.models().iter().map(|(role, model)| format!("- {role}: `{model}`")).collect::<Vec<_>>().join("\n");
    let helpers = if helpers.is_empty() {
        String::new()
    } else {
        let list = helpers
            .iter()
            .map(|(h, cmd)| format!("- {} (`{cmd}`): {}. Run: {}.", h.name, h.strength, h.run.replace("{cmd}", cmd)))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "Other AI agents on this machine. Give them the parts their strength fits, and say so in the plan (an `agent` \
column: claude, or one of these):
{list}
Run one from a subagent's Bash (or your own), in this folder, with everything it needs in its prompt; read what it \
produces and have it checked like any other agent's work. A change that should land as its own branch in a git \
repository can go to a Codex session instead: `\"agent\": \"codex\"` in that task of `agentty tasks --plan`. Never \
turn off their permission or sandbox checks (no `--yolo`, no `--dangerously-…` flags).

"
        )
    };
    format!(
        "Goal: {goal}

You are the orchestrator of a team of agents for this goal, started from Agentty's Orchestrator. Agentty draws \
your subagents as waves on its Orchestrator page, so work in the shape described here. You plan, hand out, check \
and combine; you do not do the agents' work yourself.

1. Plan first. Read what you need of this folder and of the goal; ask the user only what you truly cannot decide. \
Then reply with the plan as a table, one row per agent: name (short and kebab-case, e.g. `market-scan-1`), role, \
model, wave, depends on, and what it hands over. Use as many agents as the work needs and no more: independent work \
side by side in one wave, work that needs another's result in a later wave, and a reviewer in the last wave who \
checks the result against the goal. Hand the same plan to Agentty, which draws every agent of it at once and fills \
each in as it runs (run it again whenever the plan changes):
```
agentty plan <<'PLAN'
{{\"agents\": [{{\"name\": \"market-scan-1\", \"wave\": 1, \"role\": \"…\", \"agent\": \"claude\", \"model\": \"…\", \"depends_on\": []}}, …]}}
PLAN
```
`depends_on` lists the names of agents in earlier waves whose results it needs. {go}

2. Run it wave by wave. Start every agent of a wave in ONE message (one Agent tool call each, so they run in \
parallel), with its name from the plan, exactly, as the description, the model from the plan, and a prompt that says exactly what to do, \
what to hand over and where (files with their paths), and the earlier results it needs. Wait for a wave before \
starting the next. Work that changes code in a git repository and should land as its own branch or pull request \
goes to parallel sessions instead: `agentty tasks --plan plan.json` (see `agentty tasks --help`), one task per \
change. An agent that needs a browser uses `agentty browser` (see `agentty browser --help`) to open pages, click, \
type, read them and take screenshots.

3. Check. The reviewer reads what was produced and says for each deliverable whether it is done, and why. Send back \
what is not, with what to fix, at most twice.

4. Finish with a short report: the conclusion, what each agent produced (files with their paths), what the reviewer \
confirmed, and what is left open.

{helpers}Models by role (the Agent tool's `model`):
{models}

Talk to me in {language}."
    )
}

/// Example goals, one click from the goal box.
const EXAMPLES: [&str; 4] = ["orch.example_research", "orch.example_tutorial", "orch.example_feature", "orch.example_tests"];

/// Where a started orchestrator opens.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// A workspace of its own, in `group` or else the orchestration group.
    NewWorkspace { group: Option<u64> },
    /// A tab in the workspace in front.
    NewTab,
}

pub struct OrchestratorDialog {
    pub goal: Entity<TextInput>,
    _goal_events: Subscription,
    profile: Profile,
    pub folder: PathBuf,
    place: Place,
    /// Other agents installed here, by binary, and whether this team uses them.
    helpers: Vec<(&'static str, bool)>,
}

impl Workbench {
    /// Opens the start dialog: a goal, a profile, a folder.
    pub(super) fn open_orchestrator_dialog(&mut self, place: Place, window: &mut Window, cx: &mut Context<Self>) {
        let goal = cx.new(|cx| TextInput::localized("", "orch.goal_placeholder", window, cx).multiline(4));
        let events = cx.subscribe_in(&goal, window, |this, _, event: &TextInputEvent, window, cx| {
            if matches!(event, TextInputEvent::Cancelled) {
                this.orchestrator_dialog = None;
                this.focus_active(window, cx);
                cx.notify();
            }
        });
        goal.update(cx, |input, cx| input.focus_and_select_all(window, cx));
        let folder = self.default_cwd(cx);
        let prefs = crate::settings::settings(cx);
        let off = &prefs.orchestration_helpers_off;
        let helpers = HELPERS
            .iter()
            .filter(|h| self.installed.as_ref().is_some_and(|i| i.has(h.binary)))
            .map(|h| (h.binary, !off.iter().any(|o| o == h.binary)))
            .collect();
        let profile = prefs.orchestration_profile;
        // From the home page or the page: a tab of the workspace in front, when that is the setting.
        let place = match place {
            Place::NewWorkspace { group: None } if prefs.orchestration_in_tab && !self.workspaces.is_empty() => Place::NewTab,
            place => place,
        };
        self.orchestrator_dialog = Some(OrchestratorDialog { goal, _goal_events: events, profile, folder, place, helpers });
        self.tab_menu = None;
        cx.notify();
    }

    fn pick_orchestrator_folder(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                if let Some(path) = paths.into_iter().next() {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(dialog) = &mut this.orchestrator_dialog {
                            dialog.folder = path;
                        }
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// Starts the orchestrator: a Claude Code session with the goal and how to orchestrate it.
    fn start_orchestrator(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.orchestrator_dialog.as_ref() else { return };
        let goal = dialog.goal.read(cx).text().trim().to_string();
        if goal.is_empty() {
            return self.show_toast(t(cx, "orch.goal_empty"), cx);
        }
        let (profile, folder, place) = (dialog.profile, dialog.folder.clone(), dialog.place);
        let paths = self.installed.as_ref().map(|i| i.paths.clone()).unwrap_or_default();
        let helpers: Vec<(&Helper, String)> = dialog
            .helpers
            .iter()
            .filter(|(_, on)| *on)
            .filter_map(|(binary, _)| HELPERS.iter().find(|h| h.binary == *binary))
            .map(|h| (h, paths.get(h.binary).cloned().unwrap_or_else(|| h.binary.to_string())))
            .collect();
        let language = agentty_bridge::idea::language_name(crate::settings::settings(cx).language.code());
        let title: String = goal.lines().next().unwrap_or("").chars().take(40).collect();
        let request = PromptRequest {
            text: prompt(&goal, profile, &helpers, crate::settings::settings(cx).orchestration_confirm_plan, language),
            title: Some(title),
            target: match place {
                Place::NewWorkspace { .. } => PromptTarget::NewWorkspace,
                Place::NewTab => PromptTarget::NewTab,
            },
            agent: Some("claude".into()),
            cwd: Some(folder.clone()),
            submit: true,
            model: profile.lead_model(),
            ..Default::default()
        };
        match self.deliver_prompt(request, window, cx) {
            Ok(pane_id) => {
                self.orchestrator_dialog = None;
                if let Place::NewWorkspace { group } = place {
                    let group = group.unwrap_or_else(|| self.orchestration_group(cx));
                    if let Some(ws) = self.workspaces.last_mut() {
                        ws.group = Some(group);
                    }
                    self.persist(cx);
                }
                // Kept on disk, so it is found again after a restart; its work shows on the
                // Orchestration page.
                if let Some(id) = self.add_orchestration(pane_id, goal, folder, cx) {
                    self.open_orchestration(id, cx);
                }
                self.show_toast_for(t(cx, "orch.started"), 6000, cx);
            }
            Err(err) => self.show_toast(err, cx),
        }
        cx.notify();
    }

    /// The group orchestrators' workspaces go in, made on first use (found by name in any language,
    /// like the board's: a group the user renamed is theirs, and the next orchestrator makes a new one).
    pub(super) fn orchestration_group(&mut self, cx: &gpui::App) -> u64 {
        use crate::settings::Language;
        let names =
            [Language::En, Language::Ko, Language::Ja, Language::Zh, Language::De].map(|l| crate::i18n::tr(l, "group.orchestration"));
        if let Some(group) = self.groups.iter_mut().find(|g| names.contains(&g.name.as_str())) {
            group.collapsed = false;
            return group.id;
        }
        let id = self.next_id();
        self.groups.push(super::Group { id, name: t(cx, "group.orchestration").to_string(), collapsed: false, color: None });
        id
    }

    /// The start dialog, over everything.
    pub(super) fn render_orchestrator_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.orchestrator_dialog.as_ref()?;
        let empty = dialog.goal.read(cx).text().trim().is_empty();
        let button = |id: &'static str, label: &'static str, primary: bool, enabled: bool| {
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .rounded_md()
                .t_body()
                .bg(if primary { hex(Chrome::ACCENT) } else { hex(0x2d2d30) })
                .when(!primary && crate::theme::is_light(), crate::ui::outlined)
                .text_color(hex(if primary { Chrome::ON_ACCENT } else { Chrome::BRIGHT }))
                .when(enabled, |d| d.cursor_pointer().hover(|s| s.opacity(0.85)))
                .when(!enabled, |d| d.opacity(0.45))
                .child(label)
        };
        let examples = div().flex().flex_wrap().gap_1p5().children(EXAMPLES.iter().enumerate().map(|(index, key)| {
            let text = t(cx, key);
            div()
                .id(("orch-example", index))
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .t_caption()
                .text_color(hex(Chrome::FOREGROUND))
                .hover(|s| s.bg(hex(Chrome::HOVER)).border_color(hex_alpha(Chrome::ACCENT, 0.7)))
                .child(t(cx, short_key(key)))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    if let Some(dialog) = &this.orchestrator_dialog {
                        dialog.goal.update(cx, |input, cx| {
                            input.set_text(text, cx);
                            input.focus_and_select_all(window, cx);
                        });
                    }
                    cx.notify();
                }))
        }));
        let profiles = div().flex().gap_1().children(Profile::ALL.into_iter().map(|profile| {
            let on = dialog.profile == profile;
            div()
                .id(SharedString::from(format!("orch-profile-{profile:?}")))
                .flex_1()
                .px_2()
                .py_1p5()
                .rounded_md()
                .cursor_pointer()
                .border_1()
                .border_color(hex(if on { Chrome::ACCENT } else { Chrome::BORDER }))
                .when(on, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.12)))
                .when(!on, |d| d.hover(|s| s.bg(hex(Chrome::HOVER))))
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div().t_small().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, profile.label_key())),
                )
                .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, profile.hint_key())))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(dialog) = &mut this.orchestrator_dialog {
                        dialog.profile = profile;
                    }
                    cx.notify();
                }))
        }));
        // Claude Code always; the others installed here, each switched on or off.
        let helpers = (!dialog.helpers.is_empty()).then(|| {
            let chip = |id: SharedString, logo: &'static str, name: &'static str, on: bool| {
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(hex(if on { Chrome::ACCENT } else { Chrome::BORDER }))
                    .when(on, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.12)))
                    .when(!on, |d| d.opacity(0.55))
                    .t_small()
                    .text_color(hex(Chrome::BRIGHT))
                    .child(crate::brand::avatar(logo, 14.))
                    .child(name)
            };
            div().flex().flex_wrap().gap_1p5().child(chip("orch-helper-claude".into(), "claude", "Claude Code", true)).children(
                dialog.helpers.iter().filter_map(|(binary, on)| {
                    let helper = HELPERS.iter().find(|h| h.binary == *binary)?;
                    let binary = *binary;
                    Some(
                        chip(SharedString::from(format!("orch-helper-{binary}")), binary, helper.name, *on)
                            .cursor_pointer()
                            .hover(|s| s.bg(hex(Chrome::HOVER)))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if let Some(dialog) = &mut this.orchestrator_dialog {
                                    if let Some(entry) = dialog.helpers.iter_mut().find(|(b, _)| *b == binary) {
                                        entry.1 = !entry.1;
                                    }
                                }
                                cx.notify();
                            })),
                    )
                }),
            )
        });
        let place = match dialog.place {
            Place::NewWorkspace { group: Some(id) } => {
                let name = self.groups.iter().find(|g| g.id == id).map(|g| g.name.clone()).unwrap_or_default();
                tf(cx, "orch.place_workspace_in", &[("group", &name)])
            }
            Place::NewWorkspace { group: None } => t(cx, "orch.place_workspace").to_string(),
            Place::NewTab => t(cx, "orch.place_tab").to_string(),
        };
        let section = |label: &'static str| div().t_small().text_color(hex(Chrome::MUTED)).child(label);
        Some(
            div()
                .id("orch-dialog-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex_alpha(0x000000, 0.45))
                .occlude()
                .child(
                    div()
                        .w(px(560.))
                        .max_w_full()
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .shadow_lg()
                        .child(div().flex().items_center().gap_2().child(icon("orbit", 16., hex(Chrome::BRIGHT))).child(
                            div().t_title().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, "orch.title")),
                        ))
                        .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(t(cx, "orch.intro")))
                        .child(section(t(cx, "orch.goal")))
                        .child(
                            div()
                                .px_2()
                                .py_1p5()
                                .rounded_md()
                                .border_1()
                                .border_color(hex(Chrome::ACCENT))
                                .t_body()
                                .text_color(hex(Chrome::BRIGHT))
                                .child(dialog.goal.clone()),
                        )
                        .child(examples)
                        .child(section(t(cx, "orch.profile")))
                        .child(profiles)
                        .when_some(helpers, |d, helpers| d.child(section(t(cx, "orch.helpers"))).child(helpers))
                        .child(section(t(cx, "orch.folder")))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(icon("folder", 12., hex(Chrome::MUTED)))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .t_small()
                                        .text_color(hex(Chrome::FOREGROUND))
                                        .child(crate::ui::tilde(&dialog.folder)),
                                )
                                .child(
                                    div()
                                        .id("orch-folder")
                                        .px_2()
                                        .py_0p5()
                                        .rounded_sm()
                                        .cursor_pointer()
                                        .t_caption()
                                        .text_color(hex(Chrome::FOREGROUND))
                                        .hover(|s| s.bg(hex(Chrome::HOVER)))
                                        .child(t(cx, "orch.change_folder"))
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.pick_orchestrator_folder(cx))),
                                ),
                        )
                        .child(
                            div()
                                .pt_1()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().flex_1().min_w_0().truncate().t_caption().text_color(hex(Chrome::MUTED)).child(place))
                                .child(button("orch-cancel", t(cx, "confirm.cancel"), false, true).on_click(cx.listener(
                                    |this, _: &ClickEvent, window, cx| {
                                        this.orchestrator_dialog = None;
                                        this.focus_active(window, cx);
                                        cx.notify();
                                    },
                                )))
                                .child(
                                    button("orch-start", t(cx, "orch.start"), true, !empty)
                                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.start_orchestrator(window, cx))),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// The chip label of an example goal (`orch.example_x` → `orch.example_x_short`).
fn short_key(key: &str) -> &'static str {
    match key {
        "orch.example_research" => "orch.example_research_short",
        "orch.example_tutorial" => "orch.example_tutorial_short",
        "orch.example_feature" => "orch.example_feature_short",
        _ => "orch.example_tests_short",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_message_carries_the_goal_the_models_and_the_language() {
        let text = prompt("Compare AI coding agents", Profile::Economy, &[], true, "Korean");
        assert!(text.starts_with("Goal: Compare AI coding agents\n"));
        assert!(text.contains("- research, building, writing: `haiku`"));
        assert!(text.contains("- review: `sonnet`"));
        assert!(text.contains("\"Start?\""));
        assert!(text.ends_with("Talk to me in Korean."));
        assert_eq!(Profile::Quality.lead_model().as_deref(), Some("opus"));
        assert_eq!(Profile::Balanced.lead_model(), None);
        assert!(!text.contains("Other AI agents"));
    }

    #[test]
    fn other_agents_come_with_their_strength_and_how_to_run_them() {
        let codex = HELPERS.iter().find(|h| h.binary == "codex").unwrap();
        let text = prompt("Fix the failing tests", Profile::Balanced, &[(codex, "/opt/example/bin/codex".into())], true, "English");
        assert!(text.contains("\"Start?\""));
        let straight = prompt("Fix the failing tests", Profile::Balanced, &[], false, "English");
        assert!(!straight.contains("\"Start?\""));
        assert!(straight.contains("start it right away"));
        assert!(text.contains("- Codex (`/opt/example/bin/codex`): precise code changes"));
        assert!(text.contains("`/opt/example/bin/codex exec --skip-git-repo-check"));
        assert!(!text.contains("{cmd}"));
        assert!(text.contains("an `agent`"));
        assert!(text.contains("no `--yolo`"));
        assert!(text.ends_with("Talk to me in English."));
    }
}

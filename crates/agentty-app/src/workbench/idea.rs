//! "Build my idea" and "Launch": from an idea on the idea page to a project folder with an agent
//! building it, and from a project to the Launch plugin that publishes it (GitHub + Vercel).

use super::{Group, Page, Workbench};
use crate::i18n::tf;
use crate::idea_view::{IdeaEvent, IdeaView};
use crate::launch::PaneKind;
use crate::plugins;
use agentty_bridge::idea::IdeaInput;
use agentty_bridge::model::Agent;
use agentty_bridge::plugins::{store, PromptRequest, PromptTarget};
use gpui::{AppContext, Context, Entity, Window};
use serde_json::json;
use std::path::PathBuf;

/// The built-in plugin that publishes projects.
pub(super) const LAUNCH_PLUGIN: &str = "launch";
/// The workspace group idea projects are collected in.
const IDEA_GROUP: &str = "Agentty Idea";

impl Workbench {
    pub(super) fn open_idea_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.welcome = false;
        self.new_workspace = None;
        if self.page != Some(Page::Idea) {
            self.open_page(Page::Idea, cx);
        }
        self.detect_agents(cx);
        let view = self.idea_view(window, cx);
        view.update(cx, |view, cx| view.focus(window, cx));
    }

    pub(super) fn idea_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<IdeaView> {
        let (claude, codex) = (self.offers_agent("claude", cx), self.offers_agent("codex", cx));
        let view = match &self.idea {
            Some(view) => view.clone(),
            None => {
                let view = cx.new(|cx| IdeaView::new(window, cx));
                cx.subscribe_in(&view, window, |this, _, event: &IdeaEvent, window, cx| match event {
                    IdeaEvent::Start { input, root, agent, team } => {
                        this.start_idea(input.clone(), root.clone(), *agent, *team, window, cx)
                    }
                })
                .detach();
                self.idea = Some(view.clone());
                view
            }
        };
        // Before detection finishes, offer both.
        let detected = self.installed.is_some();
        let other = self.pick_agent(None, false, cx).filter(|pick| pick.first_class().is_none()).map(crate::agents::AgentPick::name);
        let orchestration = crate::settings::settings(cx).orchestration;
        view.update(cx, |v, cx| v.set_agents(claude || !detected, codex || !detected, other, orchestration, cx));
        view
    }

    /// Creates the project folder, then starts the agent there: with the build prompt, or (`team`) as
    /// the orchestrator of a team that goes from analysis and design documents to the code.
    fn start_idea(&mut self, input: IdeaInput, root: PathBuf, agent: PaneKind, team: bool, window: &mut Window, cx: &mut Context<Self>) {
        let language = crate::settings::settings(cx).language.code().to_string();
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
        let task = cx.background_spawn(async move {
            let project = agentty_bridge::idea::create_project(&root, &input, &language, &stamp);
            // The 🚀 button the agent points to at the end lives in the Launch plugin.
            if project.is_ok() && !store::installed().iter().any(|p| p.id == LAUNCH_PLUGIN) {
                if let Err(err) = store::install_builtin(LAUNCH_PLUGIN) {
                    eprintln!("agentty: could not install the launch plugin: {err:#}");
                }
            }
            project
        });
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let project = task.await;
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    plugins::reload(cx);
                    let project = match project {
                        Ok(project) => {
                            // Only folders recorded here start Claude Code in auto mode later.
                            if let Err(err) = agentty_bridge::idea::register_project(&project.dir) {
                                eprintln!("agentty: could not record the idea project: {err:#}");
                            }
                            project
                        }
                        Err(err) => {
                            if let Some(idea) = &this.idea {
                                idea.update(cx, |v, cx| v.show_error(format!("{err:#}"), cx));
                            }
                            return;
                        }
                    };
                    let lead = if agent == PaneKind::Codex { Agent::Codex } else { Agent::Claude };
                    let mut request = PromptRequest {
                        text: project.prompt.clone(),
                        title: Some(project.title.clone()),
                        target: PromptTarget::NewWorkspace,
                        agent: Some(lead.id().into()),
                        cwd: Some(project.dir.clone()),
                        submit: true,
                        ..Default::default()
                    };
                    if team {
                        let prefs = crate::settings::settings(cx);
                        let profile = prefs.orchestration_profile;
                        let language = agentty_bridge::idea::language_name(prefs.language.code());
                        let helpers = this.default_helpers(lead, cx);
                        let goal = agentty_bridge::idea::team_goal(&project.title);
                        // The owner handed over the idea: the team starts without waiting for a yes.
                        request.text = super::orchestrator::prompt(&goal, lead, profile, &helpers, false, language);
                        request.model = profile.lead_model().filter(|_| lead == Agent::Claude);
                    }
                    match this.deliver_prompt(request, window, cx) {
                        Ok(pane) => {
                            // Its stages and agents show on the Orchestration page like any other's.
                            if team {
                                this.add_orchestration(pane, project.title.clone(), project.dir.clone(), cx);
                            }
                            let group = this.idea_group();
                            if let Some(ws) = this.workspaces.last_mut() {
                                ws.group = Some(group);
                            }
                            this.persist(cx);
                            crate::metrics::track(cx, "feature_used", json!({ "feature": "idea_start" }));
                            if let Some(idea) = &this.idea {
                                idea.update(cx, |v, cx| v.reset(cx));
                            }
                            let folder = crate::ui::tilde(&project.dir);
                            let mut message = tf(cx, if team { "idea.started_team" } else { "idea.started" }, &[("folder", &folder)]);
                            if !project.skipped.is_empty() {
                                message.push_str(&tf(cx, "idea.skipped", &[("n", &project.skipped.len().to_string())]));
                            }
                            this.show_toast(message, cx);
                        }
                        Err(err) => {
                            this.page = Some(Page::Idea);
                            if let Some(idea) = &this.idea {
                                idea.update(cx, |v, cx| v.show_error(err, cx));
                            }
                        }
                    }
                });
            });
        })
        .detach();
    }

    /// The group for idea projects, made on first use and opened so the new workspace shows. It is
    /// found by name: a renamed group is the user's own, and the next idea starts a new one.
    fn idea_group(&mut self) -> u64 {
        if let Some(group) = self.groups.iter_mut().find(|g| g.name == IDEA_GROUP) {
            group.collapsed = false;
            return group.id;
        }
        let id = self.next_id();
        self.groups.push(Group { id, name: IDEA_GROUP.into(), collapsed: false, color: None });
        id
    }

    /// Opens the Launch panel for the focused project (installing the built-in plugin on first use).
    pub(super) fn open_launch(&mut self, cx: &mut Context<Self>) {
        self.launcher_open = false;
        if plugins::plugin(cx, LAUNCH_PLUGIN).is_none() {
            if let Err(err) = store::install_builtin(LAUNCH_PLUGIN) {
                self.show_toast(format!("{err:#}"), cx);
                return;
            }
            plugins::reload(cx);
        }
        if plugins::plugin(cx, LAUNCH_PLUGIN).is_some_and(|p| !p.active()) {
            self.show_toast(tf(cx, "plugins.not_enabled", &[("name", "Launch")]), cx);
            return;
        }
        let project = self.active_pane().map(|p| p.read(cx).display_cwd());
        self.open_plugin_panel(LAUNCH_PLUGIN, cx);
        if let Some(project) = project {
            let context = self.plugin_context(LAUNCH_PLUGIN, None, cx);
            plugins::notify_plugin(
                LAUNCH_PLUGIN,
                "command/execute",
                json!({ "command": "launch.open", "args": { "path": project }, "context": context }),
                cx,
            );
        }
    }
}

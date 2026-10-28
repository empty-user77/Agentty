//! Starting an agent with a first message, whichever agent it is: Claude Code and Codex take it on
//! their command line; another agent CLI is started in a shell pane and the message is typed into
//! it once it is up. Features choose the agent with [`Workbench::pick_agent`], so none of them
//! needs one agent in particular.

use super::{Pane, Workbench};
use crate::agents::AgentPick;
use crate::launch::LaunchSpec;
use gpui::{Context, Window};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long another agent's TUI gets at least before its first message is typed in.
const TYPE_AFTER: Duration = Duration::from_secs(2);
/// A screen unchanged this long has finished drawing.
const SETTLED: Duration = Duration::from_millis(1000);
/// From then on a settled screen is taken as ready even without bracketed paste.
const NO_PASTE_MODE: Duration = Duration::from_secs(8);
/// The longest wait: the text is typed in then, ready or not.
const GIVE_UP: Duration = Duration::from_secs(30);
/// How long a first-run question (trust this folder?) may wait for the user's answer; the text is
/// never typed into it, since Enter would answer it for them.
const ANSWER_WAIT: Duration = Duration::from_secs(600);
const POLL: Duration = Duration::from_millis(250);

/// The pane that starts `pick` in `cwd`, and the message still to type into it (other agents).
pub fn prompt_spec(pick: AgentPick, prompt: String, title: String, cwd: PathBuf) -> (LaunchSpec, Option<String>) {
    match pick.first_class() {
        Some(agent) => (LaunchSpec::with_prompt(agent, prompt, title, cwd), None),
        None => {
            let title = if title.is_empty() { pick.name().to_string() } else { title };
            (LaunchSpec::shell_command(pick.binary().to_string(), title, cwd), Some(prompt))
        }
    }
}

/// What the launcher starts for `pick`, without a first message.
pub fn launch_choice(pick: AgentPick) -> crate::launch::LaunchChoice {
    match pick.first_class() {
        Some(agent) => crate::launch::LaunchChoice::Kind(agent.into()),
        None => crate::launch::LaunchChoice::Command { title: pick.name().to_string(), command: pick.binary().to_string() },
    }
}

/// Types `text` into `pane`, whose program has just started, once it takes input: at least `after`,
/// then until its screen has settled with bracketed paste on (an agent CLI can take seconds behind a
/// login shell) and no first-run question is open, and sends it when `submit`.
pub(super) fn type_when_ready(pane: Pane, text: String, after: Duration, submit: bool, cx: &mut Context<Workbench>) {
    cx.spawn(async move |_, cx| {
        let started = Instant::now();
        cx.background_executor().timer(after).await;
        // `from`: since when the program has been past its questions.
        let (mut screen, mut since, mut from) = (None, Instant::now(), started);
        loop {
            let Ok((paste, now, asking)) = pane.read_with(cx, |view, _| {
                let (paste, now) = view.input_state();
                (paste, now, super::chat::is_setup_screen(&view.screen_lines(super::chat::WHOLE_SCREEN)))
            }) else {
                return;
            };
            if asking {
                if started.elapsed() >= ANSWER_WAIT {
                    return;
                }
                (screen, from) = (None, Instant::now());
            } else if from.elapsed() >= GIVE_UP {
                break;
            } else if screen != Some(now) {
                (screen, since) = (Some(now), Instant::now());
            } else if since.elapsed() >= SETTLED && (paste || from.elapsed() >= NO_PASTE_MODE) {
                break;
            }
            cx.background_executor().timer(POLL).await;
        }
        let _ = pane.update(cx, |view, cx| if submit { view.submit_prompt(text, cx) } else { view.insert_text(&text) });
    })
    .detach();
}

/// [`Workbench::type_first_message`] into `pane`.
pub(super) fn type_first_message_in(pane: Pane, prompt: Option<String>, cx: &mut Context<Workbench>) {
    if let Some(prompt) = prompt {
        type_when_ready(pane, prompt, TYPE_AFTER, true, cx);
    }
}

impl Workbench {
    /// Types `prompt` into the active pane, an agent just started there, once it is up, and sends it.
    pub(super) fn type_first_message(&mut self, prompt: Option<String>, cx: &mut Context<Self>) {
        if let Some(pane) = self.active_pane() {
            type_first_message_in(pane, prompt, cx);
        }
    }

    /// A new tab with `wanted` (an agent id), or the agent that runs here when it does not.
    pub(super) fn launch_agent_tab(&mut self, wanted: &str, window: &mut Window, cx: &mut Context<Self>) {
        match self.pick_agent(Some(wanted), false, cx) {
            Some(pick) => self.request_launch(launch_choice(pick), super::LaunchTarget::NewTab, window, cx),
            None => self.show_toast(crate::i18n::t(cx, "agent.none_installed"), cx),
        }
    }

    /// Opens a tab with `pick` started on `prompt`.
    pub(super) fn open_agent_tab(
        &mut self,
        pick: AgentPick,
        prompt: String,
        title: String,
        cwd: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (spec, later) = prompt_spec(pick, prompt, title, cwd);
        self.open_tab(spec, window, cx);
        self.type_first_message(later, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_agents_start_as_commands_and_get_the_message_typed() {
        let (spec, later) = prompt_spec(AgentPick::Claude, "fix it".into(), "Fix".into(), PathBuf::from("/tmp"));
        assert_eq!(spec.kind, crate::launch::PaneKind::Claude);
        assert!(later.is_none());
        let gemini = AgentPick::from_id("gemini").unwrap();
        let (spec, later) = prompt_spec(gemini, "fix it".into(), String::new(), PathBuf::from("/tmp"));
        assert_eq!(spec.kind, crate::launch::PaneKind::Shell);
        assert_eq!(spec.start, crate::launch::Start::Command("gemini".into()));
        assert_eq!(spec.title, "Gemini CLI");
        assert_eq!(later.as_deref(), Some("fix it"));
    }
}

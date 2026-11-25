//! Bots as the app starts them: a bot's conversation is an ordinary agent pane whose agent is told
//! who the bot is (Agentty's guide, then the bot's own instructions and memory) and runs with the
//! bot's permission. The definitions live in `agentty_bridge::bots`; nothing here draws anything.

use crate::launch::{LaunchSpec, PaneKind, Start};
use agentty_bridge::bots::{guide, Bot, BotAgent, BotPermission, BotStore};
use std::path::PathBuf;

/// What a bot's session is given, written next to its other files when the session starts.
const INSTRUCTIONS_FILE: &str = "instructions.md";

pub fn store() -> BotStore {
    BotStore::open()
}

pub fn pane_kind(agent: BotAgent) -> PaneKind {
    match agent {
        BotAgent::Claude => PaneKind::Claude,
        BotAgent::Codex => PaneKind::Codex,
    }
}

/// Everything the bot's agent is told: Agentty's guide (while it is on), then the bot's own text.
/// Reads the bot's files: for when a session starts, never for drawing.
pub fn instructions_text(handle: &str) -> Option<String> {
    let store = store();
    let bot = store.load(handle).ok()?;
    let language = agentty_bridge::idea::language_name(crate::settings::language_code());
    use agentty_bridge::bots::Part;
    let (soul, rules, context) = (store.soul(handle).ok()?, store.part(handle, Part::Rules).ok()?, store.part(handle, Part::Context).ok()?);
    let (memory, skills) = (store.memory(handle).ok()?, store.skills(handle));
    let parts = guide::Parts { soul: &soul, rules: &rules, context: &context, memory: &memory, skills: &skills };
    let own = guide::instructions(&bot, &store.memory_path(handle).ok()?, &parts, language);
    Some(match crate::agent_guide::guide_text() {
        Some(agentty) => format!("{agentty}\n\n{own}"),
        None => own,
    })
}

/// [`instructions_text`] as a file in the bot's folder (Claude Code takes a path).
pub fn instructions_file(handle: &str) -> Option<PathBuf> {
    let text = instructions_text(handle)?;
    let path = store().dir(handle).ok()?.join(INSTRUCTIONS_FILE);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        agentty_bridge::fsutil::write_private(&path, text.as_bytes()).ok()?;
    }
    Some(path)
}

/// A new conversation with `bot`, or one continued from its agent session.
pub fn spec(bot: &Bot, session: Option<String>) -> LaunchSpec {
    let kind = pane_kind(bot.agent);
    let mut spec = match session {
        Some(id) => LaunchSpec::resume(kind.agent().expect("a bot runs an agent"), id, bot.name.clone(), bot.folder.clone()),
        None => LaunchSpec::new(kind, bot.folder.clone()),
    };
    spec.title = bot.name.clone();
    if !bot.folder.is_dir() {
        spec.missing_cwd = Some(bot.folder.clone());
        spec.cwd = crate::launch::home_dir();
    }
    apply(&mut spec, bot);
    spec
}

/// A saved pane of a bot, coming back: the bot's permission as it is now. A bot deleted since
/// leaves an ordinary pane.
pub fn restore(spec: &mut LaunchSpec, handle: &str) {
    if let Ok(bot) = store().load(handle) {
        if spec.kind == pane_kind(bot.agent) {
            spec.title = bot.name.clone();
            apply(spec, &bot);
        }
    }
}

fn apply(spec: &mut LaunchSpec, bot: &Bot) {
    debug_assert!(!matches!(spec.start, Start::Command(_)));
    spec.bot = Some(bot.handle.clone());
    spec.read_only = bot.permission == BotPermission::ReadOnly;
    spec.restricted = bot.permission == BotPermission::Files;
    if spec.model.is_none() {
        spec.model = bot.model.clone();
    }
    // A bot is talked to: its conversation shows as a chat unless the user chose its terminal.
    spec.chat_view.get_or_insert(true);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bot(agent: BotAgent, permission: BotPermission) -> Bot {
        let mut bot = Bot::new("reviewer", "Reviewer", "Reviews branches", agent, std::env::temp_dir());
        bot.permission = permission;
        bot
    }

    #[test]
    fn a_bots_pane_carries_its_handle_its_permission_and_its_model() {
        let mut reviewer = bot(BotAgent::Claude, BotPermission::ReadOnly);
        reviewer.model = Some("opus".into());
        let spec = spec(&reviewer, None);
        assert_eq!((spec.kind, spec.bot.as_deref(), spec.title.as_str()), (PaneKind::Claude, Some("reviewer"), "Reviewer"));
        assert!(spec.read_only && !spec.restricted);
        assert_eq!(spec.model.as_deref(), Some("opus"));
        assert_eq!(spec.chat_view, Some(true));
        let files = super::spec(&bot(BotAgent::Codex, BotPermission::Files), None);
        assert!((files.kind, files.restricted, files.read_only) == (PaneKind::Codex, true, false));
        let asking = super::spec(&bot(BotAgent::Claude, BotPermission::Ask), Some("session-example".into()));
        assert!(!asking.restricted && !asking.read_only);
        assert_eq!(asking.start, Start::Resume("session-example".into()));
    }

    /// Whatever the user's "always bypass" setting says, a bot's agent is never started with its
    /// permission prompts skipped.
    #[test]
    fn a_bots_command_line_never_skips_permissions() {
        for agent in [BotAgent::Claude, BotAgent::Codex] {
            for permission in [BotPermission::Ask, BotPermission::ReadOnly, BotPermission::Files] {
                let args = spec(&bot(agent, permission), None).command().unwrap();
                assert!(!args.iter().any(|a| a.contains("dangerously") || a == "bypassPermissions"), "{agent:?} {permission:?}: {args:?}");
            }
        }
    }
}

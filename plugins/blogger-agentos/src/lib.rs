//! Blogger AgentOS — the skeleton of a plugin that runs work through Agentty's agents.
//!
//! Three steps (outline, draft, edit). Each one sends its skill — the prompt written for that
//! step — to a Claude Code session, waits for that session to stop working, reads what it wrote,
//! and checks it against the step's rule before offering the next one. The run is kept in the
//! plugin's own storage, so closing Agentty does not lose it.
//!
//! It is a skeleton on purpose: a real AgentOS is mostly its prompts, and those are written by
//! whoever knows the trade. What is here is the machine that walks them.
//!
//! The user stays in front of it: every step runs in a session they can read and take over, and
//! nothing moves on until they press the button.

use agentty_plugin::{export_plugin, ui, Host, PaneStatus, Plugin, UiEvent};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How long to wait before looking again when a session says nothing.
const POLL_MS: u64 = 4_000;
/// What is shown of a step's result.
const SHOWN: usize = 4_000;

/// A step of the trade: what it asks for, and what it will not accept back.
struct Step {
    id: &'static str,
    title: &'static str,
    /// The skill. Prompts that ship in code are English; the agent is told which language to
    /// answer in from the user's own setting.
    prompt: &'static str,
    /// What the plugin checks before calling the step done.
    rule: fn(&str) -> Result<(), &'static str>,
}

const STEPS: &[Step] = &[
    Step {
        id: "outline",
        title: "Outline",
        prompt: "You are writing a blog post about: {topic}\n\nWrite an outline only: a working title, \
                 then three to six section headings, each with one line saying what it covers. No prose yet.\n\n\
                 Talk to me in {language}.",
        rule: has_sections,
    },
    Step {
        id: "draft",
        title: "Draft",
        prompt: "Write the post from the outline above. Follow the headings as they are. Keep it concrete: \
                 every claim either shows something or is cut. No filler openings, no summary of what you are \
                 about to say.\n\nTalk to me in {language}.",
        rule: long_enough,
    },
    Step {
        id: "edit",
        title: "Edit",
        prompt: "Edit the draft above. Cut what repeats, replace anything vague with the thing it stands for, \
                 and leave the structure alone. Say what you changed and why, in a few lines, after the post.\n\n\
                 Talk to me in {language}.",
        rule: no_placeholders,
    },
];

fn has_sections(text: &str) -> Result<(), &'static str> {
    let headings = text.lines().filter(|line| line.trim_start().starts_with('#') || line.trim_start().starts_with("- ")).count();
    if headings < 3 {
        return Err("the outline has fewer than three sections");
    }
    Ok(())
}

fn long_enough(text: &str) -> Result<(), &'static str> {
    if text.chars().count() < 600 {
        return Err("the draft is very short");
    }
    Ok(())
}

fn no_placeholders(text: &str) -> Result<(), &'static str> {
    const LEFTOVERS: &[&str] = &["TODO", "TBD", "lorem ipsum", "[insert"];
    match LEFTOVERS.iter().find(|word| text.to_lowercase().contains(&word.to_lowercase())) {
        Some(word) => Err(match *word {
            "TODO" => "the text still says TODO",
            "TBD" => "the text still says TBD",
            "lorem ipsum" => "the text still has placeholder prose",
            _ => "the text still has a placeholder in brackets",
        }),
        None => Ok(()),
    }
}

/// Where a step is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum State {
    #[default]
    Waiting,
    /// The prompt was sent; the agent is at work.
    Running,
    /// The agent stopped and what it wrote passed the rule.
    Done,
    /// The agent stopped and what it wrote did not.
    Failed,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Run {
    topic: String,
    /// Which step is next.
    step: usize,
    state: State,
    /// The pane the run is happening in.
    pane: Option<u64>,
    /// What each finished step produced, in order.
    results: Vec<String>,
    note: String,
}

#[derive(Default)]
struct BloggerAgentOs {
    run: Run,
    topic_field: String,
    language: String,
    /// The `session/get` or `host/timer` being waited for.
    pending: Option<u64>,
    loading: bool,
}

impl BloggerAgentOs {
    fn step(&self) -> Option<&'static Step> {
        STEPS.get(self.run.step)
    }

    fn language(&self) -> &str {
        if self.language.is_empty() {
            "English"
        } else {
            &self.language
        }
    }

    fn save(&self, host: &Host) {
        if let Ok(value) = serde_json::to_value(&self.run) {
            host.storage_set("run", value);
        }
    }

    /// Sends the current step's skill to the agent.
    fn start_step(&mut self, host: &Host) {
        let Some(step) = self.step() else { return };
        let prompt = step.prompt.replace("{topic}", &self.run.topic).replace("{language}", self.language());
        // The first step opens the session; the rest continue in the same one, where the user can
        // read everything that came before.
        let mut params = serde_json::json!({ "text": prompt, "title": format!("Blogger: {}", self.run.topic), "submit": true });
        match self.run.pane {
            Some(pane) => {
                params["target"] = serde_json::json!("pane");
                params["paneId"] = serde_json::json!(pane);
            }
            None => params["target"] = serde_json::json!("newTab"),
        }
        self.pending = Some(host.call("prompt/inject", params));
        self.run.state = State::Running;
        self.run.note = format!("{} — asked the agent", step.title);
        host.set_badge(format!("{}/{}", self.run.step + 1, STEPS.len()));
        self.save(host);
    }

    /// Reads what the agent wrote in the run's pane.
    fn read_session(&mut self, host: &Host) {
        let Some(pane) = self.run.pane else { return };
        self.pending = Some(host.call("session/get", serde_json::json!({ "paneId": pane, "maxTurns": 40 })));
    }

    /// What the agent last wrote, from a `session/get` answer.
    fn last_answer(session: &Value) -> String {
        session
            .get("turns")
            .and_then(Value::as_array)
            .map(|turns| {
                turns
                    .iter()
                    .rev()
                    .find(|turn| turn.get("role").and_then(Value::as_str) == Some("assistant"))
                    .and_then(|turn| turn.get("text").and_then(Value::as_str))
                    .unwrap_or_default()
                    .to_string()
            })
            .unwrap_or_default()
    }

    fn finish_step(&mut self, host: &Host, text: String) {
        let Some(step) = self.step() else { return };
        match (step.rule)(&text) {
            Ok(()) => {
                self.run.state = State::Done;
                self.run.note = format!("{} — done", step.title);
            }
            Err(why) => {
                self.run.state = State::Failed;
                self.run.note = format!("{} — {why}", step.title);
            }
        }
        if self.run.results.len() > self.run.step {
            self.run.results[self.run.step] = text;
        } else {
            self.run.results.push(text);
        }
        self.save(host);
    }

    fn draw(&self, host: &Host) {
        let mut children = vec![ui::styled_text("Blogger AgentOS", "title")];
        if self.run.topic.is_empty() {
            children.push(ui::styled_text("A post, written in three steps by an agent you can watch.", "muted"));
            children.push(ui::input("topic", "What is the post about?", self.topic_field.clone()));
            children.push(ui::styled_button("start", "Start", "primary"));
            host.set_panel(ui::column(children));
            return;
        }

        children.push(ui::styled_text(self.run.topic.clone(), "body"));
        let items: Vec<Value> = STEPS
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let state = match (index.cmp(&self.run.step), self.run.state) {
                    (std::cmp::Ordering::Less, _) => "done",
                    (std::cmp::Ordering::Equal, State::Running) => "working",
                    (std::cmp::Ordering::Equal, State::Done) => "done",
                    (std::cmp::Ordering::Equal, State::Failed) => "needs another try",
                    (std::cmp::Ordering::Equal, State::Waiting) => "next",
                    _ => "",
                };
                ui::item(step.id, step.title, state)
            })
            .collect();
        children.push(ui::list("steps", items, ""));
        if !self.run.note.is_empty() {
            children.push(ui::styled_text(self.run.note.clone(), "muted"));
        }

        let mut buttons = Vec::new();
        match self.run.state {
            State::Waiting => buttons.push(ui::styled_button("go", format!("Run: {}", self.step().map_or("", |s| s.title)), "primary")),
            State::Running => children.push(ui::spinner("The agent is working…")),
            State::Done if self.run.step + 1 < STEPS.len() => {
                buttons.push(ui::styled_button("next", "Approve and continue", "primary"));
                buttons.push(ui::button("again", "Run this step again"));
            }
            State::Done => buttons.push(ui::button("again", "Run this step again")),
            State::Failed => {
                buttons.push(ui::styled_button("again", "Try this step again", "primary"));
                buttons.push(ui::button("next", "Keep it anyway"));
            }
        }
        buttons.push(ui::styled_button("reset", "Start over", "ghost"));
        children.push(ui::row(buttons));

        if let Some(text) = self.run.results.get(self.run.step) {
            children.push(ui::divider());
            children.push(ui::styled_text(text.chars().take(SHOWN).collect::<String>(), "code"));
        }
        host.set_panel(ui::column(children));
    }
}

impl Plugin for BloggerAgentOs {
    fn init(&mut self, host: &Host, _: &Value) {
        self.loading = true;
        self.pending = Some(host.storage_get("run"));
        self.language = host.context().get("language").and_then(Value::as_str).unwrap_or("en").to_string();
    }

    fn panel_open(&mut self, host: &Host) {
        self.draw(host);
    }

    fn command(&mut self, host: &Host, _: &str) {
        host.show_panel();
        self.draw(host);
    }

    fn ui_event(&mut self, host: &Host, event: UiEvent) {
        match event.element.as_str() {
            "topic" => {
                self.topic_field = event.text();
                if event.event != "submit" {
                    return;
                }
                self.run = Run { topic: self.topic_field.trim().to_string(), ..Default::default() };
                self.start_step(host);
            }
            "start" => {
                let topic = self.topic_field.trim().to_string();
                if topic.is_empty() {
                    host.notify_user("warning", "What is the post about?");
                    return;
                }
                self.run = Run { topic, ..Default::default() };
                self.start_step(host);
            }
            "go" | "again" => self.start_step(host),
            "next" => {
                self.run.step += 1;
                self.run.state = State::Waiting;
                if self.run.step >= STEPS.len() {
                    self.run.step = STEPS.len() - 1;
                    self.run.note = "Finished.".into();
                    host.set_badge("✓");
                } else {
                    self.start_step(host);
                    return;
                }
                self.save(host);
            }
            "reset" => {
                self.run = Run::default();
                self.topic_field.clear();
                host.set_badge("");
                self.save(host);
            }
            _ => return,
        }
        self.draw(host);
    }

    /// The agent it set to work changed what it is doing.
    fn pane_status(&mut self, host: &Host, status: PaneStatus) {
        if self.run.pane != Some(status.pane_id) || self.run.state != State::Running {
            return;
        }
        if status.needs_user() {
            self.run.note = format!("The agent is asking you something in {}", status.title);
            self.draw(host);
            return;
        }
        if status.is_done() {
            self.read_session(host);
        }
    }

    fn answer(&mut self, host: &Host, id: u64, result: Result<Value, String>) {
        if self.pending != Some(id) {
            return;
        }
        self.pending = None;
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                self.run.state = State::Failed;
                self.run.note = error;
                self.draw(host);
                return;
            }
        };
        if self.loading {
            // The run that was kept, read back at startup.
            self.loading = false;
            if let Ok(run) = serde_json::from_value::<Run>(value.get("value").cloned().unwrap_or(Value::Null)) {
                self.run = run;
                self.topic_field = self.run.topic.clone();
            }
            self.draw(host);
            return;
        }
        if let Some(pane) = value.get("paneId").and_then(Value::as_u64) {
            if value.get("turns").is_none() {
                // The prompt landed: this is the pane to watch.
                self.run.pane = Some(pane);
                self.save(host);
                // Until the first status arrives, look again in a while: a session that was
                // already idle may never change what it is doing.
                self.pending = Some(host.wait(POLL_MS));
                self.draw(host);
                return;
            }
            let text = Self::last_answer(&value);
            self.finish_step(host, text);
            self.draw(host);
            return;
        }
        if value.get("elapsedMs").is_some() {
            // The wait is over: ask the session what it has.
            self.read_session(host);
        }
    }
}

export_plugin!(BloggerAgentOs);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_says_what_is_missing() {
        assert!(has_sections("# Title\n- one\n- two\n- three").is_ok());
        assert_eq!(has_sections("just a line").unwrap_err(), "the outline has fewer than three sections");
        assert_eq!(long_enough("short").unwrap_err(), "the draft is very short");
        assert!(long_enough(&"x".repeat(700)).is_ok());
        assert_eq!(no_placeholders("a TODO remains").unwrap_err(), "the text still says TODO");
        assert_eq!(no_placeholders("[insert name]").unwrap_err(), "the text still has a placeholder in brackets");
        assert!(no_placeholders("finished text").is_ok());
    }

    #[test]
    fn the_last_thing_the_agent_wrote_is_the_result() {
        let session = serde_json::json!({
            "paneId": 3,
            "turns": [
                { "role": "user", "text": "write it" },
                { "role": "assistant", "text": "first" },
                { "role": "user", "text": "again" },
                { "role": "assistant", "text": "second" }
            ]
        });
        assert_eq!(BloggerAgentOs::last_answer(&session), "second");
        assert_eq!(BloggerAgentOs::last_answer(&serde_json::json!({ "turns": [] })), "");
    }

    #[test]
    fn a_run_is_kept_and_read_back() {
        let run = Run {
            topic: "WebAssembly plugins".into(),
            step: 1,
            state: State::Done,
            pane: Some(7),
            results: vec!["an outline".into()],
            note: "Outline — done".into(),
        };
        let back: Run = serde_json::from_value(serde_json::to_value(&run).unwrap()).unwrap();
        assert_eq!(back.step, 1);
        assert_eq!(back.state, State::Done);
        assert_eq!(back.pane, Some(7));
        assert_eq!(back.results, ["an outline"]);
    }

    /// Every step's prompt is English, and says which language to answer in.
    #[test]
    fn the_skills_are_written_the_way_prompts_in_code_are() {
        for step in STEPS {
            assert!(step.prompt.contains("{language}"), "{} does not say which language to answer in", step.id);
            assert!(step.prompt.is_ascii(), "{} is not written in English", step.id);
        }
        assert!(STEPS[0].prompt.contains("{topic}"), "the first step asks about the topic");
    }
}

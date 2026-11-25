//! Bots that start work when something happens: a new issue, a new pull request, a failed CI run,
//! a new commit on the remote, a new file in the bot's folder.
//!
//! Agentty looks, not the agent: every few minutes it asks `gh` or `git` (a fixed command per kind
//! of watch, never a line the user or a bot wrote) or lists the folder, and compares with what it
//! saw before. Only when something is new does the bot get a message, like any other in its
//! mailbox, so a quiet repository costs no agent turn. The first look only notes what is there.
//!
//! What was seen is kept in the bot's folder, so a restart never reports the same thing twice.

use super::bots::Origin;
use super::Workbench;
use agentty_bridge::bots::WatchKind;
use gpui::{AppContext as _, Context};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How often the watches look.
const LOOK_EVERY: Duration = Duration::from_secs(5 * 60);
/// What is remembered as seen, per watch.
const SEEN_MAX: usize = 500;
/// New things named in one message; the rest are counted.
const TOLD_MAX: usize = 10;
/// Entries of a folder that are read.
const FILES_MAX: usize = 2000;
const SEEN_FILE: &str = "watch.json";

pub const KINDS: [WatchKind; 5] = [WatchKind::Issues, WatchKind::PullRequests, WatchKind::CiFailures, WatchKind::Commits, WatchKind::Files];

/// The name of a kind of watch, for the settings and the cards.
pub fn label(kind: WatchKind) -> &'static str {
    match kind {
        WatchKind::Issues => "bots.watch.issues",
        WatchKind::PullRequests => "bots.watch.pulls",
        WatchKind::CiFailures => "bots.watch.ci",
        WatchKind::Commits => "bots.watch.commits",
        WatchKind::Files => "bots.watch.files",
    }
}

/// What a kind is called in the message to the bot.
fn noun(kind: WatchKind) -> &'static str {
    match kind {
        WatchKind::Issues => "open issues",
        WatchKind::PullRequests => "open pull requests",
        WatchKind::CiFailures => "failed CI runs",
        WatchKind::Commits => "commits on the remote's default branch",
        WatchKind::Files => "files in your folder",
    }
}

/// The program and arguments that list what a kind watches. `Files` reads the folder instead.
fn command(kind: WatchKind) -> Option<(&'static str, &'static [&'static str])> {
    Some(match kind {
        WatchKind::Issues => ("gh", &["issue", "list", "--state", "open", "--limit", "30", "--json", "number,title,url"]),
        WatchKind::PullRequests => ("gh", &["pr", "list", "--state", "open", "--limit", "30", "--json", "number,title,url"]),
        WatchKind::CiFailures => {
            ("gh", &["run", "list", "--limit", "20", "--json", "databaseId,displayTitle,workflowName,conclusion,headBranch,url"])
        }
        WatchKind::Commits => ("git", &["ls-remote", "origin", "HEAD"]),
        WatchKind::Files => return None,
    })
}

/// What a command's output lists: a key that stays the same for the same thing, and the line the
/// bot is told.
pub fn items(kind: WatchKind, output: &str) -> Vec<(String, String)> {
    let text = |value: &serde_json::Value, key: &str| value.get(key).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let rows = || serde_json::from_str::<Vec<serde_json::Value>>(output).unwrap_or_default();
    match kind {
        WatchKind::Issues | WatchKind::PullRequests => rows()
            .iter()
            .filter_map(|row| {
                let number = row.get("number")?.as_u64()?;
                Some((number.to_string(), format!("#{number} {} {}", one_line(&text(row, "title")), text(row, "url"))))
            })
            .collect(),
        WatchKind::CiFailures => rows()
            .iter()
            .filter(|row| text(row, "conclusion") == "failure")
            .filter_map(|row| {
                let id = row.get("databaseId")?.as_u64()?;
                let line = format!(
                    "{} · {} · {} {}",
                    one_line(&text(row, "workflowName")),
                    one_line(&text(row, "displayTitle")),
                    one_line(&text(row, "headBranch")),
                    text(row, "url")
                );
                Some((id.to_string(), line))
            })
            .collect(),
        WatchKind::Commits => output
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|sha| sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()))
            .take(1)
            .map(|sha| (sha.to_string(), format!("the remote is now at {sha}")))
            .collect(),
        WatchKind::Files => Vec::new(),
    }
}

/// A title from outside, on one line and not too long: it goes into a message to an agent.
fn one_line(text: &str) -> String {
    agentty_bridge::fsutil::one_line(text, 160)
}

/// What is there now. An error says why it could not be looked at (no `gh`, not signed in, not a
/// repository), in the tool's own words.
fn look(kind: WatchKind, folder: &Path) -> Result<Vec<(String, String)>, String> {
    let Some((program, arguments)) = command(kind) else {
        let mut names: Vec<String> = std::fs::read_dir(folder)
            .map_err(|err| err.to_string())?
            .flatten()
            .take(FILES_MAX)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with('.'))
            .collect();
        names.sort();
        return Ok(names.into_iter().map(|name| (name.clone(), one_line(&name))).collect());
    };
    let output = agentty_bridge::process::command(program)
        .args(arguments)
        .current_dir(folder)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|err| format!("{program}: {err}"))?;
    if !output.status.success() {
        let why = String::from_utf8_lossy(&output.stderr);
        return Err(one_line(why.lines().find(|line| !line.trim().is_empty()).unwrap_or("it failed")));
    }
    Ok(items(kind, &String::from_utf8_lossy(&output.stdout)))
}

/// The lines of what is new since `seen`, which then holds what is there now too. The first look
/// (`None`) is the baseline: nothing in it is new.
pub fn fresh(seen: &mut Option<Vec<String>>, now: &[(String, String)]) -> Vec<String> {
    let Some(known) = seen else {
        *seen = Some(now.iter().map(|(key, _)| key.clone()).rev().take(SEEN_MAX).collect());
        return Vec::new();
    };
    let new: Vec<&(String, String)> = now.iter().filter(|(key, _)| !known.contains(key)).collect();
    for (key, _) in &new {
        known.push(key.clone());
    }
    if known.len() > SEEN_MAX {
        known.drain(..known.len() - SEEN_MAX);
    }
    new.into_iter().map(|(_, line)| line.clone()).collect()
}

/// The message a bot gets about what is new.
pub fn message(kind: WatchKind, lines: &[String]) -> String {
    let mut text = format!("New {}:", noun(kind));
    for line in lines.iter().take(TOLD_MAX) {
        text.push_str("\n- ");
        text.push_str(line);
    }
    if lines.len() > TOLD_MAX {
        text.push_str(&format!("\n- and {} more", lines.len() - TOLD_MAX));
    }
    text
}

type Seen = HashMap<WatchKind, Vec<String>>;

fn read_seen(dir: &Path) -> Seen {
    std::fs::read(dir.join(SEEN_FILE)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// One round over a bot's watches, off the main thread: what is new, and what could not be looked at.
fn look_round(dir: PathBuf, folder: PathBuf, kinds: Vec<WatchKind>) -> Vec<(WatchKind, Result<Vec<String>, String>)> {
    let mut seen = read_seen(&dir);
    let before = seen.clone();
    let mut out = Vec::new();
    for kind in kinds {
        out.push((
            kind,
            look(kind, &folder).map(|now| {
                let mut known = seen.remove(&kind);
                let new = fresh(&mut known, &now);
                seen.insert(kind, known.unwrap_or_default());
                new
            }),
        ));
    }
    if seen != before {
        if let Ok(json) = serde_json::to_vec(&seen) {
            let _ = agentty_bridge::fsutil::write_private(&dir.join(SEEN_FILE), &json);
        }
    }
    out
}

#[derive(Default)]
pub struct WatchState {
    /// Bumped when the bots change: the loop of an older round ends.
    round: u64,
    /// Watches that could not look, and why: the bot's settings say it.
    pub(super) trouble: HashMap<(String, WatchKind), String>,
}

impl Workbench {
    /// The bots or their watches changed (or the window just opened). Only the window that hosts
    /// the bots looks, and only while some bot that is switched on watches something.
    pub(super) fn bot_watches_changed(&mut self, cx: &mut Context<Self>) {
        self.bots.watch.round += 1;
        let round = self.bots.watch.round;
        let any = self.bots.list.iter().any(|bot| bot.enabled && !bot.watches.is_empty());
        if !any || !self.prefs(cx).bots_enabled || !self.hosts_bots() {
            if !self.bots.watch.trouble.is_empty() {
                self.bots.watch.trouble.clear();
                cx.notify();
            }
            return;
        }
        cx.spawn(async move |this, cx| loop {
            let Ok(Some(jobs)) = this.update(cx, |this, _| {
                (this.bots.watch.round == round).then(|| {
                    this.bots
                        .list
                        .iter()
                        .filter(|bot| bot.enabled && !bot.watches.is_empty())
                        .filter_map(|bot| {
                            Some((bot.handle.clone(), crate::bots::store().dir(&bot.handle).ok()?, bot.folder.clone(), bot.watches.clone()))
                        })
                        .collect::<Vec<_>>()
                })
            }) else {
                break;
            };
            let found = cx
                .background_spawn(async move {
                    jobs.into_iter().map(|(handle, dir, folder, kinds)| (handle, look_round(dir, folder, kinds))).collect::<Vec<_>>()
                })
                .await;
            let alive = this.update(cx, |this, cx| {
                if this.bots.watch.round != round {
                    return false;
                }
                let mut trouble = HashMap::new();
                for (handle, results) in found {
                    for (kind, result) in results {
                        match result {
                            Ok(lines) if lines.is_empty() => {}
                            Ok(lines) => {
                                if let Err(why) = this.bot_post(&handle, Origin::Watch(kind), message(kind, &lines), 0, cx) {
                                    eprintln!("agentty: bot @{handle} watch not passed on: {why}");
                                }
                            }
                            Err(why) => {
                                trouble.insert((handle.clone(), kind), why);
                            }
                        }
                    }
                }
                // Nothing is drawn again for a look that found what the last one did.
                if trouble != this.bots.watch.trouble {
                    this.bots.watch.trouble = trouble;
                    cx.notify();
                }
                true
            });
            if !matches!(alive, Ok(true)) {
                break;
            }
            cx.background_executor().timer(LOOK_EVERY).await;
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_look_is_the_baseline_and_only_what_comes_after_is_new() {
        let listed = |numbers: &[u32]| numbers.iter().map(|n| (n.to_string(), format!("#{n}"))).collect::<Vec<_>>();
        let mut seen = None;
        assert!(fresh(&mut seen, &listed(&[1, 2])).is_empty());
        assert!(fresh(&mut seen, &listed(&[1, 2])).is_empty());
        assert_eq!(fresh(&mut seen, &listed(&[3, 2, 1])), vec!["#3"]);
        // One that was closed and is listed no more is not new when it is listed again.
        assert!(fresh(&mut seen, &listed(&[3])).is_empty());
        assert!(fresh(&mut seen, &listed(&[1, 3])).is_empty());
    }

    #[test]
    fn what_was_seen_is_capped() {
        let mut seen = Some(Vec::new());
        let many: Vec<(String, String)> = (0..SEEN_MAX + 20).map(|n| (n.to_string(), n.to_string())).collect();
        assert_eq!(fresh(&mut seen, &many).len(), SEEN_MAX + 20);
        assert_eq!(seen.as_ref().unwrap().len(), SEEN_MAX);
        // The newest are the ones kept.
        assert!(fresh(&mut seen, &many[many.len() - 5..]).is_empty());
    }

    #[test]
    fn a_listing_is_read_into_keys_and_lines() {
        let issues = r#"[{"number":41,"title":"Pay button\nignores first tap","url":"https://example.com/i/41"}]"#;
        assert_eq!(
            items(WatchKind::Issues, issues),
            vec![("41".to_string(), "#41 Pay button ignores first tap https://example.com/i/41".to_string())]
        );
        let runs = r#"[{"databaseId":7,"displayTitle":"fix","workflowName":"CI","conclusion":"failure","headBranch":"main","url":"u"},
                       {"databaseId":8,"displayTitle":"ok","workflowName":"CI","conclusion":"success","headBranch":"main","url":"u"}]"#;
        assert_eq!(items(WatchKind::CiFailures, runs), vec![("7".to_string(), "CI · fix · main u".to_string())]);
        let head = "0123456789abcdef0123456789abcdef01234567\tHEAD\n";
        assert_eq!(items(WatchKind::Commits, head)[0].0, "0123456789abcdef0123456789abcdef01234567");
        // What is not a listing gives nothing, never a panic.
        assert!(items(WatchKind::PullRequests, "gh: not logged in").is_empty());
        assert!(items(WatchKind::Commits, "fatal: not a git repository").is_empty());
    }

    #[test]
    fn a_long_list_is_cut_in_the_message() {
        let lines: Vec<String> = (0..TOLD_MAX + 3).map(|n| format!("#{n}")).collect();
        let text = message(WatchKind::Issues, &lines);
        assert!(text.starts_with("New open issues:\n- #0"));
        assert!(text.ends_with("- and 3 more"));
    }

    #[test]
    fn a_folder_is_watched_by_its_names() {
        let dir = std::env::temp_dir().join(format!("agentty-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"").unwrap();
        std::fs::write(dir.join(".hidden"), b"").unwrap();
        let mut seen = None;
        assert!(fresh(&mut seen, &look(WatchKind::Files, &dir).unwrap()).is_empty());
        std::fs::write(dir.join("b.txt"), b"").unwrap();
        assert_eq!(fresh(&mut seen, &look(WatchKind::Files, &dir).unwrap()), vec!["b.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

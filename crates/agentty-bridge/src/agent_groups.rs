//! Agent groups: a team of agents with a role each, opened together in one project.
//!
//! A group is one JSON file — the package people share. Installed groups live in
//! `<data dir>/agent-groups/<id>.json`. A group is either for any project (`"scope": "any"`) or for
//! one repository (`"scope": "owner/name"`, matched against the project's `origin` remote).
//!
//! ```json
//! {
//!   "name": "E-ticket team",
//!   "description": "Build, review and ship the e-ticket service",
//!   "scope": "acme/eticket",
//!   "notes": "Deploy with ./scripts/deploy.sh <env>. Staging first.",
//!   "agents": [
//!     { "name": "Dev", "role": "Implements features and fixes bugs", "badge": "Build", "agent": "claude" },
//!     { "name": "Deploy", "role": "Builds and deploys to staging and production", "badge": "Deploy", "agent": "claude",
//!       "instructions": "Never deploy to production without the user's OK." }
//!   ]
//! }
//! ```
//!
//! The first agent is the main one: its pane starts enlarged (focus view), the others beside it.
//! Every member gets its role as its first message, with the other members' roles: work that belongs
//! to another member is handed to it (`agentty group send`), work nobody's role covers is not done.

use crate::fsutil;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MAX_AGENTS: usize = 6;
const MAX_NAME: usize = 40;
const MAX_ROLE: usize = 300;
const MAX_BADGE: usize = 16;
const MAX_TEXT: usize = 20_000;
/// A package is a small JSON file; anything bigger is not one.
const MAX_FILE: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentGroup {
    /// File name in the groups folder; derived from the name when a package is installed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// `any`, or the repository (`owner/name`) the group is for.
    #[serde(default)]
    pub scope: Scope,
    /// Shared notes every member reads: how to build, deploy, where things are.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    pub agents: Vec<GroupAgent>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Scope {
    /// Usable in every project.
    #[default]
    Any,
    /// Only in the repository `owner/name` (compared without case).
    Repo(String),
}

impl Serialize for Scope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Scope::Any => serializer.serialize_str("any"),
            Scope::Repo(repo) => serializer.serialize_str(repo),
        }
    }
}

impl<'de> Deserialize<'de> for Scope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let text = text.trim();
        Ok(if text.is_empty() || text.eq_ignore_ascii_case("any") { Scope::Any } else { Scope::Repo(text.to_string()) })
    }
}

impl Scope {
    /// Whether a project whose repository is `repo` (`owner/name`, when it has one) may use the group.
    pub fn allows(&self, repo: Option<&str>) -> bool {
        match self {
            Scope::Any => true,
            Scope::Repo(wanted) => repo.is_some_and(|repo| repo.eq_ignore_ascii_case(wanted)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupAgent {
    /// Shown on the pane and used to hand work over (`agentty group send <name> …`).
    pub name: String,
    /// One line: what this member does.
    pub role: String,
    /// A word or two for the role badge on the pane ("Deploy", "QA"); the start of `role` when missing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub badge: String,
    /// `claude` (default) or `codex`.
    #[serde(default = "default_agent")]
    pub agent: String,
    /// Extra rules for this member.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instructions: String,
}

impl GroupAgent {
    /// What the role badge says: the badge, or the first words of the role.
    pub fn badge_text(&self) -> String {
        if !self.badge.is_empty() {
            return self.badge.clone();
        }
        let mut text = String::new();
        for word in self.role.split_whitespace() {
            if !text.is_empty() && text.chars().count() + 1 + word.chars().count() > MAX_BADGE {
                break;
            }
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(word);
        }
        text.chars().take(MAX_BADGE).collect()
    }
}

fn default_agent() -> String {
    "claude".into()
}

fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string()
}

impl AgentGroup {
    /// The group as it can be used, or why not: bounded sizes, known agents, distinct member names.
    pub fn checked(mut self) -> Result<AgentGroup> {
        self.name = clean(&self.name);
        if self.name.is_empty() || self.name.chars().count() > MAX_NAME {
            bail!("a group needs a name of 1–{MAX_NAME} characters");
        }
        self.description = clean(&self.description);
        if self.description.chars().count() > MAX_ROLE {
            bail!("the description is longer than {MAX_ROLE} characters");
        }
        if let Scope::Repo(repo) = &self.scope {
            let valid = repo.split('/').count() == 2
                && repo.split('/').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)));
            if !valid {
                bail!("scope must be \"any\" or a repository as owner/name");
            }
        }
        if self.notes.chars().count() > MAX_TEXT {
            bail!("the notes are longer than {MAX_TEXT} characters");
        }
        if self.agents.is_empty() || self.agents.len() > MAX_AGENTS {
            bail!("a group has 1–{MAX_AGENTS} agents");
        }
        let mut names: Vec<String> = Vec::new();
        for agent in &mut self.agents {
            agent.name = clean(&agent.name);
            agent.role = clean(&agent.role);
            agent.badge = clean(&agent.badge);
            agent.agent = agent.agent.trim().to_lowercase();
            if agent.name.is_empty() || agent.name.chars().count() > MAX_NAME {
                bail!("every agent needs a name of 1–{MAX_NAME} characters");
            }
            if agent.role.is_empty() || agent.role.chars().count() > MAX_ROLE {
                bail!("agent \"{}\": the role must be 1–{MAX_ROLE} characters", agent.name);
            }
            if agent.badge.chars().count() > MAX_BADGE {
                bail!("agent \"{}\": the badge is longer than {MAX_BADGE} characters", agent.name);
            }
            if !matches!(agent.agent.as_str(), "claude" | "codex") {
                bail!("agent \"{}\": agent must be claude or codex", agent.name);
            }
            if agent.instructions.chars().count() > MAX_TEXT {
                bail!("agent \"{}\": the instructions are longer than {MAX_TEXT} characters", agent.name);
            }
            let key = agent.name.to_lowercase();
            if names.contains(&key) {
                bail!("two agents are named \"{}\"", agent.name);
            }
            names.push(key);
        }
        if self.id.is_empty() || !is_valid_id(&self.id) {
            self.id = id_from_name(&self.name);
        }
        Ok(self)
    }

    /// The member called `name` (without case).
    pub fn member(&self, name: &str) -> Option<&GroupAgent> {
        let name = name.trim();
        self.agents.iter().find(|a| a.name.eq_ignore_ascii_case(name))
    }

    /// The first message of member `index`: who it is, who else is in the group and the rules.
    /// `language` is the language to talk to the user in.
    pub fn role_prompt(&self, index: usize, language: &str) -> String {
        let me = &self.agents[index];
        let mut prompt = format!(
            "You are \"{}\", one member of the agent group \"{}\" working in this project. Talk to me in {language}.\n\n\
             Your role: {}\n",
            me.name, self.name, me.role
        );
        if !me.instructions.trim().is_empty() {
            prompt.push_str(&format!("\nYour instructions:\n{}\n", me.instructions.trim()));
        }
        prompt.push_str("\nThe members of the group:\n");
        for (i, agent) in self.agents.iter().enumerate() {
            let you = if i == index { " (you)" } else { "" };
            prompt.push_str(&format!("- \"{}\"{you}: {}\n", agent.name, agent.role));
        }
        if !self.notes.trim().is_empty() {
            prompt.push_str(&format!("\nNotes for the whole group:\n{}\n", self.notes.trim()));
        }
        prompt.push_str(
            "\nHow the group works:\n\
             - Read every request of mine as a request to you in your role. \"Deploy e-ticket\" said to a deploy \
             agent means: do the deployment work for e-ticket, the way this project deploys.\n\
             - Do only work that belongs to your role. When a request belongs to another member's role, do not do it \
             yourself: hand it over with `agentty group send \"<member>\" \"<the request, with the context it needs>\"` \
             and tell me you handed it to that member.\n\
             - When no member's role covers a request, do not do it: tell me which roles the group has.\n\
             - Other members share what they do with you as the work goes. Use it; don't redo their work.\n\
             - `agentty group list` shows the members and whether they are working.\n\n\
             Reply with one short line saying who you are and that you are ready, then wait for my request.",
        );
        prompt
    }
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A file name for a group: its name in lowercase ASCII with dashes, or a hash of it for names in
/// other scripts.
pub fn id_from_name(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug: String = slug.chars().take(40).collect();
    if slug.len() >= 3 {
        return slug;
    }
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    format!("group-{:08x}", hasher.finish() as u32)
}

pub fn groups_dir() -> PathBuf {
    fsutil::data_dir().join("agent-groups")
}

/// Reads and checks a package file.
pub fn read(path: &Path) -> Result<AgentGroup> {
    let size = std::fs::metadata(path).with_context(|| path.display().to_string())?.len();
    if size > MAX_FILE {
        bail!("{} is not an agent group (too big)", path.display());
    }
    let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    let group: AgentGroup = serde_json::from_str(&text).with_context(|| format!("{} is not an agent group", path.display()))?;
    group.checked()
}

/// Every installed group, by name; files that are not valid groups are left out.
pub fn list() -> Vec<AgentGroup> {
    list_in(&groups_dir())
}

fn list_in(dir: &Path) -> Vec<AgentGroup> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut groups: Vec<AgentGroup> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|path| {
            let mut group = read(&path).ok()?;
            group.id = path.file_stem()?.to_string_lossy().to_string();
            Some(group)
        })
        .collect();
    groups.sort_by_key(|g| g.name.to_lowercase());
    groups
}

/// The groups a project may use: for any project, or for its repository.
pub fn available_for(project: &Path) -> Vec<AgentGroup> {
    let repo = repo_slug(project);
    list().into_iter().filter(|g| g.scope.allows(repo.as_deref())).collect()
}

/// `owner/name` of the project's `origin` remote.
pub fn repo_slug(project: &Path) -> Option<String> {
    slug_of_web_url(&crate::git::remote_web_url(project)?)
}

fn slug_of_web_url(url: &str) -> Option<String> {
    let path = url.split_once("://").map_or(url, |(_, rest)| rest);
    let mut parts: Vec<&str> = path.trim_end_matches('/').split('/').skip(1).filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return None;
    }
    let name = parts.pop()?;
    let owner = parts.pop()?;
    Some(format!("{owner}/{name}"))
}

/// Installs a package (a copy in the groups folder, `0600`); a group with the same id is replaced.
pub fn install(group: AgentGroup) -> Result<AgentGroup> {
    save_in(&groups_dir(), group)
}

fn save_in(dir: &Path, group: AgentGroup) -> Result<AgentGroup> {
    let group = group.checked()?;
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(&AgentGroup { id: String::new(), ..group.clone() })?;
    fsutil::write_private(&dir.join(format!("{}.json", group.id)), json.as_bytes())?;
    Ok(group)
}

/// Installs the package at `path`.
pub fn install_file(path: &Path) -> Result<AgentGroup> {
    let mut group = read(path)?;
    group.id = id_from_name(&group.name);
    install(group)
}

/// The sample group written the first time the groups folder is empty, so there is one to try and
/// a file to copy.
pub fn sample() -> AgentGroup {
    AgentGroup {
        id: "dev-team".into(),
        name: "Dev team".into(),
        description: "Build, review and deploy — one agent each".into(),
        scope: Scope::Any,
        notes: String::new(),
        agents: vec![
            GroupAgent {
                name: "Dev".into(),
                role: "Implements features and fixes bugs in this project".into(),
                badge: "Build & fix".into(),
                agent: "claude".into(),
                instructions: String::new(),
            },
            GroupAgent {
                name: "Review".into(),
                role: "Reviews changes for bugs, security and readability, and runs the tests".into(),
                badge: "Review & test".into(),
                agent: "claude".into(),
                instructions: "Report findings; leave fixes to Dev unless they are one-line typos.".into(),
            },
            GroupAgent {
                name: "Deploy".into(),
                role: "Builds, releases and deploys this project".into(),
                badge: "Release".into(),
                agent: "claude".into(),
                instructions: "Find how this project deploys (scripts, CI, docs) before the first deploy. \
                               Never deploy to production without the user's OK."
                    .into(),
            },
        ],
    }
}

/// Writes the sample when there is no group yet. Returns whether it did.
pub fn ensure_sample() -> bool {
    let dir = groups_dir();
    if dir.exists() {
        return false;
    }
    save_in(&dir, sample()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(json: &str) -> Result<AgentGroup> {
        serde_json::from_str::<AgentGroup>(json).map_err(anyhow::Error::from).and_then(AgentGroup::checked)
    }

    #[test]
    fn a_package_reads_with_defaults() {
        let g = group(r#"{"name":"Team","agents":[{"name":"Dev","role":"Builds"}]}"#).unwrap();
        assert_eq!(g.scope, Scope::Any);
        assert_eq!(g.agents[0].agent, "claude");
        assert_eq!(g.id, "team");
        let repo = group(r#"{"name":"T","scope":"acme/eticket","agents":[{"name":"Dev","role":"Builds","agent":"Codex"}]}"#).unwrap();
        assert_eq!(repo.scope, Scope::Repo("acme/eticket".into()));
        assert_eq!(repo.agents[0].agent, "codex");
    }

    #[test]
    fn bad_packages_are_refused() {
        assert!(group(r#"{"name":"","agents":[{"name":"Dev","role":"Builds"}]}"#).is_err());
        assert!(group(r#"{"name":"T","agents":[]}"#).is_err());
        assert!(group(r#"{"name":"T","agents":[{"name":"Dev","role":"Builds","agent":"bash"}]}"#).is_err());
        assert!(group(r#"{"name":"T","agents":[{"name":"Dev","role":"a"},{"name":"dev","role":"b"}]}"#).is_err());
        assert!(group(r#"{"name":"T","scope":"not a repo","agents":[{"name":"Dev","role":"a"}]}"#).is_err());
        assert!(group(r#"{"name":"T","scope":"../../etc","agents":[{"name":"Dev","role":"a"}]}"#).is_err());
        let seven: Vec<String> = (0..7).map(|i| format!(r#"{{"name":"A{i}","role":"r"}}"#)).collect();
        assert!(group(&format!(r#"{{"name":"T","agents":[{}]}}"#, seven.join(","))).is_err());
    }

    #[test]
    fn the_badge_falls_back_to_the_first_words_of_the_role() {
        let g =
            group(r#"{"name":"T","agents":[{"name":"A","role":"Builds and deploys the service"},{"name":"B","role":"r","badge":"QA"}]}"#)
                .unwrap();
        assert_eq!(g.agents[0].badge_text(), "Builds and");
        assert_eq!(g.agents[1].badge_text(), "QA");
        assert!(group(r#"{"name":"T","agents":[{"name":"A","role":"r","badge":"a very long badge text"}]}"#).is_err());
    }

    #[test]
    fn scope_matches_the_repository_without_case() {
        let repo = Scope::Repo("Acme/ETicket".into());
        assert!(repo.allows(Some("acme/eticket")));
        assert!(!repo.allows(Some("acme/other")));
        assert!(!repo.allows(None));
        assert!(Scope::Any.allows(None));
    }

    #[test]
    fn repository_names_come_from_web_urls() {
        assert_eq!(slug_of_web_url("https://github.com/acme/eticket").as_deref(), Some("acme/eticket"));
        assert_eq!(slug_of_web_url("https://gitlab.com/group/sub/eticket/").as_deref(), Some("sub/eticket"));
        assert_eq!(slug_of_web_url("https://github.com/acme"), None);
    }

    #[test]
    fn ids_are_safe_file_names() {
        assert_eq!(id_from_name("E-ticket Team!"), "e-ticket-team");
        let korean = id_from_name("배포 팀");
        assert!(korean.starts_with("group-") && is_valid_id(&korean));
        assert!(!is_valid_id("../x"));
    }

    #[test]
    fn the_role_prompt_names_every_member_and_the_rules() {
        let g = sample();
        let prompt = g.role_prompt(2, "Korean");
        assert!(prompt.contains("You are \"Deploy\""));
        assert!(prompt.contains("\"Dev\": Implements"));
        assert!(prompt.contains("\"Deploy\" (you)"));
        assert!(prompt.contains("agentty group send"));
        assert!(prompt.contains("Talk to me in Korean"));
    }

    #[test]
    fn installed_groups_are_listed_and_broken_files_skipped() {
        let dir = std::env::temp_dir().join(format!("agentty-groups-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        save_in(&dir, sample()).unwrap();
        std::fs::write(dir.join("broken.json"), "{").unwrap();
        let groups = list_in(&dir);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, "dev-team");
        std::fs::remove_dir_all(&dir).ok();
    }
}

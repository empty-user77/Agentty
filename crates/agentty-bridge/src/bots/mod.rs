//! Bots: named agents that keep their instructions, their memory and their conversations between
//! runs. A bot is one folder in Agentty's data folder, `bots/<handle>/`, made of files a person
//! can read and edit:
//!
//! | file | holds | written by |
//! |---|---|---|
//! | `bot.json` | the [`Bot`] itself | the app only |
//! | `SOUL.md` | its prompt: what the bot is for and how it works | the user |
//! | `RULES.md` | what always holds while it works | the user |
//! | `CONTEXT.md` | what it should know | the user |
//! | `MEMORY.md` | what it learned while working | the bot and the user |
//! | `skills/` | its skills: the bundled ones and copies of the user's own ([`skills`]) | the app, for the user |
//! | `threads.json` | its conversations and their agent sessions | the app |
//!
//! Nothing here starts an agent or knows about the UI: this is the store. Tokens of a bot's
//! channels are never in these files (they live in the system keychain).

pub mod channels;
pub mod guide;
pub mod skills;

use crate::fsutil;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

/// The longest handle: it is a folder name and is typed after `@`.
pub const HANDLE_MAX: usize = 32;
pub const NAME_MAX: usize = 64;
pub const PURPOSE_MAX: usize = 200;
/// How much of `MEMORY.md` a bot is given. What is past it stays in the file.
pub const MEMORY_LIMIT: usize = 16 * 1024;
/// Conversations a bot keeps; the ones not touched for the longest go first.
pub const THREADS_MAX: usize = 200;
/// A `bot.json` or `threads.json` larger than this is not one Agentty wrote.
const JSON_LIMIT: u64 = 1024 * 1024;

const BOT_FILE: &str = "bot.json";
/// The characters a bot can be drawn as.
pub const AVATARS: [&str; 9] = ["cat", "bunny", "bear", "frog", "owl", "fox", "chick", "robot", "dog"];
/// The colours a character can stand on; each character's own is the one at its place in
/// [`AVATARS`].
pub const AVATAR_COLORS: [(&str, u32); 9] = [
    ("amber", 0xf6b756),
    ("pink", 0xf4a6c0),
    ("sky", 0x8fc1f2),
    ("lime", 0xc9ee9b),
    ("violet", 0xc7b4f3),
    ("mint", 0x86d6c4),
    ("blue", 0x9fd3ff),
    ("coral", 0xff9f8a),
    ("cream", 0xf3e3b3),
];

/// The kinds of picture a bot can be given, and the largest one taken.
pub const IMAGE_TYPES: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];
pub const IMAGE_MAX: u64 = 5 * 1024 * 1024;

const SOUL_FILE: &str = "SOUL.md";
const MEMORY_FILE: &str = "MEMORY.md";
const THREADS_FILE: &str = "threads.json";
const RULES_FILE: &str = "RULES.md";
const CONTEXT_FILE: &str = "CONTEXT.md";

/// The most a part the user writes for a bot holds.
pub const PART_LIMIT: usize = 32 * 1024;

/// What the user writes for a bot, each in a file of its own: what it should do, what always
/// holds while it does it, and what it should know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    Prompt,
    Rules,
    Context,
}

pub const PARTS: [Part; 3] = [Part::Prompt, Part::Rules, Part::Context];

impl Part {
    pub fn file(self) -> &'static str {
        match self {
            Part::Prompt => SOUL_FILE,
            Part::Rules => RULES_FILE,
            Part::Context => CONTEXT_FILE,
        }
    }
}
const SKILLS_DIR: &str = "skills";

/// The agent CLI a bot runs as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotAgent {
    Claude,
    Codex,
}

/// Where a bot works: in its folder as it is, or in a git worktree made for each job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotWorkspace {
    #[default]
    Folder,
    Worktree,
}

/// What a bot may do without the user. Only the app's UI changes it: no message does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotPermission {
    /// The agent's own approval flow: it asks, the user answers.
    #[default]
    Ask,
    /// It reads and runs what it needs to judge work, and edits nothing.
    ReadOnly,
    /// Files in its folder only: no shell, no web, no MCP tools.
    Files,
}

/// A time the bot starts work by itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotSchedule {
    pub id: String,
    /// Five-field cron expression.
    pub cron: String,
    pub prompt: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

/// Something a bot watches for, to start work when it appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchKind {
    /// A new open issue of the repository in the bot's folder.
    Issues,
    PullRequests,
    /// A CI run that failed.
    CiFailures,
    /// A new commit on the remote's default branch.
    Commits,
    /// A new file or folder directly in the bot's folder.
    Files,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Telegram,
    Slack,
    Discord,
}

/// A place outside the app the bot takes messages from. Its token is in the keychain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotChannel {
    pub kind: ChannelKind,
    /// The only senders it listens to. Never empty: a channel open to anyone is not allowed.
    pub allowed_senders: Vec<String>,
    /// Whether what the bot did by itself (a schedule, a watch) is reported on this channel.
    #[serde(default = "yes")]
    pub reports: bool,
    /// Where those reports go: the chat an allowed sender last wrote to the bot from. Empty until
    /// one has.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub report_chat: String,
}

/// The longest chat address kept for a channel's reports.
pub const REPORT_CHAT_MAX: usize = 120;

impl BotChannel {
    pub fn new(kind: ChannelKind, allowed_senders: Vec<String>) -> Self {
        BotChannel { kind, allowed_senders, reports: true, report_chat: String::new() }
    }

    /// Whether `chat` has the shape of a chat's address on a channel: ids, with the few marks the
    /// channels put between them.
    pub fn chat_looks_right(chat: &str) -> bool {
        !chat.is_empty() && chat.len() <= REPORT_CHAT_MAX && chat.chars().all(|c| c.is_ascii_alphanumeric() || "-._/".contains(c))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bot {
    /// Its unique name after `@`, and its folder's name: lowercase letters, digits and `-`.
    pub handle: String,
    /// The name shown.
    pub name: String,
    /// One line on what it is for; other bots read it to choose whom to hand work to.
    #[serde(default)]
    pub purpose: String,
    /// The character drawn for it, one of [`AVATARS`]. Empty: the one its handle picks.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub avatar: String,
    /// The colour behind its character, one of [`AVATAR_COLORS`]. Empty: the character's own.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub color: String,
    /// A picture of the user's own in place of the character: a file in the bot's folder
    /// (`avatar-….png`), copied there by [`BotStore::set_image`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub image: String,
    pub agent: BotAgent,
    /// Empty: the agent's own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The project folder it works in.
    pub folder: PathBuf,
    #[serde(default)]
    pub workspace: BotWorkspace,
    #[serde(default)]
    pub permission: BotPermission,
    #[serde(default)]
    pub schedules: Vec<BotSchedule>,
    /// What it watches for: it starts work when something new appears.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watches: Vec<WatchKind>,
    #[serde(default)]
    pub channels: Vec<BotChannel>,
    /// Off: it takes no new messages and its schedules do not run.
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// What is wrong with a bot's definition. The UI words it; `Display` is for logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalid {
    /// Not 1 to [`HANDLE_MAX`] of `a-z`, `0-9` and `-`, or `-` at either end.
    Handle,
    /// A name Windows keeps for devices (`con`, `nul`, `com1`…): it cannot be a folder there.
    HandleReserved,
    Name,
    Purpose,
    Model,
    /// Not an absolute path.
    Folder,
    Schedule,
    /// A channel named twice, one without allowed senders, or a report chat that is no chat's address.
    Channel,
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Invalid::Handle => "the handle takes 1 to 32 lowercase letters, digits and hyphens, and no hyphen at either end",
            Invalid::HandleReserved => "the handle is a name the system reserves",
            Invalid::Name => "the name is empty or too long",
            Invalid::Purpose => "the purpose is more than one line or too long",
            Invalid::Model => "the model is not a model name",
            Invalid::Folder => "the folder is not an absolute path",
            Invalid::Schedule => "a schedule has no id, expression or prompt, or shares its id with another",
            Invalid::Channel => "a channel is listed twice or has no allowed senders",
        })
    }
}

impl std::error::Error for Invalid {}

/// Whether `handle` can name a bot (and so its folder).
pub fn check_handle(handle: &str) -> Result<(), Invalid> {
    let shaped = !handle.is_empty()
        && handle.len() <= HANDLE_MAX
        && handle.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !handle.starts_with('-')
        && !handle.ends_with('-');
    if !shaped {
        return Err(Invalid::Handle);
    }
    let device = matches!(handle, "con" | "prn" | "aux" | "nul")
        || (handle.len() == 4
            && (handle.starts_with("com") || handle.starts_with("lpt"))
            && handle.as_bytes()[3].is_ascii_digit()
            && handle.as_bytes()[3] != b'0');
    if device {
        return Err(Invalid::HandleReserved);
    }
    Ok(())
}

/// A handle made from a display name: `Release Check` → `release-check`. `None` when nothing of
/// the name can be kept (a name without Latin letters or digits); the user then types one.
pub fn handle_from_name(name: &str) -> Option<String> {
    let mut handle = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            handle.push(c.to_ascii_lowercase());
        } else if !handle.is_empty() && !handle.ends_with('-') {
            handle.push('-');
        }
        if handle.len() >= HANDLE_MAX {
            break;
        }
    }
    let handle = handle.trim_end_matches('-').to_string();
    check_handle(&handle).ok().map(|()| handle)
}

impl Bot {
    /// A bot as "New bot" makes it: asking before it acts, working in its folder, switched on.
    pub fn new(handle: &str, name: &str, purpose: &str, agent: BotAgent, folder: PathBuf) -> Self {
        Bot {
            handle: handle.to_string(),
            name: name.trim().to_string(),
            purpose: purpose.trim().to_string(),
            avatar: String::new(),
            color: String::new(),
            image: String::new(),
            agent,
            model: None,
            folder,
            workspace: BotWorkspace::default(),
            permission: BotPermission::default(),
            schedules: Vec::new(),
            watches: Vec::new(),
            channels: Vec::new(),
            enabled: true,
        }
    }

    /// The character drawn for this bot: the one chosen, else one its handle picks, the same
    /// every time.
    pub fn avatar(&self) -> &'static str {
        if let Some(chosen) = AVATARS.iter().find(|name| **name == self.avatar) {
            return chosen;
        }
        let sum = self.handle.bytes().fold(0usize, |sum, byte| sum.wrapping_mul(31).wrapping_add(byte as usize));
        AVATARS[sum % AVATARS.len()]
    }

    /// The name of the bot's own picture, when it has one and the name is one `set_image` gives:
    /// `bot.json` can be edited by hand, and a path in it must never be followed.
    pub fn image_file(&self) -> Option<&str> {
        let (stem, extension) = self.image.rsplit_once('.')?;
        let plain = stem.strip_prefix("avatar-").is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()));
        (plain && IMAGE_TYPES.contains(&extension)).then_some(self.image.as_str())
    }

    /// The colour behind the character: the one chosen, else the character's own.
    pub fn avatar_color(&self) -> u32 {
        let own = AVATARS.iter().position(|name| *name == self.avatar()).unwrap_or(0);
        AVATAR_COLORS.iter().find(|(name, _)| *name == self.color).map_or(AVATAR_COLORS[own].1, |(_, color)| *color)
    }

    pub fn validate(&self) -> Result<(), Invalid> {
        check_handle(&self.handle)?;
        let one_line = |text: &str| !text.chars().any(char::is_control);
        if self.name.trim().is_empty() || self.name.chars().count() > NAME_MAX || !one_line(&self.name) {
            return Err(Invalid::Name);
        }
        if self.purpose.chars().count() > PURPOSE_MAX || !one_line(&self.purpose) {
            return Err(Invalid::Purpose);
        }
        if let Some(model) = &self.model {
            // It becomes an argument of the agent's command line: a name, never an option.
            let named = !model.is_empty()
                && model.len() <= 100
                && !model.starts_with('-')
                && model.chars().all(|c| c.is_ascii_alphanumeric() || "._:/[]-".contains(c));
            if !named {
                return Err(Invalid::Model);
            }
        }
        if !self.folder.is_absolute() {
            return Err(Invalid::Folder);
        }
        for (at, schedule) in self.schedules.iter().enumerate() {
            let filled = !schedule.id.trim().is_empty() && !schedule.cron.trim().is_empty() && !schedule.prompt.trim().is_empty();
            if !filled || self.schedules[..at].iter().any(|other| other.id == schedule.id) {
                return Err(Invalid::Schedule);
            }
        }
        for (at, channel) in self.channels.iter().enumerate() {
            let listed = !channel.allowed_senders.is_empty() && channel.allowed_senders.iter().all(|s| !s.trim().is_empty());
            let chat_ok = channel.report_chat.is_empty() || BotChannel::chat_looks_right(&channel.report_chat);
            if !listed || !chat_ok || self.channels[..at].iter().any(|other| other.kind == channel.kind) {
                return Err(Invalid::Channel);
            }
        }
        Ok(())
    }
}

/// One conversation with a bot, and the agent session that holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotThread {
    pub id: String,
    #[serde(default)]
    pub title: String,
    /// The agent's own session id, once it has one: what the conversation is continued by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub created_ms: u64,
    pub updated_ms: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct ThreadsFile {
    #[serde(default)]
    threads: Vec<BotThread>,
}

#[derive(Debug)]
pub enum StoreError {
    Invalid(Invalid),
    /// A bot with this handle is already there.
    Exists,
    Missing,
    Io(std::io::Error),
    /// `bot.json` or `threads.json` does not read as one.
    Format(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Invalid(why) => why.fmt(f),
            StoreError::Exists => f.write_str("a bot with this handle already exists"),
            StoreError::Missing => f.write_str("no bot with this handle"),
            StoreError::Io(err) => err.fmt(f),
            StoreError::Format(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<Invalid> for StoreError {
    fn from(why: Invalid) -> Self {
        StoreError::Invalid(why)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(err: std::io::Error) -> Self {
        StoreError::Io(err)
    }
}

/// The bots on disk. Every call reads or writes files: keep the result, and call it off `render`.
#[derive(Debug, Clone)]
pub struct BotStore {
    root: PathBuf,
}

impl BotStore {
    /// `<data dir>/bots`.
    pub fn open() -> Self {
        BotStore { root: fsutil::data_dir().join("bots") }
    }

    pub fn at(root: PathBuf) -> Self {
        BotStore { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A bot's folder. The handle is checked first, so the path never leaves the store.
    pub fn dir(&self, handle: &str) -> Result<PathBuf, StoreError> {
        check_handle(handle)?;
        Ok(self.root.join(handle))
    }

    pub fn soul_path(&self, handle: &str) -> Result<PathBuf, StoreError> {
        Ok(self.dir(handle)?.join(SOUL_FILE))
    }

    pub fn memory_path(&self, handle: &str) -> Result<PathBuf, StoreError> {
        Ok(self.dir(handle)?.join(MEMORY_FILE))
    }

    pub fn skills_dir(&self, handle: &str) -> Result<PathBuf, StoreError> {
        Ok(self.dir(handle)?.join(SKILLS_DIR))
    }

    /// Every bot that reads, by handle. A folder that is not a bot (no `bot.json`, one that does
    /// not read, one whose handle is not the folder's name) is left out and left alone.
    pub fn list(&self) -> Vec<Bot> {
        let Ok(entries) = std::fs::read_dir(&self.root) else { return Vec::new() };
        let mut bots: Vec<Bot> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name();
                self.load(name.to_str()?).ok()
            })
            .collect();
        bots.sort_by(|a, b| a.handle.cmp(&b.handle));
        bots
    }

    pub fn load(&self, handle: &str) -> Result<Bot, StoreError> {
        let path = self.dir(handle)?.join(BOT_FILE);
        let Some(bytes) = read_json(&path)? else { return Err(StoreError::Missing) };
        let bot: Bot = serde_json::from_slice(&bytes).map_err(|err| StoreError::Format(format!("{}: {err}", path.display())))?;
        if bot.handle != handle {
            return Err(StoreError::Format(format!("{}: the handle is not the folder's name", path.display())));
        }
        bot.validate()?;
        Ok(bot)
    }

    /// Makes the bot's folder with its files: `bot.json`, an empty `SOUL.md` and `MEMORY.md`, and
    /// `skills/`. The folder is the user's only (`0700`), each file `0600`.
    pub fn create(&self, bot: &Bot) -> Result<(), StoreError> {
        bot.validate()?;
        let dir = self.dir(&bot.handle)?;
        // A folder without `bot.json` (a creation that was cut short) is taken over.
        if dir.join(BOT_FILE).exists() {
            return Err(StoreError::Exists);
        }
        fsutil::create_private_dir(&dir.join(SKILLS_DIR))?;
        fsutil::create_private_dir(&dir)?;
        for file in [SOUL_FILE, MEMORY_FILE] {
            if !dir.join(file).exists() {
                fsutil::write_private(&dir.join(file), b"")?;
            }
        }
        self.write_bot(&dir, bot)
    }

    /// Where the bot's own picture is, when it has one.
    pub fn image_path(&self, bot: &Bot) -> Option<PathBuf> {
        Some(self.dir(&bot.handle).ok()?.join(bot.image_file()?))
    }

    /// Copies the picture at `source` into the bot's folder and returns the name to keep in
    /// `Bot::image`. Each picture gets a new name, so what draws it never shows the one before;
    /// the one before is removed.
    pub fn set_image(&self, handle: &str, source: &Path) -> Result<String, StoreError> {
        let refuse = |why: &str| StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, why.to_string()));
        let extension = source.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
        if !IMAGE_TYPES.contains(&extension.as_str()) {
            return Err(refuse("not a picture"));
        }
        if std::fs::metadata(source)?.len() > IMAGE_MAX {
            return Err(refuse("the picture is too large"));
        }
        let dir = self.dir(handle)?;
        if !dir.join(BOT_FILE).exists() {
            return Err(StoreError::Missing);
        }
        let bytes = std::fs::read(source)?;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let name = format!("avatar-{stamp}.{extension}");
        for entry in std::fs::read_dir(&dir)?.flatten() {
            if entry.file_name().to_string_lossy().starts_with("avatar-") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        fsutil::write_private(&dir.join(&name), &bytes)?;
        Ok(name)
    }

    pub fn part_path(&self, handle: &str, part: Part) -> Result<PathBuf, StoreError> {
        Ok(self.dir(handle)?.join(part.file()))
    }

    /// A part as the user wrote it: its first [`PART_LIMIT`] bytes.
    pub fn part(&self, handle: &str, part: Part) -> Result<String, StoreError> {
        Ok(read_text(&self.part_path(handle, part)?, PART_LIMIT))
    }

    /// Writes a part of a bot that exists, cut at [`PART_LIMIT`].
    pub fn write_part(&self, handle: &str, part: Part, text: &str) -> Result<(), StoreError> {
        let dir = self.dir(handle)?;
        if !dir.join(BOT_FILE).is_file() {
            return Err(StoreError::Missing);
        }
        let mut end = text.len().min(PART_LIMIT);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Ok(fsutil::write_private(&dir.join(part.file()), &text.as_bytes()[..end])?)
    }

    /// The bot's skills, by name.
    pub fn skills(&self, handle: &str) -> Vec<skills::Skill> {
        self.skills_dir(handle).map(|dir| skills::list(&dir)).unwrap_or_default()
    }

    fn skill_dir(&self, handle: &str, name: &str) -> Result<PathBuf, StoreError> {
        let dir = self.dir(handle)?;
        if !dir.join(BOT_FILE).is_file() {
            return Err(StoreError::Missing);
        }
        if !skills::check_name(name) {
            return Err(StoreError::Format(format!("{name}: not a skill's name")));
        }
        Ok(dir.join(SKILLS_DIR).join(name))
    }

    /// Gives the bot a skill that ships with Agentty.
    pub fn add_skill(&self, handle: &str, name: &str) -> Result<(), StoreError> {
        let Some(skill) = skills::bundled(name) else { return Err(StoreError::Missing) };
        let dir = self.skill_dir(handle, name)?;
        fsutil::create_private_dir(&dir)?;
        Ok(fsutil::write_private(&dir.join(skills::SKILL_FILE), skill.file().as_bytes())?)
    }

    /// Gives the bot a copy of a skill of the user's own. Returns the name it has in the bot.
    pub fn add_skill_from(&self, handle: &str, source: &Path) -> Result<String, StoreError> {
        let name = source.file_name().and_then(|name| name.to_str()).map(skills::slug).unwrap_or_default();
        let dir = self.skill_dir(handle, &name)?;
        if skills::read(source).is_none() {
            return Err(StoreError::Missing);
        }
        // A copy that fails halfway leaves no half a skill.
        let _ = std::fs::remove_dir_all(&dir);
        if let Err(err) = skills::copy(source, &dir) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(err.into());
        }
        Ok(name)
    }

    /// Takes a skill away from the bot.
    pub fn remove_skill(&self, handle: &str, name: &str) -> Result<(), StoreError> {
        let dir = self.skill_dir(handle, name)?;
        match std::fs::remove_dir_all(&dir) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
            _ => Ok(()),
        }
    }

    /// Writes the bot's instructions (`SOUL.md`), as a template does when it makes a bot.
    pub fn write_soul(&self, handle: &str, text: &str) -> Result<(), StoreError> {
        Ok(fsutil::write_private(&self.soul_path(handle)?, text.as_bytes())?)
    }

    /// Replaces the definition of a bot that exists. Its handle never changes: it is its folder.
    pub fn save(&self, bot: &Bot) -> Result<(), StoreError> {
        bot.validate()?;
        let dir = self.dir(&bot.handle)?;
        if !dir.join(BOT_FILE).is_file() {
            return Err(StoreError::Missing);
        }
        self.write_bot(&dir, bot)
    }

    fn write_bot(&self, dir: &Path, bot: &Bot) -> Result<(), StoreError> {
        let json = serde_json::to_vec_pretty(bot).map_err(|err| StoreError::Format(err.to_string()))?;
        Ok(fsutil::write_private_durable(&dir.join(BOT_FILE), &json)?)
    }

    /// Removes the bot with everything in its folder: instructions, memory, conversations.
    pub fn delete(&self, handle: &str) -> Result<(), StoreError> {
        let dir = self.dir(handle)?;
        match std::fs::symlink_metadata(&dir) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(StoreError::Missing),
            Err(err) => Err(err.into()),
            // A link put where the folder should be: the link goes, what it points at stays.
            Ok(meta) if meta.file_type().is_symlink() => Ok(std::fs::remove_file(&dir).or_else(|_| std::fs::remove_dir(&dir))?),
            Ok(_) => Ok(std::fs::remove_dir_all(&dir)?),
        }
    }

    /// The bot's instructions as the user wrote them.
    pub fn soul(&self, handle: &str) -> Result<String, StoreError> {
        Ok(read_text(&self.soul_path(handle)?, usize::MAX))
    }

    /// The first [`MEMORY_LIMIT`] bytes of the bot's memory (cut at a character).
    pub fn memory(&self, handle: &str) -> Result<String, StoreError> {
        Ok(read_text(&self.memory_path(handle)?, MEMORY_LIMIT))
    }

    /// The bot's conversations, the one touched last first.
    pub fn threads(&self, handle: &str) -> Result<Vec<BotThread>, StoreError> {
        let path = self.dir(handle)?.join(THREADS_FILE);
        let Some(bytes) = read_json(&path)? else { return Ok(Vec::new()) };
        let file: ThreadsFile = serde_json::from_slice(&bytes).map_err(|err| StoreError::Format(format!("{}: {err}", path.display())))?;
        Ok(newest_first(file.threads))
    }

    /// Writes the bot's conversations, keeping the [`THREADS_MAX`] touched last.
    pub fn save_threads(&self, handle: &str, threads: Vec<BotThread>) -> Result<(), StoreError> {
        let dir = self.dir(handle)?;
        if !dir.join(BOT_FILE).is_file() {
            return Err(StoreError::Missing);
        }
        let mut threads = newest_first(threads);
        threads.truncate(THREADS_MAX);
        let json = serde_json::to_vec_pretty(&ThreadsFile { threads }).map_err(|err| StoreError::Format(err.to_string()))?;
        Ok(fsutil::write_private(&dir.join(THREADS_FILE), &json)?)
    }
}

fn newest_first(mut threads: Vec<BotThread>) -> Vec<BotThread> {
    threads.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms).then_with(|| a.id.cmp(&b.id)));
    let mut seen = std::collections::HashSet::new();
    threads.retain(|thread| seen.insert(thread.id.clone()));
    threads
}

/// A JSON file's bytes: `None` when it is not there, an error when it is no file of ours.
fn read_json(path: &Path) -> Result<Option<Vec<u8>>, StoreError> {
    match std::fs::metadata(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
        Ok(meta) if !meta.is_file() || meta.len() > JSON_LIMIT => Err(StoreError::Format(format!("{}: not a bot file", path.display()))),
        Ok(_) => Ok(Some(std::fs::read(path)?)),
    }
}

/// Up to `limit` bytes of a text file, cut at a character; empty when it cannot be read.
fn read_text(path: &Path, limit: usize) -> String {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(path) else { return String::new() };
    let mut bytes = Vec::new();
    // Four more than asked for: enough to tell a character cut in half from the end of the file.
    if file.take(limit.saturating_add(4) as u64).read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> BotStore {
        let root = std::env::temp_dir().join(format!("agentty-bots-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        BotStore::at(root.join("bots"))
    }

    fn done(store: BotStore) {
        let _ = std::fs::remove_dir_all(store.root().parent().unwrap());
    }

    fn folder() -> PathBuf {
        std::env::temp_dir().join("project")
    }

    fn reviewer() -> Bot {
        Bot::new("reviewer", "Reviewer", "Reviews branches before they merge", BotAgent::Claude, folder())
    }

    fn thread(id: &str, updated_ms: u64) -> BotThread {
        BotThread { id: id.into(), title: String::new(), session: None, created_ms: 1, updated_ms }
    }

    #[test]
    fn a_bot_keeps_its_rules_its_context_and_its_skills_in_its_folder() {
        let store = store("parts");
        store.create(&reviewer()).unwrap();
        assert_eq!(store.part("reviewer", Part::Rules).unwrap(), "");
        store.write_part("reviewer", Part::Rules, "- Never push.\n").unwrap();
        store.write_part("reviewer", Part::Context, "The team merges on Fridays.").unwrap();
        store.write_part("reviewer", Part::Prompt, "Review what changed.").unwrap();
        assert_eq!(store.part("reviewer", Part::Rules).unwrap(), "- Never push.\n");
        assert_eq!(store.soul("reviewer").unwrap(), "Review what changed.");
        // A part is cut where it gets long, at a character.
        store.write_part("reviewer", Part::Context, &"가".repeat(PART_LIMIT)).unwrap();
        assert!(store.part("reviewer", Part::Context).unwrap().len() <= PART_LIMIT);
        assert!(matches!(store.write_part("nobody", Part::Rules, "x"), Err(StoreError::Missing)));

        store.add_skill("reviewer", "pr-review-checklist").unwrap();
        let skills = store.skills("reviewer");
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "pr-review-checklist");
        assert_eq!(skills[0].description, skills::bundled("pr-review-checklist").unwrap().description);
        assert!(matches!(store.add_skill("reviewer", "no-such-skill"), Err(StoreError::Missing)));
        // A name is a folder in the bot's own `skills/`, never a path out of it.
        assert!(store.remove_skill("reviewer", "../../reviewer").is_err());
        assert!(store.dir("reviewer").unwrap().join("bot.json").is_file());

        // One of the user's own is copied under a name a folder can have.
        let own = store.root().parent().unwrap().join("My Skill");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("SKILL.md"), "---\nname: My Skill\ndescription: Use for mine.\n---\nDo it.").unwrap();
        assert_eq!(store.add_skill_from("reviewer", &own).unwrap(), "my-skill");
        assert_eq!(
            store.skills("reviewer").iter().map(|skill| skill.name.as_str()).collect::<Vec<_>>(),
            ["my-skill", "pr-review-checklist"]
        );
        assert!(store.add_skill_from("reviewer", &store.root().join("reviewer")).is_err());
        store.remove_skill("reviewer", "my-skill").unwrap();
        store.remove_skill("reviewer", "my-skill").unwrap();
        assert_eq!(store.skills("reviewer").len(), 1);
    }

    #[test]
    fn a_bot_takes_a_picture_of_the_users_own_and_never_a_path() {
        let store = store("image");
        let mut bot = reviewer();
        store.create(&bot).unwrap();
        let source = store.root().parent().unwrap().join("Me.PNG");
        std::fs::write(&source, b"not really a picture, but named like one").unwrap();

        let name = store.set_image("reviewer", &source).unwrap();
        bot.image = name.clone();
        assert_eq!(store.image_path(&bot), Some(store.dir("reviewer").unwrap().join(&name)));
        assert!(store.image_path(&bot).unwrap().is_file());

        // The next one replaces it.
        std::thread::sleep(std::time::Duration::from_millis(3));
        let next = store.set_image("reviewer", &source).unwrap();
        assert!(next != name && !store.dir("reviewer").unwrap().join(&name).exists());

        // What is not a picture is refused, and a name written by hand is never followed.
        let text = store.root().parent().unwrap().join("notes.txt");
        std::fs::write(&text, b"x").unwrap();
        assert!(store.set_image("reviewer", &text).is_err());
        for name in ["../../secret.png", "/etc/passwd", "avatar-1.png/../x.png", "avatar-.png", "avatar-12.exe", "face.png"] {
            bot.image = name.into();
            assert_eq!(store.image_path(&bot), None, "{name}");
        }
        done(store);
    }

    #[test]
    fn a_bot_keeps_the_character_chosen_for_it_and_has_one_without() {
        let mut bot = reviewer();
        // None chosen: its handle picks one, the same every time, on that character's own colour.
        let own = bot.avatar();
        assert!(AVATARS.contains(&own));
        assert_eq!(own, reviewer().avatar());
        assert_eq!(bot.avatar_color(), AVATAR_COLORS[AVATARS.iter().position(|name| *name == own).unwrap()].1);
        assert!(!serde_json::to_string(&bot).unwrap().contains("avatar"));

        bot.avatar = "fox".into();
        bot.color = "pink".into();
        let read: Bot = serde_json::from_str(&serde_json::to_string(&bot).unwrap()).unwrap();
        assert_eq!((read.avatar(), read.avatar_color()), ("fox", 0xf4a6c0));

        // A name that is not a character falls back instead of drawing nothing.
        bot.avatar = "dragon".into();
        bot.color = "plaid".into();
        assert_eq!(bot.avatar(), own);
        assert_eq!(bot.avatar_color(), reviewer().avatar_color());
    }

    #[test]
    fn handles_are_lowercase_names_that_can_be_folders_everywhere() {
        for good in ["a", "reviewer", "release-check", "bot2", "0", &"a".repeat(HANDLE_MAX), "console", "com0", "com10"] {
            assert_eq!(check_handle(good), Ok(()), "{good}");
        }
        for bad in
            ["", "Reviewer", "re viewer", "-a", "a-", "a_b", "a.b", "../x", "a/b", "a\\b", "리뷰어", ".", &"a".repeat(HANDLE_MAX + 1)]
        {
            assert_eq!(check_handle(bad), Err(Invalid::Handle), "{bad}");
        }
        for device in ["con", "nul", "aux", "prn", "com1", "lpt9"] {
            assert_eq!(check_handle(device), Err(Invalid::HandleReserved), "{device}");
        }
    }

    #[test]
    fn a_name_gives_a_handle_when_it_has_latin_letters() {
        assert_eq!(handle_from_name("Release Check").as_deref(), Some("release-check"));
        assert_eq!(handle_from_name("  QA -- Bot #2! ").as_deref(), Some("qa-bot-2"));
        assert_eq!(handle_from_name("리뷰어 Bot").as_deref(), Some("bot"));
        assert_eq!(handle_from_name("리뷰어"), None);
        assert_eq!(handle_from_name("CON"), None);
        let long = handle_from_name(&"word ".repeat(20)).unwrap();
        assert!(long.len() <= HANDLE_MAX && !long.ends_with('-'), "{long}");
    }

    #[test]
    fn a_definition_is_checked_field_by_field() {
        assert_eq!(reviewer().validate(), Ok(()));
        let with = |change: fn(&mut Bot)| {
            let mut bot = reviewer();
            change(&mut bot);
            bot.validate()
        };
        assert_eq!(with(|b| b.handle = "Reviewer".into()), Err(Invalid::Handle));
        assert_eq!(with(|b| b.name = "  ".into()), Err(Invalid::Name));
        assert_eq!(with(|b| b.name = "x".repeat(NAME_MAX + 1)), Err(Invalid::Name));
        assert_eq!(with(|b| b.name = "리뷰어".into()), Ok(()));
        assert_eq!(with(|b| b.purpose = "two\nlines".into()), Err(Invalid::Purpose));
        assert_eq!(with(|b| b.purpose = "x".repeat(PURPOSE_MAX + 1)), Err(Invalid::Purpose));
        assert_eq!(with(|b| b.purpose.clear()), Ok(()));
        assert_eq!(with(|b| b.folder = PathBuf::from("relative/path")), Err(Invalid::Folder));
        assert_eq!(with(|b| b.model = Some("opus[1m]".into())), Ok(()));
        // A model is an argument of the agent's command line: never an option of it.
        assert_eq!(with(|b| b.model = Some("--some-option".into())), Err(Invalid::Model));
        assert_eq!(with(|b| b.model = Some("a b".into())), Err(Invalid::Model));
        assert_eq!(with(|b| b.model = Some(String::new())), Err(Invalid::Model));
    }

    #[test]
    fn schedules_need_their_fields_and_channels_their_senders() {
        let schedule = |id: &str| BotSchedule { id: id.into(), cron: "0 9 * * *".into(), prompt: "Check CI".into(), enabled: true };
        let mut bot = reviewer();
        bot.schedules = vec![schedule("morning"), schedule("evening")];
        assert_eq!(bot.validate(), Ok(()));
        bot.schedules.push(schedule("morning"));
        assert_eq!(bot.validate(), Err(Invalid::Schedule));
        bot.schedules = vec![BotSchedule { prompt: " ".into(), ..schedule("a") }];
        assert_eq!(bot.validate(), Err(Invalid::Schedule));

        let mut bot = reviewer();
        bot.channels = vec![BotChannel::new(ChannelKind::Telegram, vec!["1000".into()])];
        assert_eq!(bot.validate(), Ok(()));
        // A channel anyone could write to is never saved.
        bot.channels[0].allowed_senders.clear();
        assert_eq!(bot.validate(), Err(Invalid::Channel));
        bot.channels[0].allowed_senders = vec!["  ".into()];
        assert_eq!(bot.validate(), Err(Invalid::Channel));
        bot.channels[0].allowed_senders = vec!["1000".into()];
        bot.channels[0].report_chat = "D1/../x y".into();
        assert_eq!(bot.validate(), Err(Invalid::Channel));
        bot.channels[0].report_chat = "-1001234".into();
        assert_eq!(bot.validate(), Ok(()));
        bot.channels = vec![BotChannel::new(ChannelKind::Slack, vec!["U1".into()]), BotChannel::new(ChannelKind::Slack, vec!["U2".into()])];
        assert_eq!(bot.validate(), Err(Invalid::Channel));
    }

    #[test]
    fn a_bot_is_created_read_back_changed_and_deleted() {
        let store = store("round-trip");
        assert!(store.list().is_empty());
        let mut bot = reviewer();
        store.create(&bot).unwrap();
        let dir = store.dir("reviewer").unwrap();
        assert!(dir.join("SOUL.md").is_file() && dir.join("MEMORY.md").is_file() && dir.join("skills").is_dir());
        assert_eq!(store.load("reviewer").unwrap(), bot);
        assert!(matches!(store.create(&bot), Err(StoreError::Exists)));

        bot.model = Some("opus".into());
        bot.permission = BotPermission::ReadOnly;
        bot.workspace = BotWorkspace::Worktree;
        bot.enabled = false;
        bot.schedules = vec![BotSchedule { id: "morning".into(), cron: "0 9 * * 1-5".into(), prompt: "Check CI".into(), enabled: false }];
        bot.channels = vec![BotChannel::new(ChannelKind::Telegram, vec!["1000".into()])];
        store.save(&bot).unwrap();
        assert_eq!(store.load("reviewer").unwrap(), bot);

        store.create(&Bot::new("builder", "Builder", "", BotAgent::Codex, folder())).unwrap();
        assert_eq!(store.list().iter().map(|b| b.handle.as_str()).collect::<Vec<_>>(), ["builder", "reviewer"]);

        store.delete("reviewer").unwrap();
        assert!(!dir.exists());
        assert!(matches!(store.load("reviewer"), Err(StoreError::Missing)));
        assert!(matches!(store.delete("reviewer"), Err(StoreError::Missing)));
        assert!(matches!(store.save(&bot), Err(StoreError::Missing)));
        assert_eq!(store.list().len(), 1);
        done(store);
    }

    #[test]
    fn the_file_format_is_the_documented_one() {
        let store = store("format");
        store.create(&reviewer()).unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(store.dir("reviewer").unwrap().join("bot.json")).unwrap()).unwrap();
        assert_eq!(json["handle"], "reviewer");
        assert_eq!(json["agent"], "claude");
        assert_eq!(json["workspace"], "folder");
        assert_eq!(json["permission"], "ask");
        assert_eq!(json["enabled"], true);
        assert!(json.get("model").is_none());
        // A file written by hand with only what has no default still reads, as an asking bot.
        let dir = store.root().join("minimal");
        std::fs::create_dir_all(&dir).unwrap();
        let minimal = serde_json::json!({ "handle": "minimal", "name": "Minimal", "agent": "codex", "folder": folder() });
        std::fs::write(dir.join("bot.json"), minimal.to_string()).unwrap();
        let bot = store.load("minimal").unwrap();
        assert_eq!((bot.permission, bot.workspace, bot.enabled), (BotPermission::Ask, BotWorkspace::Folder, true));
        // A permission this version does not know is not read as a wider one: the bot is left out.
        let wide =
            serde_json::json!({ "handle": "minimal", "name": "Minimal", "agent": "codex", "folder": folder(), "permission": "anything" });
        std::fs::write(dir.join("bot.json"), wide.to_string()).unwrap();
        assert!(matches!(store.load("minimal"), Err(StoreError::Format(_))));
        done(store);
    }

    #[test]
    fn folders_that_are_not_bots_are_left_out_and_nothing_leaves_the_store() {
        let store = store("strays");
        store.create(&reviewer()).unwrap();
        std::fs::create_dir_all(store.root().join("empty")).unwrap();
        std::fs::create_dir_all(store.root().join("Not A Handle")).unwrap();
        std::fs::create_dir_all(store.root().join("broken")).unwrap();
        std::fs::write(store.root().join("broken").join("bot.json"), "{ not json").unwrap();
        // A folder copied under another name still says the old handle.
        std::fs::create_dir_all(store.root().join("copy")).unwrap();
        std::fs::copy(store.dir("reviewer").unwrap().join("bot.json"), store.root().join("copy").join("bot.json")).unwrap();
        std::fs::write(store.root().join("notes.txt"), "x").unwrap();
        assert_eq!(store.list(), vec![reviewer()]);
        assert!(matches!(store.load("broken"), Err(StoreError::Format(_))));
        assert!(matches!(store.load("copy"), Err(StoreError::Format(_))));

        let outside = store.root().parent().unwrap().join("keep");
        std::fs::create_dir_all(&outside).unwrap();
        for handle in ["../keep", "..", "", "a/../../keep"] {
            assert!(matches!(store.delete(handle), Err(StoreError::Invalid(Invalid::Handle))), "{handle}");
            assert!(matches!(store.memory(handle), Err(StoreError::Invalid(Invalid::Handle))), "{handle}");
        }
        assert!(outside.is_dir());
        let mut bot = reviewer();
        bot.handle = "../keep".into();
        assert!(matches!(store.create(&bot), Err(StoreError::Invalid(Invalid::Handle))));
        // A creation that was cut short left a folder without `bot.json`: the handle is still free.
        store.create(&Bot::new("empty", "Empty", "", BotAgent::Claude, folder())).unwrap();
        done(store);
    }

    #[cfg(unix)]
    #[test]
    fn a_bots_files_are_the_users_only() {
        use std::os::unix::fs::PermissionsExt;
        let store = store("private");
        store.create(&reviewer()).unwrap();
        store.save(&reviewer()).unwrap();
        store.save_threads("reviewer", vec![thread("t1", 5)]).unwrap();
        let dir = store.dir("reviewer").unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        for file in ["bot.json", "SOUL.md", "MEMORY.md", "threads.json"] {
            assert_eq!(mode(&dir.join(file)), 0o600, "{file}");
        }
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("skills")), 0o700);
        done(store);
    }

    #[cfg(unix)]
    #[test]
    fn deleting_a_link_leaves_what_it_points_at() {
        let store = store("link");
        store.create(&reviewer()).unwrap();
        let target = store.root().parent().unwrap().join("elsewhere");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("kept.txt"), "x").unwrap();
        std::os::unix::fs::symlink(&target, store.root().join("linked")).unwrap();
        store.delete("linked").unwrap();
        assert!(target.join("kept.txt").is_file());
        assert!(!store.root().join("linked").exists());
        done(store);
    }

    #[test]
    fn memory_is_read_up_to_its_limit_at_a_character() {
        let store = store("memory");
        store.create(&reviewer()).unwrap();
        assert_eq!(store.memory("reviewer").unwrap(), "");
        assert_eq!(store.soul("reviewer").unwrap(), "");
        std::fs::write(store.soul_path("reviewer").unwrap(), "Be brief.").unwrap();
        assert_eq!(store.soul("reviewer").unwrap(), "Be brief.");
        std::fs::write(store.memory_path("reviewer").unwrap(), "- prefers small PRs\n").unwrap();
        assert_eq!(store.memory("reviewer").unwrap(), "- prefers small PRs\n");
        // Three bytes a character: the limit falls inside one, which is then left out whole.
        let long = "가".repeat(MEMORY_LIMIT);
        std::fs::write(store.memory_path("reviewer").unwrap(), &long).unwrap();
        let read = store.memory("reviewer").unwrap();
        assert_eq!(read.len(), MEMORY_LIMIT - MEMORY_LIMIT % 3);
        assert!(read.chars().all(|c| c == '가'));
        done(store);
    }

    #[test]
    fn threads_are_kept_newest_first_up_to_their_cap() {
        let store = store("threads");
        assert!(matches!(store.save_threads("reviewer", Vec::new()), Err(StoreError::Missing)));
        store.create(&reviewer()).unwrap();
        assert!(store.threads("reviewer").unwrap().is_empty());
        let mut first = thread("t1", 10);
        first.title = "Review the branch".into();
        first.session = Some("session-example-1".into());
        store.save_threads("reviewer", vec![first.clone(), thread("t2", 30), thread("t3", 20)]).unwrap();
        let read = store.threads("reviewer").unwrap();
        assert_eq!(read.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t2", "t3", "t1"]);
        assert_eq!(read[2], first);
        // The same conversation twice: the copy touched last is the one kept.
        store.save_threads("reviewer", vec![thread("t1", 10), thread("t1", 40), thread("t2", 30)]).unwrap();
        let read = store.threads("reviewer").unwrap();
        assert_eq!(read.iter().map(|t| (t.id.as_str(), t.updated_ms)).collect::<Vec<_>>(), [("t1", 40), ("t2", 30)]);

        let many: Vec<BotThread> = (0..THREADS_MAX as u64 + 5).map(|n| thread(&format!("t{n}"), n)).collect();
        store.save_threads("reviewer", many).unwrap();
        let read = store.threads("reviewer").unwrap();
        assert_eq!(read.len(), THREADS_MAX);
        assert_eq!(read.first().unwrap().updated_ms, THREADS_MAX as u64 + 4);
        assert_eq!(read.last().unwrap().updated_ms, 5);
        done(store);
    }
}

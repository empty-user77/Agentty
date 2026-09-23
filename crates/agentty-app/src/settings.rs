//! User settings (`~/.agentty/settings.json`), held in a GPUI global so every view reads the
//! same values and re-renders when they change.

use crate::theme::{load_themes, TerminalTheme, DEFAULT_THEME, LEGACY_DEFAULT_THEME};
use gpui::{App, BorrowAppContext, Global};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// Follow the macOS language (English when it isn't supported).
    System,
    En,
    Ko,
    Ja,
    Zh,
}

impl Language {
    pub const ALL: [Language; 5] = [Language::System, Language::En, Language::Ko, Language::Ja, Language::Zh];

    /// The concrete language to show: `System` resolves to the macOS setting (read once).
    pub fn resolved(self) -> Language {
        static SYSTEM: std::sync::OnceLock<Language> = std::sync::OnceLock::new();
        match self {
            Language::System => *SYSTEM.get_or_init(Language::detect),
            other => other,
        }
    }

    /// `en`, `ko`, `ja` or `zh` for the language shown.
    pub fn code(self) -> &'static str {
        match self.resolved() {
            Language::System | Language::En => "en",
            Language::Ko => "ko",
            Language::Ja => "ja",
            Language::Zh => "zh",
        }
    }

    pub fn native_name(self) -> &'static str {
        match self {
            Language::System => "System",
            Language::En => "English",
            Language::Ko => "한국어",
            Language::Ja => "日本語",
            Language::Zh => "中文",
        }
    }

    /// First preferred system language (macOS languages, the Windows locale, `LANG` on Linux),
    /// then English.
    fn detect() -> Self {
        Self::from_code(&crate::platform::preferred_language())
    }

    /// `ko-KR` → Korean; unsupported languages → English.
    pub fn from_code(code: &str) -> Self {
        match code.to_lowercase().get(..2) {
            Some("ko") => Language::Ko,
            Some("ja") => Language::Ja,
            Some("zh") => Language::Zh,
            _ => Language::En,
        }
    }
}

/// Where links (⌘-click in terminals) open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkOpener {
    External,
    #[default]
    InApp,
}

/// Where the file editor's "Open in editor" button sends a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExternalEditor {
    /// Visual Studio Code when it is installed, else the system's text editor.
    #[default]
    Auto,
    VsCode,
    Cursor,
    System,
}

impl ExternalEditor {
    pub const ALL: [ExternalEditor; 4] = [Self::Auto, Self::VsCode, Self::Cursor, Self::System];

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Auto => "editor.external_auto",
            Self::VsCode => "editor.external_vscode",
            Self::Cursor => "editor.external_cursor",
            Self::System => "editor.external_system",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorShapeSetting {
    Block,
    Beam,
    Underline,
}

/// A reserved word: when `keyword` is passed as a whole argument to `command`, it is replaced by
/// `expansion` (e.g. `zzzz` → `--dangerously-skip-permissions` for `claude`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandAlias {
    pub keyword: String,
    pub expansion: String,
    pub command: String,
}

pub const SETTINGS_VERSION: u32 = 6;

pub const BUNDLED_FONT: &str = "JetBrains Mono";
/// The coding font Korean developers reach for: its Hangul is exactly two cells wide. Not bundled
/// (it is several times the size of Agentty itself); recommended when it is missing.
pub const KOREAN_FONT: &str = "D2Coding";
/// Where D2Coding is published (SIL Open Font License), for the recommendation to point at.
pub const KOREAN_FONT_URL: &str = "https://github.com/naver/d2-coding-font/releases/latest";

/// Whether D2Coding is installed. Asks for that one family rather than listing every font on the
/// computer, and only once a run: a font installed while Agentty runs counts from the next start.
pub fn korean_font_installed(cx: &App) -> bool {
    static INSTALLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *INSTALLED.get_or_init(|| {
        let text = cx.text_system();
        // A family that is missing resolves to a fallback, which answers to its own name.
        text.get_font_for_id(text.resolve_font(&gpui::font(KOREAN_FONT))).is_some_and(|f| f.family.as_ref() == KOREAN_FONT)
    })
}

/// The font the terminals use when the user has not picked one: on macOS, D2Coding for someone
/// reading Agentty in Korean who has it installed; the bundled font otherwise, and always on Windows
/// and Linux, where D2Coding lacks glyphs PowerShell and other shells draw.
pub fn default_terminal_font(cx: &App) -> &'static str {
    let language = settings(cx).language.resolved();
    let mac = cfg!(target_os = "macos");
    // Asked only where it can matter: finding out means loading a font family.
    default_font_for(mac, language, mac && language == Language::Ko && korean_font_installed(cx))
}

fn default_font_for(mac: bool, language: Language, korean_font_installed: bool) -> &'static str {
    if mac && language == Language::Ko && korean_font_installed {
        KOREAN_FONT
    } else {
        BUNDLED_FONT
    }
}

/// The font the terminals, diffs and editor draw with: the one the user picked, else the default.
pub fn terminal_font(cx: &App) -> String {
    let picked = &settings(cx).font_family;
    if picked.is_empty() {
        default_terminal_font(cx).to_string()
    } else {
        picked.clone()
    }
}
/// Nerd Font-patched JetBrains Mono, used for Powerline / Nerd Font glyphs. (The symbols-only font
/// cannot be used: GPUI skips fonts without an `m` glyph.)
pub const SYMBOLS_FONT: &str = "JetBrainsMono Nerd Font Mono";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Format version, for migrations. Missing in old files (→ 0).
    #[serde(default)]
    pub settings_version: u32,
    pub language: Language,
    pub theme: String,
    /// The font the user picked; empty until they pick one (see `terminal_font`).
    pub font_family: String,
    pub font_size: f32,
    pub line_height: f32,
    pub cursor_shape: CursorShapeSetting,
    pub cursor_blink: bool,
    pub padding: f32,
    /// Extra width added to every cell, in points. A terminal is a grid, so tracking is the cell,
    /// not the glyph: widening it spaces the text out without breaking the columns.
    #[serde(default)]
    pub letter_spacing: f32,
    /// Draws ordinary text at bold weight (what the terminal marks bold then goes heavier still).
    #[serde(default)]
    pub bold_text: bool,
    /// Colours of the chosen theme the user replaced, as `0xRRGGBB`. Unset means the theme's own.
    #[serde(default)]
    pub color_background: Option<u32>,
    #[serde(default)]
    pub color_foreground: Option<u32>,
    #[serde(default)]
    pub color_cursor: Option<u32>,
    #[serde(default)]
    pub color_selection: Option<u32>,
    pub option_as_meta: bool,
    pub scrollback: usize,
    pub sidebar_width: f32,
    /// Width of the files panel docked at the right edge.
    pub files_panel_width: f32,
    /// Width of the plugin panel docked right of the terminals.
    pub plugin_panel_width: f32,
    /// How each plugin's panel opens (`push`, `overlay`, `window`, `full`), when the user picked
    /// something other than what the plugin asks for.
    #[serde(default)]
    pub plugin_panel_modes: std::collections::BTreeMap<String, String>,
    /// Width of the Docker panel docked right of the terminals.
    pub docker_panel_width: f32,
    /// Width of the database panel docked right of the terminals.
    #[serde(default = "default_db_panel_width")]
    pub db_panel_width: f32,
    /// Height of its working-tree list once the user dragged it (0: as tall as its rows, up to a few).
    pub files_panel_trees_height: f32,
    /// Ask for a starting folder whenever a new workspace is opened.
    pub ask_directory: bool,
    /// Also ask for new tabs (otherwise they open in the current tab's folder).
    pub ask_directory_for_tabs: bool,
    pub recent_dirs: Vec<PathBuf>,
    pub aliases: Vec<CommandAlias>,
    /// macOS notifications when an agent finishes or needs input while Agentty is in the background.
    pub system_notifications: bool,
    /// Local sessions pinned to the top (`claude:<id>`, `codex:<id>`).
    pub favorite_sessions: Vec<String>,
    /// Notify even while Agentty is the focused app.
    pub notify_when_focused: bool,
    /// An agent asking for an answer (permission, question) always notifies, unless its pane is the
    /// one in front.
    pub notify_answer_requests: bool,
    /// Messages to Slack / Discord / Telegram (their secrets live in the credential store).
    pub chat_notify: ChatNotify,
    /// Menu bar icon; closing the window keeps Agentty running there.
    pub menu_bar: bool,
    pub link_opener: LinkOpener,
    /// App the file editor's "Open in editor" button uses.
    pub external_editor: ExternalEditor,
    pub browser: BrowserSettings,
    /// Offer earlier AI sessions when a terminal enters their folder.
    pub resume_bar: bool,
    /// Status bar (model, context, branch) above AI CLI panes.
    pub agent_bar: bool,
    /// Whether that bar (and the header of a split pane) sits above the terminal or under it.
    #[serde(deserialize_with = "crate::hud::lenient_position")]
    pub agent_bar_position: crate::hud::HudPosition,
    /// Items of that status bar, in order (`hud::normalized` fills in what a file is missing).
    #[serde(deserialize_with = "crate::hud::lenient")]
    pub hud: Vec<crate::hud::HudEntry>,
    /// Offer "Build my idea": the start page banner, the + menu and the command palette.
    pub idea_mode: bool,
    /// Ask before closing a pane, tab or workspace that was used.
    pub confirm_close: bool,
    /// Ask, when closing the last pane working in a linked working tree, whether to remove that
    /// tree and its branch. Off means never ask and never remove — the tree simply stays.
    pub ask_remove_trees: bool,
    /// A new agent session in a project another agent already works in gets its own git worktree.
    pub auto_worktree: bool,
    /// Agents may ask (`agentty tasks`) to start parallel tasks in split panes; the user still
    /// confirms every request.
    pub agent_tasks: bool,
    /// Agents started by Agentty get a short guide to what Agentty offers them (and Claude Code its
    /// Agentty skills).
    pub agent_guide: bool,
    /// Closing a pane stops the local servers (dev servers) started in it.
    pub stop_servers_on_close: bool,
    /// Claude Code advisor for new Claude tabs.
    pub advisor: AdvisorChoice,
    /// Offer to start work through a project's agent harness when a terminal enters it.
    pub harness_detect: bool,
    /// Extra harness patterns (relative to the project), on top of the built-in ones.
    pub harness_patterns: Vec<String>,
    /// Send the harness prompt right away (otherwise it is typed in for review).
    pub harness_submit: bool,
    /// Agent that starts harness work.
    pub harness_agent: HarnessAgent,
    /// Consent to anonymous usage statistics (Google Analytics: which features were used, app and
    /// OS version, a random installation id — never personal data, paths, commands, prompts or
    /// output). `DO_NOT_TRACK=1` turns it off regardless, and builds without analytics credentials
    /// send nothing either way.
    pub analytics: bool,
    /// Thin workspace rows: title, colour and state only, for a sidebar with many workspaces.
    #[serde(default)]
    pub compact_workspaces: bool,
    /// A workspace whose agent just finished jumps to the top of its group (or the ungrouped
    /// list). Off by default: a fixed order is what most people expect from a sidebar they
    /// arranged themselves, and the card already shows the result without moving.
    #[serde(default)]
    pub sort_finished_to_top: bool,
    /// The search box above the workspace list. On by default; off gives the list the row back
    /// (and stops filtering, whatever was typed before).
    #[serde(default = "yes")]
    pub workspace_search_bar: bool,
    /// Keep the machine awake (display and system sleep) while Agentty runs. Off by default:
    /// it costs energy, so the user turns it on for a long unattended run.
    #[serde(default)]
    pub prevent_sleep: bool,
    /// How long "Prevent sleep" stays on once turned on: 0 = always (the default), else hours.
    #[serde(default)]
    pub prevent_sleep_hours: u32,
    /// When (Unix seconds) a timed "Prevent sleep" turns itself off; 0 while it is always on or off.
    #[serde(default)]
    pub prevent_sleep_until: u64,
    /// The first-launch system check ran (Windows / Linux).
    pub setup_check_shown: bool,
    /// The first-run onboarding was finished or skipped.
    pub onboarding_done: bool,
    /// "Later" in the update popup: the version it was for and when (Unix seconds) the popup may
    /// come back. Until then the update only shows in the status bar.
    #[serde(default)]
    pub update_later_version: String,
    #[serde(default)]
    pub update_later_until: u64,
}

/// Which agent starts work through a harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HarnessAgent {
    /// The one the harness is built for (Claude Code when both).
    #[default]
    Auto,
    Claude,
    Codex,
}

impl HarnessAgent {
    pub fn resolve(self, harness: &agentty_bridge::harness::Harness) -> agentty_bridge::model::Agent {
        match self {
            Self::Auto => harness.preferred_agent(),
            Self::Claude => agentty_bridge::model::Agent::Claude,
            Self::Codex => agentty_bridge::model::Agent::Codex,
        }
    }
}

/// Claude Code's advisor tool (a stronger model Claude consults at key moments) for a Claude tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AdvisorChoice {
    /// No flag: whatever Claude Code itself is configured with (`/advisor`, `advisorModel`).
    #[default]
    Inherit,
    Off,
    Opus,
    Fable,
}

impl AdvisorChoice {
    pub const ALL: [AdvisorChoice; 4] = [Self::Inherit, Self::Off, Self::Opus, Self::Fable];

    /// Alias for `claude --advisor`.
    pub fn model(self) -> Option<&'static str> {
        match self {
            Self::Opus => Some("opus"),
            Self::Fable => Some("fable"),
            Self::Inherit | Self::Off => None,
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Inherit => "advisor.inherit",
            Self::Off => "advisor.off",
            Self::Opus => "advisor.opus",
            Self::Fable => "advisor.fable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchEngine {
    #[default]
    Google,
    DuckDuckGo,
    Bing,
}

impl SearchEngine {
    pub const ALL: [SearchEngine; 3] = [Self::Google, Self::DuckDuckGo, Self::Bing];

    pub fn name(self) -> &'static str {
        match self {
            Self::Google => "Google",
            Self::DuckDuckGo => "DuckDuckGo",
            Self::Bing => "Bing",
        }
    }

    /// Search URL prefix; the encoded query is appended.
    pub fn query_prefix(self) -> &'static str {
        match self {
            Self::Google => "https://www.google.com/search?q=",
            Self::DuckDuckGo => "https://duckduckgo.com/?q=",
            Self::Bing => "https://www.bing.com/search?q=",
        }
    }
}

/// Unknown or removed engines in older settings fall back to the default instead of failing the file.
fn lenient_search_engine<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<SearchEngine, D::Error> {
    let value = String::deserialize(deserializer)?;
    Ok(match value.as_str() {
        "duckDuckGo" => SearchEngine::DuckDuckGo,
        "bing" => SearchEngine::Bing,
        _ => SearchEngine::Google,
    })
}

/// Which chat services get a message, and what. The webhook URLs and the bot token are in the
/// credential store (`agentty_bridge::notify`), never here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChatNotify {
    pub slack: bool,
    pub discord: bool,
    pub telegram: bool,
    /// Telegram chat the bot writes to (a number, or `@channel`).
    pub telegram_chat: String,
    /// Slack with a bot token and a channel instead of a webhook URL.
    #[serde(default)]
    pub slack_bot: bool,
    /// Slack channel the bot writes to (`#general`, `general`, or a channel id).
    #[serde(default)]
    pub slack_channel: String,
    /// Discord with a bot token and a channel id instead of a webhook URL.
    #[serde(default)]
    pub discord_bot: bool,
    /// Discord channel id the bot writes to (the API posts to an id, not a name).
    #[serde(default)]
    pub discord_channel: String,
    /// Also when an agent finishes (by default only when one needs an answer).
    pub on_finish: bool,
    /// Include what the agent asks (the command, the question). Off: only who and where.
    pub details: bool,
}

impl ChatNotify {
    /// How this channel sends: what the user picked for Slack and Discord, and always a bot for
    /// Telegram, which has no webhook form.
    pub fn transport(&self, channel: agentty_bridge::notify::Channel) -> agentty_bridge::notify::Transport {
        use agentty_bridge::notify::{Channel, Transport};
        let bot = match channel {
            Channel::Slack => self.slack_bot,
            Channel::Discord => self.discord_bot,
            Channel::Telegram => true,
        };
        if bot {
            Transport::Bot
        } else {
            Transport::Webhook
        }
    }

    pub fn set_transport(&mut self, channel: agentty_bridge::notify::Channel, transport: agentty_bridge::notify::Transport) {
        use agentty_bridge::notify::{Channel, Transport};
        let bot = transport == Transport::Bot;
        match channel {
            Channel::Slack => self.slack_bot = bot,
            Channel::Discord => self.discord_bot = bot,
            Channel::Telegram => {}
        }
    }

    /// The channel or chat this one writes to, for the transport in use. Empty when none is needed.
    pub fn target(&self, channel: agentty_bridge::notify::Channel) -> String {
        use agentty_bridge::notify::{Channel, Transport};
        if self.transport(channel) == Transport::Webhook {
            return String::new();
        }
        match channel {
            Channel::Slack => self.slack_channel.clone(),
            Channel::Discord => self.discord_channel.clone(),
            Channel::Telegram => self.telegram_chat.clone(),
        }
    }

    pub fn set_target(&mut self, channel: agentty_bridge::notify::Channel, value: String) {
        use agentty_bridge::notify::Channel;
        match channel {
            Channel::Slack => self.slack_channel = value,
            Channel::Discord => self.discord_channel = value,
            Channel::Telegram => self.telegram_chat = value,
        }
    }

    pub fn enabled(&self, channel: agentty_bridge::notify::Channel) -> bool {
        use agentty_bridge::notify::Channel;
        match channel {
            Channel::Slack => self.slack,
            Channel::Discord => self.discord,
            Channel::Telegram => self.telegram,
        }
    }

    pub fn set_enabled(&mut self, channel: agentty_bridge::notify::Channel, on: bool) {
        use agentty_bridge::notify::Channel;
        match channel {
            Channel::Slack => self.slack = on,
            Channel::Discord => self.discord = on,
            Channel::Telegram => self.telegram = on,
        }
    }
}

/// In-app browser preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BrowserSettings {
    /// Page the browser opens with.
    pub home: String,
    /// Panel width in points.
    pub width: f32,
    /// Height of the network panel under the page, in points.
    pub network_height: f32,
    #[serde(deserialize_with = "lenient_search_engine")]
    pub search_engine: SearchEngine,
    pub javascript: bool,
    /// Let pages open windows (pop-ups) without a click.
    pub popups: bool,
    /// Page zoom (1.0 = 100%).
    pub zoom: f32,
    /// Ask sites for their mobile layout.
    pub mobile: bool,
    /// Don't keep cookies, cache or history on disk.
    pub private_mode: bool,
    /// Allow Safari's Web Inspector (Develop menu) to attach.
    pub inspectable: bool,
    /// Give Claude Code / Codex started in Agentty tools to control this browser (MCP).
    pub agent_tools: bool,
    /// A local server started in a tab opens here by itself (only while links open in-app).
    pub auto_open_servers: bool,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self {
            home: "https://www.agentty.run".into(),
            width: 560.,
            network_height: 260.,
            search_engine: SearchEngine::Google,
            javascript: true,
            popups: false,
            zoom: 1.0,
            mobile: false,
            private_mode: false,
            inspectable: false,
            agent_tools: true,
            auto_open_servers: true,
        }
    }
}

/// `#[serde(default = "yes")]`: a flag that is on unless the file says otherwise.
fn yes() -> bool {
    true
}

fn default_db_panel_width() -> f32 {
    crate::workbench::side_panels::DEFAULT_DATABASE_WIDTH
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            settings_version: SETTINGS_VERSION,
            language: Language::System,
            theme: DEFAULT_THEME.into(),
            font_family: String::new(),
            font_size: 13.0,
            line_height: 1.0,
            cursor_shape: CursorShapeSetting::Beam,
            cursor_blink: true,
            letter_spacing: 0.0,
            bold_text: false,
            color_background: None,
            color_foreground: None,
            color_cursor: None,
            color_selection: None,
            padding: 8.0,
            option_as_meta: true,
            scrollback: 10_000,
            sidebar_width: 300.0,
            files_panel_width: 300.0,
            plugin_panel_width: crate::workbench::side_panels::DEFAULT_PLUGIN_WIDTH,
            plugin_panel_modes: std::collections::BTreeMap::new(),
            docker_panel_width: crate::workbench::side_panels::DEFAULT_DOCKER_WIDTH,
            db_panel_width: crate::workbench::side_panels::DEFAULT_DATABASE_WIDTH,
            files_panel_trees_height: 0.0,
            ask_directory: true,
            ask_directory_for_tabs: false,
            recent_dirs: Vec::new(),
            aliases: Vec::new(),
            system_notifications: true,
            notify_when_focused: false,
            notify_answer_requests: true,
            chat_notify: ChatNotify::default(),
            menu_bar: true,
            link_opener: LinkOpener::InApp,
            external_editor: ExternalEditor::Auto,
            browser: BrowserSettings::default(),
            favorite_sessions: Vec::new(),
            resume_bar: true,
            agent_bar: true,
            agent_bar_position: crate::hud::HudPosition::default(),
            hud: crate::hud::default_layout(),
            idea_mode: true,
            confirm_close: true,
            ask_remove_trees: true,
            auto_worktree: true,
            agent_tasks: true,
            agent_guide: true,
            stop_servers_on_close: true,
            advisor: AdvisorChoice::Inherit,
            harness_detect: true,
            harness_patterns: Vec::new(),
            harness_submit: true,
            harness_agent: HarnessAgent::Auto,
            analytics: true,
            compact_workspaces: false,
            sort_finished_to_top: false,
            workspace_search_bar: true,
            prevent_sleep: false,
            prevent_sleep_hours: 0,
            prevent_sleep_until: 0,
            setup_check_shown: false,
            onboarding_done: false,
            update_later_version: String::new(),
            update_later_until: 0,
        }
    }
}

impl Settings {
    /// Longest "Prevent sleep" time that can be set, in hours.
    pub const PREVENT_SLEEP_MAX_HOURS: u32 = 72;

    /// Turns "Prevent sleep" on or off; a timed one counts from now.
    pub fn set_prevent_sleep(&mut self, on: bool, now: u64) {
        self.prevent_sleep = on;
        self.prevent_sleep_until = if on && self.prevent_sleep_hours > 0 { now + u64::from(self.prevent_sleep_hours) * 3600 } else { 0 };
    }

    /// "Always" (0) or a number of hours; while it is on, the new time counts from now.
    pub fn set_prevent_sleep_hours(&mut self, hours: u32, now: u64) {
        self.prevent_sleep_hours = hours.min(Self::PREVENT_SLEEP_MAX_HOURS);
        let on = self.prevent_sleep;
        self.set_prevent_sleep(on, now);
    }

    /// Seconds a timed "Prevent sleep" has left; `None` when it is off or always on.
    pub fn prevent_sleep_left(&self, now: u64) -> Option<u64> {
        (self.prevent_sleep && self.prevent_sleep_until > 0).then(|| self.prevent_sleep_until.saturating_sub(now))
    }

    pub fn path() -> PathBuf {
        agentty_bridge::fsutil::data_dir().join("settings.json")
    }

    fn load() -> Self {
        match std::fs::read(Self::path()) {
            Ok(bytes) => serde_json::from_slice::<Settings>(&bytes).map(Settings::migrate).unwrap_or_default(),
            Err(_) => Settings::default(),
        }
    }

    fn migrate(mut self) -> Self {
        if self.settings_version < 2 {
            // v2: Ghostty-like defaults. Line height became a multiple of the font's natural height.
            if self.theme == LEGACY_DEFAULT_THEME {
                self.theme = DEFAULT_THEME.into();
            }
            self.line_height = 1.0;
        }
        if self.settings_version < 3 {
            // v3: the cursor is a bar. A settings file always carries every field, so a block here
            // is the old default rather than a choice — anyone who picked a shape picked one of the
            // other two, and keeps it.
            if self.cursor_shape == CursorShapeSetting::Block {
                self.cursor_shape = CursorShapeSetting::Beam;
            }
        }
        if self.settings_version < 4 {
            // v4: the font is empty until the user picks one, so the default can follow the
            // language (D2Coding in Korean, when installed). The bundled font was the one default
            // there was, so still holding it means it was never changed.
            if self.font_family == BUNDLED_FONT {
                self.font_family.clear();
            }
        }
        if self.settings_version < 5 {
            // v5: the status bar shows local server ports by default. They were hidden until turned
            // on, so a saved "hidden" was almost always the old default rather than a choice.
            for entry in self.hud.iter_mut().filter(|e| e.item == crate::hud::HudItem::Ports) {
                entry.visible = true;
            }
        }
        if self.settings_version < 6 {
            // v6: Option is Meta by default on the Mac, so the terminal's and the agents' Option
            // shortcuts (word moves, Claude Code's model and thinking keys) work. It was off
            // unless turned on, so "off" was the old default rather than a choice.
            self.option_as_meta = true;
        }
        self.settings_version = SETTINGS_VERSION;
        self
    }

    pub fn remember_dir(&mut self, dir: PathBuf) {
        self.recent_dirs.retain(|d| d != &dir);
        self.recent_dirs.insert(0, dir);
        self.recent_dirs.truncate(10);
    }
}

pub struct SettingsStore {
    pub settings: Settings,
    /// Bumped on every change, so views can react to "settings were updated".
    pub revision: u64,
    pub themes: Vec<TerminalTheme>,
    /// The chosen theme with the user's own colour replacements applied. Everything draws from
    /// this, so a replaced colour reaches the terminals, the preview and the panels alike.
    composed: TerminalTheme,
}

impl Global for SettingsStore {}

impl SettingsStore {
    pub fn init(cx: &mut App) {
        let settings = Settings::load();
        let themes = load_themes();
        let composed = SettingsStore::compose(&settings, &themes);
        let store = SettingsStore { settings, revision: 0, themes, composed };
        let _ = store.save();
        if let Err(err) = crate::shell_integration::write_files(&store.settings.aliases) {
            eprintln!("agentty: shell integration unavailable: {err:#}");
        }
        if let Err(err) = crate::agent_guide::write_files() {
            eprintln!("agentty: agent guide unavailable: {err:#}");
        }
        crate::platform::wakelock::set(store.settings.prevent_sleep);
        cx.set_global(store);
    }

    fn save(&self) -> anyhow::Result<()> {
        let path = Settings::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.settings)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    /// The chosen theme, with every colour the user replaced put in.
    fn compose(settings: &Settings, themes: &[TerminalTheme]) -> TerminalTheme {
        let mut theme = themes.iter().find(|t| t.name == settings.theme).unwrap_or(&themes[0]).clone();
        for (slot, replacement) in [
            (&mut theme.background, settings.color_background),
            (&mut theme.foreground, settings.color_foreground),
            (&mut theme.cursor, settings.color_cursor),
            (&mut theme.selection, settings.color_selection),
        ] {
            if let Some(color) = replacement {
                *slot = color;
            }
        }
        theme
    }

    pub fn theme(&self) -> &TerminalTheme {
        &self.composed
    }
}

/// Read from the settings file (launch specs are built without an `App`).
pub fn browser_tools_enabled() -> bool {
    std::fs::read(Settings::path()).ok().and_then(|b| serde_json::from_slice::<Settings>(&b).ok()).is_none_or(|s| s.browser.agent_tools)
}

/// Whether agents get Agentty's guide, read from the settings file like [`browser_tools_enabled`].
pub fn agent_guide_enabled() -> bool {
    std::fs::read(Settings::path()).ok().and_then(|b| serde_json::from_slice::<Settings>(&b).ok()).is_none_or(|s| s.agent_guide)
}

/// The advisor for new Claude tabs, read from the settings file like [`browser_tools_enabled`].
pub fn advisor_default() -> AdvisorChoice {
    std::fs::read(Settings::path())
        .ok()
        .and_then(|b| serde_json::from_slice::<Settings>(&b).ok())
        .map_or_else(AdvisorChoice::default, |s| s.advisor)
}

pub fn settings(cx: &App) -> &Settings {
    &cx.global::<SettingsStore>().settings
}

pub fn terminal_theme(cx: &App) -> &TerminalTheme {
    cx.global::<SettingsStore>().theme()
}

/// Applies a change, persists it and repaints every window.
/// Seconds since the Unix epoch, for settings that remember a moment.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Turns a timed "Prevent sleep" off once its time is up — also when that happened while Agentty
/// was closed — and keeps the time left on screen fresh. Checked every 30 seconds.
pub fn start_prevent_sleep_timer(cx: &mut App) {
    cx.spawn(async move |cx| loop {
        let running = cx.update(|cx| {
            let now = unix_now();
            match settings(cx).prevent_sleep_left(now) {
                Some(0) => update_settings(cx, move |s| s.set_prevent_sleep(false, now)),
                Some(_) => cx.refresh_windows(),
                None => {}
            }
        });
        if running.is_err() {
            break;
        }
        cx.background_executor().timer(std::time::Duration::from_secs(30)).await;
    })
    .detach();
}

pub fn update_settings(cx: &mut App, change: impl FnOnce(&mut Settings)) {
    cx.update_global::<SettingsStore, _>(|store, _| {
        let aliases_before = store.settings.aliases.clone();
        change(&mut store.settings);
        store.composed = SettingsStore::compose(&store.settings, &store.themes);
        store.revision += 1;
        let _ = store.save();
        if store.settings.aliases != aliases_before {
            let _ = crate::shell_integration::write_files(&store.settings.aliases);
        }
        crate::platform::wakelock::set(store.settings.prevent_sleep);
    });
    cx.refresh_windows();
}

pub fn reload_themes(cx: &mut App) {
    cx.update_global::<SettingsStore, _>(|store, _| {
        store.themes = load_themes();
        store.composed = SettingsStore::compose(&store.settings, &store.themes);
    });
    cx.refresh_windows();
}

#[cfg(test)]
mod browser_settings_tests {
    use super::*;

    #[test]
    fn the_font_is_only_chosen_for_someone_who_did_not_pick_one() {
        // macOS in Korean with D2Coding installed gets it; anyone else, or without it, the bundled font.
        assert_eq!(default_font_for(true, Language::Ko, true), KOREAN_FONT);
        assert_eq!(default_font_for(true, Language::Ko, false), BUNDLED_FONT);
        assert_eq!(default_font_for(true, Language::En, true), BUNDLED_FONT);
        assert_eq!(default_font_for(true, Language::Ja, true), BUNDLED_FONT);
        // Windows and Linux always get the bundled font: D2Coding breaks PowerShell's glyphs.
        assert_eq!(default_font_for(false, Language::Ko, true), BUNDLED_FONT);

        // A fresh install has picked nothing.
        assert!(Settings::default().font_family.is_empty());
        // The old default was the only value there was: holding it means it was never changed.
        let old = |font: &str| Settings { settings_version: 3, font_family: font.into(), ..Settings::default() }.migrate();
        assert_eq!(old(BUNDLED_FONT).font_family, "");
        // A font someone picked stays theirs, D2Coding included.
        assert_eq!(old("D2Coding").font_family, "D2Coding");
        assert_eq!(old("Menlo").font_family, "Menlo");
        // Once migrated, picking the bundled font again is a choice, and it is kept.
        let current = Settings { font_family: BUNDLED_FONT.into(), ..Settings::default() }.migrate();
        assert_eq!(current.font_family, BUNDLED_FONT);
    }

    #[test]
    fn server_ports_show_in_the_status_bar_by_default() {
        let ports = |s: &Settings| s.hud.iter().find(|e| e.item == crate::hud::HudItem::Ports).map(|e| e.visible);
        assert_eq!(ports(&Settings::default()), Some(true));
        // Saved hidden by an older version: shown after the upgrade.
        let mut old = Settings { settings_version: 4, ..Settings::default() };
        old.hud.iter_mut().for_each(|e| e.visible = e.item != crate::hud::HudItem::Ports || e.item.required());
        assert_eq!(ports(&old.migrate()), Some(true));
        // Hidden once this version is in use: a choice, kept.
        let mut current = Settings::default().migrate();
        current.hud.iter_mut().filter(|e| e.item == crate::hud::HudItem::Ports).for_each(|e| e.visible = false);
        assert_eq!(ports(&current.migrate()), Some(false));
    }

    #[test]
    fn option_is_meta_by_default() {
        assert!(Settings::default().option_as_meta);
        let old = Settings { settings_version: 5, option_as_meta: false, ..Settings::default() };
        assert!(old.migrate().option_as_meta);
        // Turned off once this version is in use: a choice, kept.
        let current = Settings { option_as_meta: false, ..Settings::default().migrate() };
        assert!(!current.migrate().option_as_meta);
    }

    #[test]
    fn timed_prevent_sleep_counts_from_when_it_is_turned_on() {
        let mut s = Settings::default();
        // Always by default.
        assert_eq!(s.prevent_sleep_hours, 0);
        s.set_prevent_sleep(true, 1_000);
        assert_eq!((s.prevent_sleep_until, s.prevent_sleep_left(1_000)), (0, None));
        // Two hours, while on: counts from now.
        s.set_prevent_sleep_hours(2, 5_000);
        assert_eq!(s.prevent_sleep_until, 5_000 + 7_200);
        assert_eq!(s.prevent_sleep_left(6_000), Some(6_200));
        assert_eq!(s.prevent_sleep_left(20_000), Some(0));
        // Off and on again: a fresh two hours.
        s.set_prevent_sleep(false, 7_000);
        assert_eq!((s.prevent_sleep_until, s.prevent_sleep_left(7_000)), (0, None));
        s.set_prevent_sleep(true, 9_000);
        assert_eq!(s.prevent_sleep_until, 9_000 + 7_200);
        // Back to always.
        s.set_prevent_sleep_hours(0, 9_500);
        assert_eq!(s.prevent_sleep_until, 0);
        // Hours are capped.
        s.set_prevent_sleep_hours(500, 0);
        assert_eq!(s.prevent_sleep_hours, Settings::PREVENT_SLEEP_MAX_HOURS);
    }

    #[test]
    fn system_language_codes() {
        assert_eq!(Language::from_code("ko-KR"), Language::Ko);
        assert_eq!(Language::from_code("ja"), Language::Ja);
        assert_eq!(Language::from_code("zh-Hans-CN"), Language::Zh);
        assert_eq!(Language::from_code("fr-FR"), Language::En);
        assert_eq!(Language::from_code(""), Language::En);
        let parsed: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed.language, Language::System);
    }

    #[test]
    fn removed_search_engine_falls_back() {
        let parsed: BrowserSettings = serde_json::from_str(r#"{"searchEngine":"naver","zoom":1.2}"#).unwrap();
        assert_eq!((parsed.search_engine, parsed.zoom), (SearchEngine::Google, 1.2));
        let parsed: BrowserSettings = serde_json::from_str(r#"{"searchEngine":"bing"}"#).unwrap();
        assert_eq!(parsed.search_engine, SearchEngine::Bing);
    }
}

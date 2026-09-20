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

pub const SETTINGS_VERSION: u32 = 2;

pub const BUNDLED_FONT: &str = "JetBrains Mono";
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
    pub font_family: String,
    pub font_size: f32,
    pub line_height: f32,
    pub cursor_shape: CursorShapeSetting,
    pub cursor_blink: bool,
    pub padding: f32,
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
    /// The first-launch system check ran (Windows / Linux).
    pub setup_check_shown: bool,
    /// The first-run onboarding was finished or skipped.
    pub onboarding_done: bool,
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
    /// Also when an agent finishes (by default only when one needs an answer).
    pub on_finish: bool,
    /// Include what the agent asks (the command, the question). Off: only who and where.
    pub details: bool,
}

impl ChatNotify {
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

impl Default for Settings {
    fn default() -> Self {
        Self {
            settings_version: SETTINGS_VERSION,
            language: Language::System,
            theme: DEFAULT_THEME.into(),
            font_family: BUNDLED_FONT.into(),
            font_size: 13.0,
            line_height: 1.0,
            cursor_shape: CursorShapeSetting::Block,
            cursor_blink: true,
            padding: 8.0,
            option_as_meta: false,
            scrollback: 10_000,
            sidebar_width: 280.0,
            files_panel_width: 300.0,
            plugin_panel_width: crate::workbench::side_panels::DEFAULT_PLUGIN_WIDTH,
            plugin_panel_modes: std::collections::BTreeMap::new(),
            docker_panel_width: crate::workbench::side_panels::DEFAULT_DOCKER_WIDTH,
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
            setup_check_shown: false,
            onboarding_done: false,
        }
    }
}

impl Settings {
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
}

impl Global for SettingsStore {}

impl SettingsStore {
    pub fn init(cx: &mut App) {
        let store = SettingsStore { settings: Settings::load(), revision: 0, themes: load_themes() };
        let _ = store.save();
        if let Err(err) = crate::shell_integration::write_files(&store.settings.aliases) {
            eprintln!("agentty: shell integration unavailable: {err:#}");
        }
        if let Err(err) = crate::agent_guide::write_files() {
            eprintln!("agentty: agent guide unavailable: {err:#}");
        }
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

    pub fn theme(&self) -> &TerminalTheme {
        self.themes.iter().find(|t| t.name == self.settings.theme).unwrap_or(&self.themes[0])
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
pub fn update_settings(cx: &mut App, change: impl FnOnce(&mut Settings)) {
    cx.update_global::<SettingsStore, _>(|store, _| {
        let aliases_before = store.settings.aliases.clone();
        change(&mut store.settings);
        store.revision += 1;
        let _ = store.save();
        if store.settings.aliases != aliases_before {
            let _ = crate::shell_integration::write_files(&store.settings.aliases);
        }
    });
    cx.refresh_windows();
}

pub fn reload_themes(cx: &mut App) {
    cx.update_global::<SettingsStore, _>(|store, _| store.themes = load_themes());
    cx.refresh_windows();
}

#[cfg(test)]
mod browser_settings_tests {
    use super::*;

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

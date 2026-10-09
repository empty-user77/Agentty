//! What this binary is, for Settings → About and bug reports: the build (`build.rs` fills in
//! `AGENTTY_BUILD_*`) and the versions of the interfaces Agentty offers, read from the constants
//! the code itself checks against.

use crate::i18n::tr;
use crate::settings::Language;

pub const COMMIT: &str = env!("AGENTTY_BUILD_COMMIT");
pub const DATE: &str = env!("AGENTTY_BUILD_DATE");
pub const PROFILE: &str = env!("AGENTTY_BUILD_PROFILE");
pub const TARGET: &str = env!("AGENTTY_BUILD_TARGET");
pub const RUSTC: &str = env!("AGENTTY_BUILD_RUSTC");

pub enum Value {
    Text(String),
    /// A word that is translated (`about.build_included`, …).
    Label(&'static str),
}

pub struct Row {
    pub label: &'static str,
    pub value: Value,
}

fn row(label: &'static str, value: impl Into<String>) -> Row {
    Row { label, value: Value::Text(value.into()) }
}

pub fn build_rows() -> Vec<Row> {
    let voice = if agentty_bridge::voice::AVAILABLE { "about.build_included" } else { "about.build_not_included" };
    vec![
        row("about.build_version", crate::workbench::update::CURRENT_VERSION),
        row("about.build_commit", COMMIT),
        row("about.build_date", DATE),
        row("about.build_profile", PROFILE),
        row("about.build_target", TARGET),
        row("about.build_rust", RUSTC),
        Row { label: "about.build_voice", value: Value::Label(voice) },
    ]
}

pub fn api_rows() -> Vec<Row> {
    use agentty_bridge::plugins::{manifest, market};
    vec![
        row("about.api_plugin", manifest::API_VERSION.to_string()),
        row("about.api_marketplace", market::API_VERSION.to_string()),
        row("about.api_mcp", crate::browser_mcp::PROTOCOL_VERSION),
        row("about.api_shell", crate::shell_integration::SHELL_API),
        row("about.api_backup", agentty_bridge::backup::FORMAT_VERSION.to_string()),
        row("about.api_sync", agentty_bridge::sync::model::FORMAT_VERSION.to_string()),
    ]
}

pub fn value_text(value: &Value, language: Language) -> String {
    match value {
        Value::Text(text) => text.clone(),
        Value::Label(key) => tr(language, key).to_string(),
    }
}

/// Everything on the page as plain text, in English whatever the UI language, so a bug report
/// reads the same to whoever picks it up.
pub fn plain_text() -> String {
    let lines = |rows: Vec<Row>| {
        rows.iter().map(|r| format!("{}: {}\n", tr(Language::En, r.label), value_text(&r.value, Language::En))).collect::<String>()
    };
    format!("Agentty\n{}\n{}\n{}", lines(build_rows()), tr(Language::En, "about.api_versions"), lines(api_rows()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_lists_the_build_and_no_paths() {
        let text = plain_text();
        assert!(text.contains(&format!("Commit: {COMMIT}")), "{text}");
        assert!(text.contains(&format!("Plugin API: {}", agentty_bridge::plugins::manifest::API_VERSION)), "{text}");
        assert!(!text.contains(['/', '\\']), "no path may end up in it: {text}");
        assert!(!text.contains("about."), "an untranslated key: {text}");
    }

    #[test]
    fn build_values_are_short_names() {
        for value in [COMMIT, DATE, PROFILE, TARGET, RUSTC] {
            assert!(!value.is_empty() && value.len() < 64, "{value}");
            assert!(!value.contains(['/', '\\', ' ']), "{value}");
        }
    }
}

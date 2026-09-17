//! Public status pages (Atlassian Statuspage-compatible `summary.json`) of the AI services agents
//! depend on, so Agentty can warn when a running agent's service is degraded.

use anyhow::{Context, Result};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    pub summary_url: &'static str,
    pub page_url: &'static str,
    /// Components that matter for the agent (substring match); empty = the page's overall status.
    pub components: &'static [&'static str],
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "claude",
        name: "Claude Code",
        summary_url: "https://status.claude.com/api/v2/summary.json",
        page_url: "https://status.claude.com",
        components: &["Claude Code", "Claude API"],
    },
    Provider {
        id: "codex",
        name: "Codex",
        summary_url: "https://status.openai.com/api/v2/summary.json",
        page_url: "https://status.openai.com",
        components: &["Codex"],
    },
    Provider {
        id: "amp",
        name: "Amp",
        summary_url: "https://ampcodestatus.com/api/v2/summary.json",
        page_url: "https://ampcodestatus.com",
        components: &[],
    },
];

pub fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStatus {
    pub provider: &'static str,
    pub degraded: bool,
    /// Incident title or affected component, for the warning.
    pub detail: String,
    /// `degraded_performance`, `partial_outage`, `major_outage`, … (empty when operational).
    pub severity: String,
}

pub fn parse_summary(json: &serde_json::Value, provider: &Provider) -> ServiceStatus {
    let matches = |name: &str| provider.components.is_empty() || provider.components.iter().any(|c| name.contains(c));
    let affected: Vec<(String, String)> = json["components"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| Some((c["name"].as_str()?.to_string(), c["status"].as_str()?.to_string())))
        .filter(|(name, status)| matches(name) && status != "operational" && status != "under_maintenance")
        .collect();
    let incident = json["incidents"].as_array().into_iter().flatten().find(|i| {
        let open = !matches!(i["status"].as_str(), Some("resolved" | "postmortem" | "completed"));
        let touches = i["components"].as_array().map_or(provider.components.is_empty(), |cs| {
            cs.iter().filter_map(|c| c["name"].as_str()).any(matches) || (cs.is_empty() && provider.components.is_empty())
        });
        open && touches
    });
    let overall = provider.components.is_empty() && json["status"]["indicator"].as_str().is_some_and(|i| i != "none");
    let degraded = !affected.is_empty() || incident.is_some() || overall;
    let detail = incident
        .and_then(|i| i["name"].as_str().map(str::to_string))
        .or_else(|| affected.first().map(|(name, _)| name.clone()))
        .or_else(|| overall.then(|| json["status"]["description"].as_str().unwrap_or_default().to_string()))
        .unwrap_or_default();
    let severity = affected
        .first()
        .map(|(_, s)| s.clone())
        .or_else(|| incident.and_then(|i| i["impact"].as_str().map(str::to_string)))
        .unwrap_or_default();
    ServiceStatus { provider: provider.id, degraded, detail, severity: if degraded { severity } else { String::new() } }
}

pub fn fetch(provider: &Provider) -> Result<ServiceStatus> {
    let json: serde_json::Value = ureq::get(provider.summary_url)
        .set("User-Agent", "Agentty")
        .timeout(Duration::from_secs(15))
        .call()
        .with_context(|| format!("{} status unavailable", provider.name))?
        .into_json()?;
    Ok(parse_summary(&json, provider))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operational_page_is_not_degraded() {
        let json = serde_json::json!({ "status": { "indicator": "none" }, "components": [{ "name": "Claude Code", "status": "operational" }], "incidents": [] });
        assert!(!parse_summary(&json, provider("claude").unwrap()).degraded);
    }

    #[test]
    fn relevant_component_or_incident_degrades() {
        let json = serde_json::json!({
            "status": { "indicator": "minor" },
            "components": [{ "name": "claude.ai", "status": "major_outage" }, { "name": "Claude Code", "status": "degraded_performance" }],
            "incidents": [{ "name": "Elevated errors on Claude Code", "status": "investigating", "impact": "minor", "components": [{ "name": "Claude Code" }] }]
        });
        let status = parse_summary(&json, provider("claude").unwrap());
        assert!(status.degraded);
        assert_eq!((status.detail.as_str(), status.severity.as_str()), ("Elevated errors on Claude Code", "degraded_performance"));

        // An outage elsewhere on the page doesn't concern Codex.
        let other = serde_json::json!({ "status": { "indicator": "major" }, "components": [{ "name": "Sora", "status": "major_outage" }, { "name": "Codex API", "status": "operational" }], "incidents": [] });
        assert!(!parse_summary(&other, provider("codex").unwrap()).degraded);
    }
}

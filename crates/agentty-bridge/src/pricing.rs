//! USD per million tokens. Claude rates are Anthropic first-party list prices; anything else
//! (e.g. OpenAI models used by Codex) only has a price when the user configures it in
//! `~/.agentty/pricing.json`, so costs are never invented.

use crate::fsutil;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Price {
    pub input: f64,
    pub output: f64,
    /// Cache read rate. Defaults to 10% of input when omitted in pricing.json.
    #[serde(default)]
    pub cache_read: Option<f64>,
    /// 5-minute cache write rate. Defaults to 125% of input when omitted.
    #[serde(default)]
    pub cache_write: Option<f64>,
    /// 1-hour cache write rate. Defaults to 200% of input when omitted.
    #[serde(default)]
    pub cache_write_1h: Option<f64>,
}

impl Price {
    const fn claude(input: f64, output: f64, cache_read: f64) -> Self {
        Self { input, output, cache_read: Some(cache_read), cache_write: None, cache_write_1h: None }
    }

    pub fn cache_read(&self) -> f64 {
        self.cache_read.unwrap_or(self.input * 0.1)
    }
    pub fn cache_write_5m(&self) -> f64 {
        self.cache_write.unwrap_or(self.input * 1.25)
    }
    pub fn cache_write_1h(&self) -> f64 {
        self.cache_write_1h.unwrap_or(self.input * 2.0)
    }

    fn scaled(self, factor: f64) -> Self {
        Self {
            input: self.input * factor,
            output: self.output * factor,
            cache_read: self.cache_read.map(|v| v * factor),
            cache_write: self.cache_write.map(|v| v * factor),
            cache_write_1h: self.cache_write_1h.map(|v| v * factor),
        }
    }
}

/// Longest prefix first so `claude-fable-5-1` wins over `claude-fable-5`.
const CLAUDE: &[(&str, Price)] = &[
    ("claude-fable-5-1", Price::claude(10.0, 50.0, 0.25)),
    ("claude-mythos-5-1", Price::claude(10.0, 50.0, 0.25)),
    ("claude-fable-5", Price::claude(10.0, 50.0, 1.0)),
    ("claude-mythos-5", Price::claude(10.0, 50.0, 1.0)),
    ("claude-opus-5", Price::claude(5.0, 25.0, 0.5)),
    ("claude-opus-4-8", Price::claude(5.0, 25.0, 0.5)),
    ("claude-opus-4-7", Price::claude(5.0, 25.0, 0.5)),
    ("claude-opus-4-6", Price::claude(5.0, 25.0, 0.5)),
    ("claude-sonnet-5", Price::claude(2.0, 10.0, 0.2)),
    ("claude-sonnet-4-6", Price::claude(3.0, 15.0, 0.3)),
    ("claude-haiku-4-5", Price::claude(1.0, 5.0, 0.1)),
];

pub struct PriceTable {
    overrides: HashMap<String, Price>,
}

impl PriceTable {
    pub fn load() -> Self {
        let path = fsutil::data_dir().join("pricing.json");
        let overrides = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Self { overrides }
    }

    pub fn empty() -> Self {
        Self { overrides: HashMap::new() }
    }

    /// `fast` applies Claude fast-mode pricing (2x, Claude Opus 5 only).
    pub fn lookup(&self, model: &str, fast: bool) -> Option<Price> {
        if let Some(price) = self.overrides.get(model) {
            return Some(*price);
        }
        let (prefix, price) = CLAUDE.iter().find(|(prefix, _)| model.starts_with(prefix))?;
        Some(if fast && *prefix == "claude-opus-5" { price.scaled(2.0) } else { *price })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_longest_prefix() {
        let table = PriceTable::empty();
        assert_eq!(table.lookup("claude-fable-5-1", false).unwrap().cache_read(), 0.25);
        assert_eq!(table.lookup("claude-fable-5", false).unwrap().cache_read(), 1.0);
        assert_eq!(table.lookup("claude-opus-5", true).unwrap().input, 10.0);
        assert!(table.lookup("gpt-6-astra", false).is_none());
        assert!(table.lookup("<synthetic>", false).is_none());
    }
}

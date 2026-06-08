use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::bundle::Bundle;
use crate::errors::{AppError, Result};

/// Composed Anki Context field: episode briefing + subtitle neighbor lines.
pub fn format_card_context(
    line_context: &str,
    source_summary: Option<&str>,
    deck_summary: Option<&str>,
) -> String {
    let briefing = source_summary.or(deck_summary);
    match briefing {
        Some(summary) if !summary.is_empty() => {
            if line_context.is_empty() {
                format!("Episode summary: {summary}")
            } else {
                format!("Episode summary: {summary} | {line_context}")
            }
        }
        _ => line_context.to_string(),
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SummariesFile {
    /// Default briefing for the whole deck when a card's source has no entry.
    #[serde(default)]
    pub deck_summary: Option<String>,
    /// Per-episode briefings keyed by video filename, episode prefix, or source id.
    #[serde(default)]
    pub sources: HashMap<String, String>,
}

impl SummariesFile {
    pub fn from_path(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| AppError::Config(format!(
            "failed to read summaries file {}: {e}",
            path.display()
        )))?;
        serde_json::from_slice(&bytes).map_err(|e| AppError::Config(format!(
            "invalid summaries JSON {}: {e}",
            path.display()
        )))
    }

    pub fn import_into(&self, bundle: &Bundle) -> Result<ImportStats> {
        let mut stats = ImportStats::default();
        if let Some(summary) = &self.deck_summary {
            if !summary.trim().is_empty() {
                bundle.set_deck_summary(summary)?;
                stats.deck = true;
            }
        }
        for (key, summary) in &self.sources {
            if summary.trim().is_empty() {
                continue;
            }
            bundle.set_source_summary(key, None, summary)?;
            stats.sources += 1;
        }
        Ok(stats)
    }
}

#[derive(Debug, Default)]
pub struct ImportStats {
    pub deck: bool,
    pub sources: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_prepend_summary() {
        let out = format_card_context("prev\nnext", Some("Pekora hunts trident."), None);
        assert_eq!(
            out,
            "Episode summary: Pekora hunts trident. | prev\nnext"
        );
    }

    #[test]
    fn source_summary_over_deck() {
        let out = format_card_context(
            "ctx",
            Some("ep"),
            Some("deck"),
        );
        assert!(out.contains("Episode summary: ep"));
        assert!(!out.contains("deck"));
    }

    #[test]
    fn falls_back_to_deck_summary() {
        let out = format_card_context("ctx", None, Some("deck briefing"));
        assert_eq!(out, "Episode summary: deck briefing | ctx");
    }
}

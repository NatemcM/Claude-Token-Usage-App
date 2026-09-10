use crate::{LongestSession, ModelUsage, StatsCache};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct LegacyDay {
    /// Only a single total per model exists in the old format.
    pub tokens_by_model: HashMap<String, u64>,
    pub message_count: u64,
    pub session_count: u64,
    pub tool_call_count: u64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct LegacyRollup {
    pub days: BTreeMap<String, LegacyDay>,
    pub model_usage: HashMap<String, ModelUsage>,
    pub total_sessions: u64,
    pub total_messages: u64,
    pub first_session_date: Option<String>,
    pub hour_counts: HashMap<String, u64>,
    pub longest_session: Option<LongestSession>,
}

/// Read the retired `stats-cache.json` once and convert it into a rollup we
/// keep forever. Returns None when the file is absent or unreadable, which is
/// the normal case on a machine that never ran the old Claude Code.
pub fn seed_from_stats_cache(path: &Path) -> Option<LegacyRollup> {
    let contents = std::fs::read_to_string(path).ok()?;
    let old: StatsCache = serde_json::from_str(&contents).ok()?;

    let mut days: BTreeMap<String, LegacyDay> = BTreeMap::new();

    for a in &old.daily_activity {
        let d = days.entry(a.date.clone()).or_default();
        d.message_count = a.message_count;
        d.session_count = a.session_count;
        d.tool_call_count = a.tool_call_count;
    }
    for t in &old.daily_model_tokens {
        let d = days.entry(t.date.clone()).or_default();
        d.tokens_by_model = t.tokens_by_model.clone();
    }

    let model_usage = old
        .model_usage
        .iter()
        .map(|(m, u)| {
            (
                m.clone(),
                ModelUsage {
                    input_tokens: u.input_tokens,
                    output_tokens: u.output_tokens,
                    cache_read_input_tokens: u.cache_read_input_tokens,
                    cache_creation_input_tokens: u.cache_creation_input_tokens,
                    web_search_requests: u.web_search_requests,
                    cost_usd: 0.0, // non-goal
                },
            )
        })
        .collect();

    Some(LegacyRollup {
        days,
        model_usage,
        total_sessions: old.total_sessions,
        total_messages: old.total_messages,
        first_session_date: old.first_session_date.clone(),
        hour_counts: old.hour_counts.clone().unwrap_or_default(),
        longest_session: old.longest_session.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD_CACHE: &str = r#"{
      "version": 1,
      "lastComputedDate": "2026-03-10",
      "dailyActivity": [
        {"date":"2026-01-25","messageCount":10,"sessionCount":2,"toolCallCount":5},
        {"date":"2026-03-10","messageCount":20,"sessionCount":3,"toolCallCount":7}
      ],
      "dailyModelTokens": [
        {"date":"2026-01-25","tokensByModel":{"claude-opus-4-8":1000}},
        {"date":"2026-03-10","tokensByModel":{"claude-opus-4-8":2000}}
      ],
      "modelUsage": {
        "claude-opus-4-8": {
          "inputTokens": 100, "outputTokens": 200,
          "cacheReadInputTokens": 300, "cacheCreationInputTokens": 400,
          "webSearchRequests": 5, "costUsd": 1.23
        }
      },
      "totalSessions": 154,
      "totalMessages": 900,
      "longestSession": {"sessionId":"old-s","duration":3600,"messageCount":50,"timestamp":"2026-02-01"},
      "firstSessionDate": "2026-01-25",
      "hourCounts": {"9": 40, "14": 60}
    }"#;

    fn write_temp(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("stats-cache.json");
        std::fs::write(&path, contents).expect("write");
        (dir, path)
    }

    #[test]
    fn seeds_days_activity_and_tokens() {
        let (_dir, path) = write_temp(OLD_CACHE);
        let seed = seed_from_stats_cache(&path).expect("seed");

        assert_eq!(seed.days.len(), 2);
        let jan = seed.days.get("2026-01-25").expect("day");
        assert_eq!(jan.message_count, 10);
        assert_eq!(jan.session_count, 2);
        assert_eq!(jan.tool_call_count, 5);
        assert_eq!(jan.tokens_by_model["claude-opus-4-8"], 1000);
    }

    #[test]
    fn seeds_all_time_totals_and_breakdown() {
        let (_dir, path) = write_temp(OLD_CACHE);
        let seed = seed_from_stats_cache(&path).expect("seed");

        assert_eq!(seed.total_sessions, 154);
        assert_eq!(seed.total_messages, 900);
        assert_eq!(seed.first_session_date.as_deref(), Some("2026-01-25"));
        assert_eq!(seed.hour_counts["9"], 40);
        let mu = &seed.model_usage["claude-opus-4-8"];
        assert_eq!(mu.input_tokens, 100);
        assert_eq!(mu.output_tokens, 200);
        // Cost is a non-goal: it must be zeroed even though the old file had one.
        assert_eq!(mu.cost_usd, 0.0);
        assert_eq!(seed.longest_session.as_ref().expect("ls").duration, 3600);
    }

    #[test]
    fn handles_a_day_present_in_activity_but_not_tokens() {
        let contents = OLD_CACHE.replace(
            r#"{"date":"2026-01-25","tokensByModel":{"claude-opus-4-8":1000}},"#,
            "",
        );
        let (_dir, path) = write_temp(&contents);
        let seed = seed_from_stats_cache(&path).expect("seed");
        let jan = seed.days.get("2026-01-25").expect("day still present");
        assert_eq!(jan.message_count, 10);
        assert!(jan.tokens_by_model.is_empty());
    }

    #[test]
    fn missing_or_unparseable_old_cache_yields_none_not_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(seed_from_stats_cache(&dir.path().join("absent.json")).is_none());

        let (_d, bad) = write_temp("{ not json");
        assert!(seed_from_stats_cache(&bad).is_none());
    }
}

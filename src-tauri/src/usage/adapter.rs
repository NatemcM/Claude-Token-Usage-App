use crate::usage::dates::{date_key_with_offset, hour_with_offset};
use crate::usage::types::{TokenCounts, UsageCache};
use crate::{DailyActivity, DailyModelTokens, LongestSession, ModelUsage, StatsCache};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Merged view of one session, which may span a parent transcript plus any
/// number of subagent transcripts (all sharing the parent's sessionId).
#[derive(Default)]
struct MergedSession {
    first_ts: i64,
    last_ts: i64,
    message_count: u64,
}

#[derive(Default)]
struct MergedDay {
    by_model: HashMap<String, TokenCounts>,
    message_count: u64,
    tool_call_count: u64,
    session_ids: HashSet<String>,
}

/// Derive the StatsCache shape the frontend already consumes. Keeping this
/// contract means Dashboard.svelte, stats.ts and all 39 frontend tests keep
/// working against the new pipeline unchanged.
pub fn to_stats_cache(cache: &UsageCache) -> StatsCache {
    let tz = cache.tz_offset_minutes;

    let mut days: BTreeMap<String, MergedDay> = BTreeMap::new();
    let mut sessions: HashMap<String, MergedSession> = HashMap::new();
    let mut model_usage: HashMap<String, TokenCounts> = HashMap::new();

    for entry in cache.files.values() {
        for (date, day) in &entry.days {
            let merged = days.entry(date.clone()).or_default();
            merged.message_count += day.message_count;
            merged.tool_call_count += day.tool_call_count;
            // Union, never sum: subagent files repeat the parent's sessionId.
            for sid in &day.session_ids {
                merged.session_ids.insert(sid.clone());
            }
            for (model, counts) in &day.by_model {
                merged.by_model.entry(model.clone()).or_default().add(counts);
                model_usage.entry(model.clone()).or_default().add(counts);
            }
        }

        if entry.session.session_id.is_empty() {
            continue;
        }
        let s = sessions.entry(entry.session.session_id.clone()).or_default();
        if s.first_ts == 0 || (entry.session.first_ts != 0 && entry.session.first_ts < s.first_ts) {
            s.first_ts = entry.session.first_ts;
        }
        if entry.session.last_ts > s.last_ts {
            s.last_ts = entry.session.last_ts;
        }
        s.message_count += entry.session.message_count;
    }

    let daily_activity: Vec<DailyActivity> = days
        .iter()
        .map(|(date, d)| DailyActivity {
            date: date.clone(),
            message_count: d.message_count,
            session_count: d.session_ids.len() as u64,
            tool_call_count: d.tool_call_count,
        })
        .collect();

    let daily_model_tokens: Vec<DailyModelTokens> = days
        .iter()
        .map(|(date, d)| DailyModelTokens {
            date: date.clone(),
            tokens_by_model: d
                .by_model
                .iter()
                .map(|(m, c)| (m.clone(), c.billable_total()))
                .collect(),
        })
        .collect();

    let model_usage_out: HashMap<String, ModelUsage> = model_usage
        .iter()
        .map(|(model, c)| {
            (
                model.clone(),
                ModelUsage {
                    input_tokens: c.input,
                    output_tokens: c.output,
                    cache_read_input_tokens: c.cache_read,
                    cache_creation_input_tokens: c.cache_creation,
                    web_search_requests: c.web_search,
                    cost_usd: 0.0, // non-goal: no price table
                },
            )
        })
        .collect();

    let longest_session = sessions
        .iter()
        .max_by_key(|(_, s)| s.last_ts.saturating_sub(s.first_ts))
        .filter(|(_, s)| s.last_ts > s.first_ts)
        .map(|(id, s)| LongestSession {
            session_id: id.clone(),
            duration: (s.last_ts - s.first_ts) as u64,
            message_count: s.message_count,
            timestamp: date_key_with_offset(s.first_ts, tz),
        });

    let first_session_date = sessions
        .values()
        .map(|s| s.first_ts)
        .filter(|t| *t != 0)
        .min()
        .map(|t| date_key_with_offset(t, tz));

    let mut hour_counts: HashMap<String, u64> = HashMap::new();
    for s in sessions.values() {
        if s.first_ts == 0 {
            continue;
        }
        *hour_counts
            .entry(hour_with_offset(s.first_ts, tz).to_string())
            .or_insert(0) += 1;
    }

    let last_computed_date = days
        .keys()
        .next_back()
        .cloned()
        .unwrap_or_else(|| "".to_string());

    StatsCache {
        version: 1,
        last_computed_date,
        daily_activity,
        daily_model_tokens,
        model_usage: model_usage_out,
        total_sessions: sessions.len() as u64,
        total_messages: sessions.values().map(|s| s.message_count).sum(),
        longest_session,
        first_session_date,
        hour_counts: if hour_counts.is_empty() { None } else { Some(hour_counts) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::types::{DayRollup, FileEntry, TokenCounts, UsageCache};

    fn counts(input: u64, output: u64, cr: u64, cc: u64) -> TokenCounts {
        TokenCounts { input, output, cache_read: cr, cache_creation: cc, ..Default::default() }
    }

    fn entry_with(
        session_id: &str,
        day: &str,
        model: &str,
        c: TokenCounts,
        messages: u64,
        tools: u64,
        first_ts: i64,
        last_ts: i64,
    ) -> FileEntry {
        let mut e = FileEntry::default();
        e.session.session_id = session_id.to_string();
        e.session.first_ts = first_ts;
        e.session.last_ts = last_ts;
        e.session.message_count = messages;
        e.session.by_model.insert(model.to_string(), c.clone());
        let mut d = DayRollup::default();
        d.by_model.insert(model.to_string(), c);
        d.message_count = messages;
        d.tool_call_count = tools;
        d.session_ids.insert(session_id.to_string());
        e.days.insert(day.to_string(), d);
        e
    }

    #[test]
    fn sums_tokens_per_day_across_files() {
        let mut cache = UsageCache::new(0);
        cache.files.insert(
            "/a.jsonl".into(),
            entry_with("s1", "2026-09-10", "claude-opus-5", counts(1, 2, 3, 4), 1, 1, 100, 200),
        );
        cache.files.insert(
            "/b.jsonl".into(),
            entry_with("s2", "2026-09-10", "claude-opus-5", counts(10, 20, 30, 40), 1, 1, 300, 400),
        );

        let stats = to_stats_cache(&cache);
        let day = stats
            .daily_model_tokens
            .iter()
            .find(|d| d.date == "2026-09-10")
            .expect("day");
        // billable_total per model: (1+2+3+4) + (10+20+30+40) = 10 + 100
        assert_eq!(day.tokens_by_model["claude-opus-5"], 110);
    }

    #[test]
    fn unions_session_ids_per_day_instead_of_summing() {
        // A parent transcript and its subagent transcript share one sessionId
        // across two files. Session count for that day must be 1, not 2.
        let mut cache = UsageCache::new(0);
        cache.files.insert(
            "/parent.jsonl".into(),
            entry_with("same-session", "2026-09-10", "claude-opus-5", counts(1, 1, 0, 0), 1, 0, 100, 200),
        );
        cache.files.insert(
            "/parent/subagents/agent-x.jsonl".into(),
            entry_with("same-session", "2026-09-10", "claude-sonnet-5", counts(1, 1, 0, 0), 1, 0, 150, 180),
        );

        let stats = to_stats_cache(&cache);
        let day = stats
            .daily_activity
            .iter()
            .find(|d| d.date == "2026-09-10")
            .expect("day");
        assert_eq!(day.session_count, 1, "subagent files must not inflate sessions");
        assert_eq!(stats.total_sessions, 1);
    }

    #[test]
    fn merges_one_session_split_across_files_for_duration() {
        let mut cache = UsageCache::new(0);
        cache.files.insert(
            "/p.jsonl".into(),
            entry_with("s1", "2026-09-10", "claude-opus-5", counts(1, 1, 0, 0), 1, 0, 1_000, 5_000),
        );
        cache.files.insert(
            "/p/subagents/a.jsonl".into(),
            entry_with("s1", "2026-09-10", "claude-opus-5", counts(1, 1, 0, 0), 1, 0, 2_000, 9_000),
        );

        let stats = to_stats_cache(&cache);
        let longest = stats.longest_session.expect("longest");
        assert_eq!(longest.session_id, "s1");
        // Merged span: 1_000 -> 9_000.
        assert_eq!(longest.duration, 8_000);
    }

    #[test]
    fn accumulates_all_time_model_usage() {
        let mut cache = UsageCache::new(0);
        cache.files.insert(
            "/a.jsonl".into(),
            entry_with("s1", "2026-09-10", "claude-opus-5", counts(1, 2, 3, 4), 1, 0, 1, 2),
        );
        cache.files.insert(
            "/b.jsonl".into(),
            entry_with("s2", "2026-09-11", "claude-opus-5", counts(5, 6, 7, 8), 1, 0, 3, 4),
        );

        let stats = to_stats_cache(&cache);
        let mu = &stats.model_usage["claude-opus-5"];
        assert_eq!(mu.input_tokens, 6);
        assert_eq!(mu.output_tokens, 8);
        assert_eq!(mu.cache_read_input_tokens, 10);
        assert_eq!(mu.cache_creation_input_tokens, 12);
        assert_eq!(mu.cost_usd, 0.0, "costs are a non-goal");
    }

    #[test]
    fn emits_days_sorted_ascending() {
        let mut cache = UsageCache::new(0);
        cache.files.insert(
            "/b.jsonl".into(),
            entry_with("s2", "2026-09-11", "claude-opus-5", counts(1, 1, 0, 0), 1, 0, 3, 4),
        );
        cache.files.insert(
            "/a.jsonl".into(),
            entry_with("s1", "2026-09-09", "claude-opus-5", counts(1, 1, 0, 0), 1, 0, 1, 2),
        );
        let stats = to_stats_cache(&cache);
        let dates: Vec<&str> = stats.daily_activity.iter().map(|d| d.date.as_str()).collect();
        assert_eq!(dates, vec!["2026-09-09", "2026-09-11"]);
    }

    #[test]
    fn derives_hour_counts_from_session_start_times() {
        let mut cache = UsageCache::new(0);
        // first_ts = 1789029179076 -> 2026-09-10T08:32:59Z -> hour 8 at UTC.
        cache.files.insert(
            "/a.jsonl".into(),
            entry_with("s1", "2026-09-10", "claude-opus-5", counts(1, 1, 0, 0), 1, 0, 1789029179076, 1789029179076),
        );
        let stats = to_stats_cache(&cache);
        let hc = stats.hour_counts.expect("hour counts");
        assert_eq!(hc.get("8"), Some(&1));
    }

    #[test]
    fn retired_entries_still_contribute_history() {
        let mut cache = UsageCache::new(0);
        let mut e = entry_with("s-old", "2026-08-01", "claude-opus-5", counts(1, 1, 1, 1), 2, 3, 10, 20);
        e.retired = true;
        e.seen.clear(); // as happens on retirement
        cache.files.insert("/pruned.jsonl".into(), e);

        let stats = to_stats_cache(&cache);
        assert_eq!(stats.daily_model_tokens.len(), 1);
        assert_eq!(stats.total_messages, 2);
        assert_eq!(stats.first_session_date.as_deref(), Some("1970-01-01"));
    }

    #[test]
    fn empty_cache_yields_valid_empty_stats() {
        let stats = to_stats_cache(&UsageCache::new(0));
        assert_eq!(stats.total_sessions, 0);
        assert_eq!(stats.total_messages, 0);
        assert!(stats.daily_activity.is_empty());
        assert!(stats.longest_session.is_none());
        assert!(stats.first_session_date.is_none());
    }
}

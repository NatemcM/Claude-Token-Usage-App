use crate::usage::dates::date_key_with_offset;
use crate::usage::record::{parse_line, ParseOutcome, ParsedRecord, RecordKind};
use crate::usage::types::{AgentRollup, DayRollup, FileEntry};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct IngestStats {
    pub records: u64,
    pub malformed: u64,
    /// Duplicate copies skipped by token dedup.
    pub deduped: u64,
}

/// FNV-1a over the bytes of an identifier. Stable across runs, unlike
/// DefaultHasher, so it is safe to persist in the cache.
pub fn key_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Apply one complete transcript line to `entry`. This is what the scanner
/// streams into, so no whole file is ever buffered.
pub fn ingest_line(
    entry: &mut FileEntry,
    line: &str,
    tz_offset_minutes: i32,
    stats: &mut IngestStats,
) {
    let rec = match parse_line(line) {
        ParseOutcome::Record(r) => r,
        ParseOutcome::Skipped => return,
        ParseOutcome::Malformed => {
            stats.malformed += 1;
            entry.malformed_lines += 1;
            return;
        }
    };
    stats.records += 1;
    apply_record(entry, &rec, tz_offset_minutes, stats);
}

/// Convenience loop over `ingest_line`, used by tests. No production caller.
#[cfg(test)]
pub fn ingest_text(entry: &mut FileEntry, text: &str, tz_offset_minutes: i32) -> IngestStats {
    let mut stats = IngestStats::default();
    for line in text.lines() {
        ingest_line(entry, line, tz_offset_minutes, &mut stats);
    }
    stats
}

fn apply_record(
    entry: &mut FileEntry,
    rec: &ParsedRecord,
    tz_offset_minutes: i32,
    stats: &mut IngestStats,
) {
    let day_key = date_key_with_offset(rec.ts_ms, tz_offset_minutes);

    // --- Session identity and timestamps: driven by records of ANY type ---
    if entry.session.session_id.is_empty() {
        entry.session.session_id = rec.session_id.clone();
    }
    if entry.session.cwd.is_none() {
        entry.session.cwd = rec.cwd.clone();
    }
    if rec.git_branch.is_some() {
        entry.session.git_branch = rec.git_branch.clone();
    }
    if entry.session.first_ts == 0 || rec.ts_ms < entry.session.first_ts {
        entry.session.first_ts = rec.ts_ms;
    }
    if rec.ts_ms > entry.session.last_ts {
        entry.session.last_ts = rec.ts_ms;
    }

    let day = entry.days.entry(day_key).or_insert_with(DayRollup::default);
    day.session_ids.insert(rec.session_id.clone());

    // --- Tool calls: dedupe on block id ---
    for tid in &rec.tool_use_ids {
        if entry.seen_tools.insert(key_hash(tid)) {
            day.tool_call_count += 1;
        }
    }

    // --- User messages: dedupe on uuid, skipping tool-result echoes ---
    if rec.kind == RecordKind::User {
        // Echoes are transport, not messages. Their timestamp already
        // contributed to last_ts above, which is all they are good for.
        if rec.is_tool_result_echo {
            return;
        }
        let is_new = match &rec.uuid {
            Some(u) => entry.seen_users.insert(key_hash(u)),
            // No uuid: cannot dedupe, count it rather than silently drop it.
            None => true,
        };
        if is_new {
            day.message_count += 1;
            entry.session.message_count += 1;
        }
        return;
    }

    // --- Assistant tokens and message count: dedupe on message.id ---
    if rec.kind != RecordKind::Assistant {
        return;
    }
    let Some(msg_id) = rec.message_id.as_deref() else {
        return;
    };
    let id_hash = key_hash(msg_id);
    let usage = rec.usage.clone().unwrap_or_default();

    // Decide what this copy contributes. A first sighting credits its whole
    // usage; a repeat credits only the amount by which it EXCEEDS what we
    // already credited, because early copies are written mid-stream with
    // partial counts. The borrow of `entry.seen` ends before the match body
    // touches other fields.
    let seen_before = entry.seen.contains_key(&id_hash);
    let to_apply = if seen_before {
        stats.deduped += 1;
        let delta = entry
            .seen
            .get_mut(&id_hash)
            .and_then(|credited| credited.raise_to(&usage));
        if delta.is_some() {
            entry.revised_messages += 1;
        }
        delta
    } else {
        entry.seen.insert(id_hash, usage.clone());
        day.message_count += 1;
        entry.session.message_count += 1;
        Some(usage)
    };

    let Some(counts) = to_apply else {
        return;
    };
    if counts.is_empty() {
        return;
    }

    let model = rec.model.clone().unwrap_or_else(|| "unknown".to_string());
    day.by_model.entry(model.clone()).or_default().add(&counts);
    entry
        .session
        .by_model
        .entry(model)
        .or_default()
        .add(&counts);

    if rec.ts_ms > entry.session.last_usage_ts {
        entry.session.last_usage_ts = rec.ts_ms;
    }

    if let Some(agent_id) = rec.agent_id.as_deref() {
        let agent = entry
            .agents
            .entry(agent_id.to_string())
            .or_insert_with(|| AgentRollup {
                agent_id: agent_id.to_string(),
                ..Default::default()
            });
        if rec.agent_type.is_some() {
            agent.agent_type = rec.agent_type.clone();
        }
        if rec.ts_ms > agent.last_ts {
            agent.last_ts = rec.ts_ms;
        }
        agent.tokens.add(&counts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::types::FileEntry;

    /// One assistant line carrying a single content block, mirroring the real
    /// format where each block gets its own line with an identical usage object.
    fn assistant_line(msg_id: &str, block: &str, out: u64) -> String {
        let block_json = match block {
            "tool_use" => r#"{"type":"tool_use","id":"TOOLID","name":"Bash","input":{}}"#.to_string(),
            "thinking" => r#"{"type":"thinking","thinking":"..."}"#.to_string(),
            _ => r#"{"type":"text","text":"hi"}"#.to_string(),
        };
        format!(
            r#"{{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","sessionId":"s-1","cwd":"/p","message":{{"id":"{}","model":"claude-opus-5","content":[{}],"usage":{{"input_tokens":10,"output_tokens":{},"cache_read_input_tokens":100,"cache_creation_input_tokens":5}}}}}}"#,
            msg_id, block_json, out
        )
    }

    fn tool_line(msg_id: &str, tool_id: &str) -> String {
        assistant_line(msg_id, "tool_use", 7).replace("TOOLID", tool_id)
    }

    #[test]
    fn identical_blocks_of_one_message_count_tokens_once() {
        // Inflation regression: three block-lines, one message, equal usage.
        let text = format!(
            "{}\n{}\n{}\n",
            assistant_line("msg_1", "thinking", 894),
            assistant_line("msg_1", "text", 894),
            assistant_line("msg_1", "tool_use", 894),
        );
        let mut entry = FileEntry::default();
        let stats = ingest_text(&mut entry, &text, 0);

        assert_eq!(stats.records, 3);
        assert_eq!(stats.deduped, 2, "two duplicate copies must be skipped");

        let day = entry.days.get("2026-09-10").expect("day");
        let counts = day.by_model.get("claude-opus-5").expect("model");
        assert_eq!(counts.output, 894, "must NOT be 2682");
        assert_eq!(counts.input, 10);
        assert_eq!(counts.cache_read, 100);
        assert_eq!(counts.cache_creation, 5);
    }

    #[test]
    fn non_consecutive_reemission_of_a_message_counts_once() {
        let text = format!(
            "{}\n{}\n{}\n{}\n",
            assistant_line("msg_a", "text", 100),
            assistant_line("msg_b", "text", 200),
            assistant_line("msg_a", "text", 100), // re-emitted later
            assistant_line("msg_b", "text", 200),
        );
        let mut entry = FileEntry::default();
        ingest_text(&mut entry, &text, 0);
        let counts = entry.days["2026-09-10"].by_model["claude-opus-5"].clone();
        assert_eq!(counts.output, 300);
    }

    #[test]
    fn duplicate_straddling_two_incremental_passes_counts_once() {
        let mut entry = FileEntry::default();
        ingest_text(&mut entry, &format!("{}\n", assistant_line("msg_1", "text", 500)), 0);
        // Second pass sees the same message id again (re-emitted after append).
        let stats = ingest_text(&mut entry, &format!("{}\n", assistant_line("msg_1", "tool_use", 500)), 0);
        assert_eq!(stats.deduped, 1);
        assert_eq!(entry.days["2026-09-10"].by_model["claude-opus-5"].output, 500);
    }

    #[test]
    fn a_later_larger_copy_revises_the_total_upward() {
        // THE regression test for the 45% undercount. The mid-stream copy
        // reports 7 output tokens; the complete copy reports 156.
        let text = format!(
            "{}\n{}\n",
            assistant_line("msg_1", "thinking", 7),
            assistant_line("msg_1", "text", 156),
        );
        let mut entry = FileEntry::default();
        ingest_text(&mut entry, &text, 0);

        assert_eq!(
            entry.days["2026-09-10"].by_model["claude-opus-5"].output, 156,
            "must be the max (156), not the first (7) and not the sum (163)"
        );
        assert_eq!(entry.revised_messages, 1);
        assert_eq!(entry.days["2026-09-10"].message_count, 1, "one message, two lines");
    }

    #[test]
    fn a_smaller_later_copy_never_lowers_the_total() {
        let text = format!(
            "{}\n{}\n",
            assistant_line("msg_1", "text", 156),
            assistant_line("msg_1", "thinking", 7),
        );
        let mut entry = FileEntry::default();
        ingest_text(&mut entry, &text, 0);
        assert_eq!(entry.days["2026-09-10"].by_model["claude-opus-5"].output, 156);
        assert_eq!(entry.revised_messages, 0);
    }

    #[test]
    fn revision_straddling_two_passes_still_reaches_the_max() {
        let mut entry = FileEntry::default();
        ingest_text(&mut entry, &format!("{}\n", assistant_line("msg_1", "thinking", 7)), 0);
        ingest_text(&mut entry, &format!("{}\n", assistant_line("msg_1", "text", 156)), 0);
        assert_eq!(entry.days["2026-09-10"].by_model["claude-opus-5"].output, 156);
    }

    #[test]
    fn tool_result_echoes_are_not_counted_as_messages() {
        let echo = r#"{"type":"user","uuid":"u-1","timestamp":"2026-09-10T08:30:00Z","sessionId":"s-1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_a"}]}}"#;
        let prompt = r#"{"type":"user","uuid":"u-2","timestamp":"2026-09-10T08:31:00Z","sessionId":"s-1","message":{"role":"user","content":"a real prompt"}}"#;
        let mut entry = FileEntry::default();
        ingest_text(&mut entry, &format!("{}\n{}\n", echo, prompt), 0);
        assert_eq!(entry.days["2026-09-10"].message_count, 1, "only the prompt counts");
    }

    #[test]
    fn tool_calls_dedupe_on_block_id_not_message_id() {
        // Two distinct tools on the SAME message, each on its own line, plus a
        // repeat of the first. Correct answer is 2.
        let text = format!(
            "{}\n{}\n{}\n",
            tool_line("msg_1", "toolu_a"),
            tool_line("msg_1", "toolu_b"),
            tool_line("msg_1", "toolu_a"),
        );
        let mut entry = FileEntry::default();
        ingest_text(&mut entry, &text, 0);
        assert_eq!(entry.days["2026-09-10"].tool_call_count, 2);
    }

    #[test]
    fn user_messages_dedupe_on_uuid() {
        let u = |uuid: &str| format!(
            r#"{{"type":"user","uuid":"{}","timestamp":"2026-09-10T08:00:00Z","sessionId":"s-1","message":{{"role":"user","content":"hi"}}}}"#,
            uuid
        );
        let text = format!("{}\n{}\n{}\n", u("u-1"), u("u-2"), u("u-1"));
        let mut entry = FileEntry::default();
        ingest_text(&mut entry, &text, 0);
        // 2 distinct user messages, 0 assistant messages.
        assert_eq!(entry.days["2026-09-10"].message_count, 2);
    }

    #[test]
    fn assistant_messages_count_once_per_message_id() {
        let text = format!(
            "{}\n{}\n{}\n",
            assistant_line("msg_1", "text", 1),
            assistant_line("msg_1", "thinking", 1),
            assistant_line("msg_2", "text", 1),
        );
        let mut entry = FileEntry::default();
        ingest_text(&mut entry, &text, 0);
        assert_eq!(entry.days["2026-09-10"].message_count, 2);
    }

    #[test]
    fn tracks_session_identity_and_timestamps_from_any_record() {
        let late = r#"{"type":"file-history-snapshot","timestamp":"2026-09-10T09:00:00Z","sessionId":"s-1"}"#;
        let text = format!("{}\n{}\n", assistant_line("msg_1", "text", 5), late);
        let mut entry = FileEntry::default();
        ingest_text(&mut entry, &text, 0);

        assert_eq!(entry.session.session_id, "s-1");
        assert_eq!(entry.session.cwd.as_deref(), Some("/p"));
        assert_eq!(entry.session.first_ts, 1789029179000); // 08:32:59Z
        // last_ts must come from the snapshot, NOT the assistant record, so a
        // session mid-tool-call is not reported idle.
        assert_eq!(entry.session.last_ts, 1789030800000); // 09:00:00Z
        assert_eq!(entry.session.last_usage_ts, 1789029179000);
        assert_eq!(entry.days["2026-09-10"].session_ids.len(), 1);
    }

    #[test]
    fn attributes_subagent_tokens_to_agent_and_parent_day() {
        let line = r#"{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","sessionId":"parent","agentId":"a1","attributionAgent":"Explore","message":{"id":"m1","model":"claude-sonnet-5","content":[],"usage":{"input_tokens":1,"output_tokens":50}}}"#;
        let mut entry = FileEntry::default();
        ingest_text(&mut entry, &format!("{}\n", line), 0);

        let agent = entry.agents.get("a1").expect("agent");
        assert_eq!(agent.agent_type.as_deref(), Some("Explore"));
        assert_eq!(agent.tokens.output, 50);
        // Also in the parent's day total, exactly once.
        assert_eq!(entry.days["2026-09-10"].by_model["claude-sonnet-5"].output, 50);
    }

    #[test]
    fn subagent_revision_reaches_day_session_and_agent_at_the_max() {
        // Same subagent message.id, first partial (7) then complete (156).
        // A regression that credits the delta to the day only, or
        // double-credits the session, would still pass every other test.
        let mk = |out: u64| {
            format!(
                r#"{{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","sessionId":"parent","agentId":"a1","attributionAgent":"Explore","message":{{"id":"m1","model":"claude-sonnet-5","content":[],"usage":{{"input_tokens":1,"output_tokens":{}}}}}}}"#,
                out
            )
        };
        let text = format!("{}\n{}\n", mk(7), mk(156));
        let mut entry = FileEntry::default();
        ingest_text(&mut entry, &text, 0);

        assert_eq!(
            entry.days["2026-09-10"].by_model["claude-sonnet-5"].output, 156,
            "day total must be the max, not 7 and not 163"
        );
        assert_eq!(
            entry.session.by_model["claude-sonnet-5"].output, 156,
            "session total must be the max, not 7 and not 163"
        );
        assert_eq!(
            entry.agents["a1"].tokens.output, 156,
            "agent total must be the max, not 7 and not 163"
        );
        assert_eq!(entry.session.message_count, 1, "one message, two lines");
    }

    #[test]
    fn key_hash_matches_published_fnv1a_64_test_vectors() {
        // key_hash is persisted to the cache file, so it must be a specific,
        // stable algorithm (FNV-1a) rather than whatever DefaultHasher does
        // this run. Swapping it for DefaultHasher would pass every other
        // test (within-process dedup works either way) while silently
        // breaking cache reuse across restarts.
        assert_eq!(key_hash(""), 0xcbf29ce484222325);
        assert_eq!(key_hash("a"), 0xaf63dc4c8601ec8c);
        assert_eq!(key_hash("foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn malformed_lines_are_counted_and_do_not_abort_ingest() {
        let text = format!(
            "{}\n{{not json\n{}\n",
            assistant_line("msg_1", "text", 10),
            assistant_line("msg_2", "text", 20),
        );
        let mut entry = FileEntry::default();
        let stats = ingest_text(&mut entry, &text, 0);
        assert_eq!(stats.malformed, 1);
        assert_eq!(entry.malformed_lines, 1);
        assert_eq!(entry.days["2026-09-10"].by_model["claude-opus-5"].output, 30);
    }

    #[test]
    fn synthetic_model_contributes_no_tokens() {
        let line = r#"{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","sessionId":"s","isApiErrorMessage":true,"message":{"id":"m","model":"<synthetic>","content":[],"usage":{"input_tokens":0,"output_tokens":0}}}"#;
        let mut entry = FileEntry::default();
        ingest_text(&mut entry, &format!("{}\n", line), 0);
        assert!(!entry.days["2026-09-10"].by_model.contains_key("<synthetic>"));
    }

    #[test]
    fn buckets_into_local_day_using_supplied_offset() {
        let line = format!(
            r#"{{"type":"assistant","timestamp":"2026-09-10T20:00:00Z","sessionId":"s","message":{{"id":"m","model":"claude-opus-5","content":[],"usage":{{"input_tokens":1,"output_tokens":1}}}}}}"#
        );
        let mut entry = FileEntry::default();
        ingest_text(&mut entry, &format!("{}\n", line), 420); // UTC+07
        assert!(entry.days.contains_key("2026-09-11"));
        assert!(!entry.days.contains_key("2026-09-10"));
    }
}

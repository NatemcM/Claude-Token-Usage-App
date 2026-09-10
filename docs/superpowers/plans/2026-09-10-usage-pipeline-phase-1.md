# Usage Pipeline (Phase 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the app's dead data source (`~/.claude/stats-cache.json`, last written 2026-03-11) with an incrementally-ingested, deduplicated rollup of Claude Code's per-session transcripts, so the dashboard shows accurate live numbers again.

**Architecture:** A new `usage` module walks `~/.claude/projects/**/*.jsonl` recursively, parses only the lines it needs, and stores **per-file** rollups keyed by byte offset so each refresh reads only newly-appended bytes. Rollups survive their source file being pruned, making our cache the durable long-term record. A thin adapter derives the existing `StatsCache` struct from those rollups, so `get_stats` keeps its signature and the entire Svelte frontend plus its 39 tests continue to work unchanged.

**Tech Stack:** Rust (Tauri v2, serde, serde_json, notify, chrono), Svelte 5 runes, Vitest, `cargo test`.

**Spec:** `docs/superpowers/specs/2026-09-10-usage-pipeline-and-session-control-design.md`

## Global Constraints

- **Three independent dedup keys.** Each transcript line carries exactly ONE content block, and whole messages are re-emitted non-consecutively, so one key cannot serve all three metrics:
  - **Tokens** -> `hash(message.id)`. The usage object is repeated identically on every block line. `requestId` is absent on 18 observed records and must never form part of this key. Undeduped output tokens inflate **4.70x** (98.5M vs 20.9M actual).
  - **Tool calls** -> `hash(tool_use block id)`. Globally 68,492 raw blocks vs 54,229 distinct ids. Using `message.id` here would undercount 3x (527 vs 1,907 in one measured file).
  - **User messages** -> `hash(uuid)`. 72,443 raw vs 57,122 distinct (1.27x).
  These are correctness, not optimization.
- **Never write to `~/.claude` in Phase 1.** It is read-only. Our cache lives under the app data dir only.
- **Do not enable macOS App Sandbox.** It breaks reading `~/.claude` and (in Phase 3) signalling processes.
- **Keep raw model ids.** Never collapse `claude-opus-4-8` into a family bucket.
- **Exclude `<synthetic>`** models from all token totals.
- **Bucket by local date**, never UTC. Record `tz_offset_minutes` in the cache.
- **Never `read_to_string` a transcript** — the largest observed file is 95 MB. Stream from the stored offset with `BufReader`.
- **Advance a stored offset only to the last complete newline.** Live sessions leave half-written records.
- **Missing/unknown field means skip the record and count it**, never fail the ingest. Every field is observed behavior of Claude Code 2.1.267, not a published contract.
- **No dollar cost estimates.** `ModelUsage.cost_usd` is emitted as `0.0`.
- **Rust serde convention:** all types crossing the Tauri boundary use `#[serde(rename_all = "camelCase")]`. Internal cache types use snake_case (they are never read by the frontend).
- **Existing test suites must stay green:** `npm test` (39 tests) and `cargo test`.

---

## File Structure

**New Rust module** `src-tauri/src/usage/` — one responsibility per file, each with inline `#[cfg(test)] mod tests` matching the convention already in `lib.rs`:

| File | Responsibility |
|---|---|
| `usage/mod.rs` | Module wiring and the public surface used by `lib.rs` |
| `usage/types.rs` | Rollup types: `TokenCounts`, `DayRollup`, `SessionRollup`, `AgentRollup`, `FileCursor`, `FileEntry`, `UsageCache` |
| `usage/dates.rs` | Local-date bucketing and ISO8601 parsing (chrono) |
| `usage/record.rs` | One transcript line -> `ParsedRecord`; the byte prefilter |
| `usage/ingest.rs` | Dedup + applying a byte range to a `FileEntry` |
| `usage/cursor.rs` | Skip/delta/full/retire decision; complete-line splitting |
| `usage/discovery.rs` | Recursive transcript discovery |
| `usage/store.rs` | Cache load/save, atomic write, corruption and schema rebuild |
| `usage/adapter.rs` | `UsageCache` -> the existing `StatsCache` |
| `usage/legacy.rs` | One-time seed from the old `stats-cache.json` |
| `usage/worker.rs` | Single ingest worker: mutex, dirty flag, debounce, persistence cadence |
| `config.rs` | Config root resolution (setting > `CLAUDE_CONFIG_DIR` > `~/.claude`) |

**Modified:** `src-tauri/src/lib.rs` (wire the worker, repoint `get_stats`, fix the month-prefix bug), `src-tauri/src/polling.rs` (recursive watch + debounce), `src-tauri/Cargo.toml` (add `chrono`), `src/lib/api.ts` + `src/lib/types.ts` (progress event + diagnostics), `src/components/Settings.svelte` (config root override).

---

### Task 1: Core rollup types

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Create: `src-tauri/src/usage/mod.rs`
- Create: `src-tauri/src/usage/types.rs`
- Modify: `src-tauri/src/lib.rs:11` (add `mod usage;` beside `mod polling;`)
- Test: inline in `src-tauri/src/usage/types.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `TokenCounts` (with `add`, `usage_fingerprint`, `billable_total`), `DayRollup`, `SessionRollup`, `AgentRollup`, `FileCursor`, `FileEntry`, `UsageCache`, `SCHEMA_VERSION`.

- [ ] **Step 1: Add the chrono dependency**

In `src-tauri/Cargo.toml`, under `[dependencies]`, after the `dirs = "6"` line:

```toml
chrono = { version = "0.4", default-features = false, features = ["clock", "std"] }
```

`default-features = false` keeps out chrono's optional `serde`/`oldtime` baggage; `clock` provides `Local`.

Note: `CLAUDE.md` documents a pin for the unrelated `time` crate (`cargo update time@0.3.47 --precise 0.3.41`) if the build demands Rust 1.88+. Apply it only if the build actually fails.

- [ ] **Step 2: Write the failing test**

Create `src-tauri/src/usage/types.rs` containing ONLY this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_counts_add_sums_every_field() {
        let mut a = TokenCounts {
            input: 1, output: 2, cache_read: 3, cache_creation: 4,
            cache_1h: 5, cache_5m: 6, thinking: 7, web_search: 8, web_fetch: 9,
        };
        let b = a.clone();
        a.add(&b);
        assert_eq!(a.input, 2);
        assert_eq!(a.output, 4);
        assert_eq!(a.cache_read, 6);
        assert_eq!(a.cache_creation, 8);
        assert_eq!(a.cache_1h, 10);
        assert_eq!(a.cache_5m, 12);
        assert_eq!(a.thinking, 14);
        assert_eq!(a.web_search, 16);
        assert_eq!(a.web_fetch, 18);
    }

    #[test]
    fn billable_total_excludes_derived_detail_fields() {
        // thinking is already inside output; cache_1h/5m are already inside
        // cache_creation. Counting them again would double-count.
        let c = TokenCounts {
            input: 10, output: 20, cache_read: 30, cache_creation: 40,
            cache_1h: 40, cache_5m: 0, thinking: 5, web_search: 1, web_fetch: 1,
        };
        assert_eq!(c.billable_total(), 100);
    }

    #[test]
    fn usage_fingerprint_differs_when_any_field_differs() {
        let a = TokenCounts { input: 1, ..Default::default() };
        let b = TokenCounts { input: 2, ..Default::default() };
        assert_ne!(a.usage_fingerprint(), b.usage_fingerprint());
        assert_eq!(a.usage_fingerprint(), a.clone().usage_fingerprint());
    }

    #[test]
    fn usage_cache_serde_roundtrip_preserves_entries() {
        let mut cache = UsageCache::new(420);
        let mut entry = FileEntry::default();
        entry.session.session_id = "s1".to_string();
        entry.seen.insert(7u64, 99u64);
        entry.seen_tools.insert(11u64);
        entry.seen_users.insert(13u64);
        entry.days.insert("2026-09-10".to_string(), DayRollup::default());
        cache.files.insert("/tmp/a.jsonl".into(), entry);

        let json = serde_json::to_string(&cache).expect("serialize");
        let back: UsageCache = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.schema, SCHEMA_VERSION);
        assert_eq!(back.tz_offset_minutes, 420);
        let e = back.files.get(std::path::Path::new("/tmp/a.jsonl")).expect("entry");
        assert_eq!(e.session.session_id, "s1");
        assert_eq!(e.seen.get(&7), Some(&99));
        assert!(e.seen_tools.contains(&11));
        assert!(e.seen_users.contains(&13));
        assert!(e.days.contains_key("2026-09-10"));
    }
}
```

- [ ] **Step 3: Run the test to verify it fails**

```bash
cd src-tauri && cargo test usage::types 2>&1 | tail -20
```

Expected: compile errors — `cannot find type TokenCounts`, `UsageCache`, etc. A compile failure IS the failing state for a types task.

- [ ] **Step 4: Write the minimal implementation**

Prepend to `src-tauri/src/usage/types.rs` (above the test module):

```rust
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

/// Bump to force a full rebuild of the cache on next load.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    /// Detail slice of cache_creation, not additive with it.
    pub cache_1h: u64,
    /// Detail slice of cache_creation, not additive with it.
    pub cache_5m: u64,
    /// Detail slice of output, not additive with it.
    pub thinking: u64,
    pub web_search: u64,
    pub web_fetch: u64,
}

impl TokenCounts {
    pub fn add(&mut self, o: &TokenCounts) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_creation += o.cache_creation;
        self.cache_1h += o.cache_1h;
        self.cache_5m += o.cache_5m;
        self.thinking += o.thinking;
        self.web_search += o.web_search;
        self.web_fetch += o.web_fetch;
    }

    /// The four independent token classes. thinking/cache_1h/cache_5m are
    /// breakdowns of output/cache_creation and would double-count.
    pub fn billable_total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_creation
    }

    /// Cheap 64-bit fingerprint, used to detect divergent duplicate copies of
    /// the same message.id without storing full counts per id.
    pub fn usage_fingerprint(&self) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for v in [
            self.input, self.output, self.cache_read, self.cache_creation,
            self.cache_1h, self.cache_5m, self.thinking,
            self.web_search, self.web_fetch,
        ] {
            h ^= v;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DayRollup {
    /// Raw model ids. Never family buckets.
    pub by_model: HashMap<String, TokenCounts>,
    pub message_count: u64,
    pub tool_call_count: u64,
    pub session_ids: HashSet<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SessionRollup {
    pub session_id: String,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub first_ts: i64,
    /// Last record of ANY type. Drives idle age in Phase 2.
    pub last_ts: i64,
    /// Last record that carried usage. Diagnostic only.
    pub last_usage_ts: i64,
    pub message_count: u64,
    pub by_model: HashMap<String, TokenCounts>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AgentRollup {
    pub agent_id: String,
    /// Absent on ~1,255 observed subagent records.
    pub agent_type: Option<String>,
    pub last_ts: i64,
    pub tokens: TokenCounts,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct FileCursor {
    pub offset: u64,
    pub size: u64,
    pub mtime_ms: u64,
    pub inode: u64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub cursor: FileCursor,
    /// hash(message.id) -> usage fingerprint. Token dedup. Cleared when retired.
    pub seen: HashMap<u64, u64>,
    /// hash(tool_use block id). Tool-call dedup: each transcript line carries
    /// exactly ONE content block, and blocks repeat via whole-message
    /// re-emission, so message.id cannot serve as the key here.
    /// Observed globally: 68,492 raw blocks vs 54,229 distinct ids.
    pub seen_tools: HashSet<u64>,
    /// hash(user record uuid). Message-count dedup: user records also
    /// duplicate (72,443 raw vs 57,122 distinct uuid).
    pub seen_users: HashSet<u64>,
    /// Source file pruned upstream; days/session retained, seen dropped.
    pub retired: bool,
    /// Local date key -> rollup.
    pub days: BTreeMap<String, DayRollup>,
    pub session: SessionRollup,
    pub agents: HashMap<String, AgentRollup>,
    pub malformed_lines: u64,
    /// Duplicate copies of one message.id whose usage disagreed. Diagnostic.
    pub divergent_copies: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageCache {
    pub schema: u32,
    /// Timezone offset in effect when day keys were written. A change forces
    /// a rebuild rather than yielding mixed-timezone days.
    pub tz_offset_minutes: i32,
    pub files: HashMap<PathBuf, FileEntry>,
    pub legacy: Option<crate::usage::legacy::LegacyRollup>,
}

impl UsageCache {
    pub fn new(tz_offset_minutes: i32) -> Self {
        UsageCache {
            schema: SCHEMA_VERSION,
            tz_offset_minutes,
            files: HashMap::new(),
            legacy: None,
        }
    }
}
```

Create `src-tauri/src/usage/mod.rs`:

```rust
pub mod types;
```

Add `mod usage;` to `src-tauri/src/lib.rs` immediately after the existing `mod polling;` line.

Because `UsageCache::legacy` refers to a type built in Task 10, add a placeholder now so this task compiles standalone. Create `src-tauri/src/usage/legacy.rs`:

```rust
use serde::{Deserialize, Serialize};

/// Populated in Task 10. Present now so UsageCache compiles.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct LegacyRollup {}
```

and add `pub mod legacy;` to `usage/mod.rs`.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cd src-tauri && cargo test usage:: 2>&1 | tail -15
```

Expected: `test result: ok. 4 passed`.

- [ ] **Step 6: Verify nothing else regressed**

```bash
cd src-tauri && cargo test 2>&1 | tail -5
```

Expected: all pre-existing tests still pass.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/usage src-tauri/src/lib.rs
git commit -m "feat(usage): add core rollup types and cache schema"
```

---

### Task 2: Local-date bucketing

**Files:**
- Create: `src-tauri/src/usage/dates.rs`
- Modify: `src-tauri/src/usage/mod.rs`
- Test: inline in `src-tauri/src/usage/dates.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `parse_iso8601_ms(&str) -> Option<i64>`, `local_date_key(i64) -> String`, `local_month_prefix(i64) -> String`, `now_ms() -> i64`, `current_tz_offset_minutes() -> i32`, `local_hour(i64) -> u32`.

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/usage/dates.rs` with ONLY this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_transcript_timestamp_to_epoch_ms() {
        // Real timestamp shape from an assistant record.
        let ms = parse_iso8601_ms("2026-09-10T08:32:59.076Z").expect("parse");
        assert_eq!(ms, 1789029179076);
    }

    #[test]
    fn parses_timestamp_without_fractional_seconds() {
        let ms = parse_iso8601_ms("2026-09-10T08:32:59Z").expect("parse");
        assert_eq!(ms, 1789029179000);
    }

    #[test]
    fn rejects_garbage_timestamp() {
        assert!(parse_iso8601_ms("not-a-date").is_none());
        assert!(parse_iso8601_ms("").is_none());
    }

    #[test]
    fn utc_timestamp_near_midnight_lands_in_correct_local_day() {
        // 2026-09-10T20:00:00Z. In UTC+07 that is 2026-09-11 03:00 local, so
        // the day key must differ from the UTC date. Uses an explicit offset
        // so the test does not depend on the machine's timezone.
        let ms = parse_iso8601_ms("2026-09-10T20:00:00Z").expect("parse");
        assert_eq!(date_key_with_offset(ms, 420), "2026-09-11");
        assert_eq!(date_key_with_offset(ms, 0), "2026-09-10");
        // UTC-07 pushes it earlier still, but same date here.
        assert_eq!(date_key_with_offset(ms, -420), "2026-09-10");
    }

    #[test]
    fn month_prefix_respects_offset_at_month_boundary() {
        // 2026-08-31T20:00:00Z is 2026-09-01 local at UTC+07: month rolls over.
        let ms = parse_iso8601_ms("2026-08-31T20:00:00Z").expect("parse");
        assert_eq!(month_prefix_with_offset(ms, 420), "2026-09");
        assert_eq!(month_prefix_with_offset(ms, 0), "2026-08");
    }

    #[test]
    fn local_hour_respects_offset() {
        let ms = parse_iso8601_ms("2026-09-10T20:00:00Z").expect("parse");
        assert_eq!(hour_with_offset(ms, 420), 3);
        assert_eq!(hour_with_offset(ms, 0), 20);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd src-tauri && cargo test usage::dates 2>&1 | tail -20
```

Expected: compile errors, `cannot find function parse_iso8601_ms`.

- [ ] **Step 3: Write the minimal implementation**

Prepend to `src-tauri/src/usage/dates.rs`:

```rust
use chrono::{DateTime, Datelike, FixedOffset, Local, Offset, TimeZone, Timelike, Utc};

/// Parse a transcript `timestamp` (ISO8601, always UTC with a Z suffix in
/// observed data) into epoch milliseconds.
pub fn parse_iso8601_ms(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// The machine's current UTC offset, in minutes east of UTC.
pub fn current_tz_offset_minutes() -> i32 {
    Local::now().offset().fix().local_minus_utc() / 60
}

fn shifted(ts_ms: i64, offset_minutes: i32) -> DateTime<FixedOffset> {
    let offset = FixedOffset::east_opt(offset_minutes * 60)
        .unwrap_or_else(|| FixedOffset::east_opt(0).expect("utc offset"));
    offset.timestamp_millis_opt(ts_ms).single().unwrap_or_else(|| {
        FixedOffset::east_opt(0)
            .expect("utc offset")
            .timestamp_millis_opt(0)
            .single()
            .expect("epoch")
    })
}

/// "YYYY-MM-DD" in the given offset. Exposed for deterministic tests.
pub fn date_key_with_offset(ts_ms: i64, offset_minutes: i32) -> String {
    let d = shifted(ts_ms, offset_minutes);
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

/// "YYYY-MM" in the given offset.
pub fn month_prefix_with_offset(ts_ms: i64, offset_minutes: i32) -> String {
    let d = shifted(ts_ms, offset_minutes);
    format!("{:04}-{:02}", d.year(), d.month())
}

pub fn hour_with_offset(ts_ms: i64, offset_minutes: i32) -> u32 {
    shifted(ts_ms, offset_minutes).hour()
}

/// Day key in the machine's current local timezone.
pub fn local_date_key(ts_ms: i64) -> String {
    date_key_with_offset(ts_ms, current_tz_offset_minutes())
}

/// Month prefix in the machine's current local timezone.
pub fn local_month_prefix(ts_ms: i64) -> String {
    month_prefix_with_offset(ts_ms, current_tz_offset_minutes())
}

pub fn local_hour(ts_ms: i64) -> u32 {
    hour_with_offset(ts_ms, current_tz_offset_minutes())
}
```

Add `pub mod dates;` to `src-tauri/src/usage/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd src-tauri && cargo test usage::dates 2>&1 | tail -15
```

Expected: `test result: ok. 6 passed`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/usage/dates.rs src-tauri/src/usage/mod.rs
git commit -m "feat(usage): add local-date bucketing with explicit-offset tests"
```

---

### Task 3: Transcript record parsing

**Files:**
- Create: `src-tauri/src/usage/record.rs`
- Modify: `src-tauri/src/usage/mod.rs`
- Test: inline in `src-tauri/src/usage/record.rs`

**Interfaces:**
- Consumes: `TokenCounts` (Task 1), `parse_iso8601_ms` (Task 2).
- Produces: `RecordKind`, `ParsedRecord`, `ParseOutcome`, `parse_line(&str) -> ParseOutcome`.

**Deviation from spec 5.2, deliberate:** the spec called for a byte prefilter (`"type":"assistant"`) before JSON parsing. That turns out inapplicable: activity metrics need `user` records for message counts and records of *every* type for `last_ts` (spec 5.3, 6.1), so each line must be parsed regardless. Instead this task extracts everything downstream needs — usage, the `uuid`, and the `tool_use` block ids — in **one** parse per line, rather than the two the prefilter design would have implied.

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/usage/record.rs` with ONLY this test module. The fixtures are trimmed but structurally faithful to real records:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const ASSISTANT: &str = r#"{"type":"assistant","timestamp":"2026-09-10T08:32:59.076Z","sessionId":"s-1","cwd":"/Users/me/proj","gitBranch":"main","requestId":"req_1","apiBlockIndex":0,"message":{"id":"msg_1","model":"claude-opus-5","role":"assistant","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":2,"output_tokens":894,"cache_read_input_tokens":48669,"cache_creation_input_tokens":2722,"cache_creation":{"ephemeral_1h_input_tokens":2722,"ephemeral_5m_input_tokens":0},"output_tokens_details":{"thinking_tokens":300},"server_tool_use":{"web_search_requests":1,"web_fetch_requests":2}}}}"#;

    #[test]
    fn parses_assistant_usage_into_token_counts() {
        let rec = match parse_line(ASSISTANT) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        assert_eq!(rec.kind, RecordKind::Assistant);
        assert_eq!(rec.session_id, "s-1");
        assert_eq!(rec.message_id.as_deref(), Some("msg_1"));
        assert_eq!(rec.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(rec.cwd.as_deref(), Some("/Users/me/proj"));
        assert_eq!(rec.git_branch.as_deref(), Some("main"));
        assert_eq!(rec.ts_ms, 1789029179076);

        let u = rec.usage.expect("usage");
        assert_eq!(u.input, 2);
        assert_eq!(u.output, 894);
        assert_eq!(u.cache_read, 48669);
        assert_eq!(u.cache_creation, 2722);
        assert_eq!(u.cache_1h, 2722);
        assert_eq!(u.cache_5m, 0);
        assert_eq!(u.thinking, 300);
        assert_eq!(u.web_search, 1);
        assert_eq!(u.web_fetch, 2);
    }

    #[test]
    fn collects_tool_use_block_ids_for_dedup() {
        // Real tool_use blocks always carry an id (0 missing in 68,492
        // observed blocks); the id is the only safe tool-call dedup key.
        let line = r#"{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","sessionId":"s","message":{"id":"m","model":"claude-opus-5","content":[{"type":"text","text":"a"},{"type":"tool_use","id":"toolu_a","name":"Bash"},{"type":"tool_use","id":"toolu_b","name":"Read"}],"usage":{"input_tokens":1,"output_tokens":1}}}"#;
        let rec = match parse_line(line) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        assert_eq!(rec.tool_use_ids, vec!["toolu_a".to_string(), "toolu_b".to_string()]);
    }

    #[test]
    fn captures_user_record_uuid_for_dedup() {
        let line = r#"{"type":"user","uuid":"u-1","timestamp":"2026-09-10T08:30:00Z","sessionId":"s","message":{"role":"user","content":"hello"}}"#;
        let rec = match parse_line(line) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        assert_eq!(rec.uuid.as_deref(), Some("u-1"));
    }

    #[test]
    fn excludes_synthetic_model_usage_but_keeps_the_record() {
        let line = r#"{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","sessionId":"s","isApiErrorMessage":true,"message":{"id":"m","model":"<synthetic>","content":[],"usage":{"input_tokens":0,"output_tokens":0}}}"#;
        let rec = match parse_line(line) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        assert!(rec.usage.is_none(), "synthetic usage must not be counted");
        assert_eq!(rec.model.as_deref(), Some("<synthetic>"));
    }

    #[test]
    fn parses_subagent_attribution_fields() {
        let line = r#"{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","sessionId":"parent-s","agentId":"a05e8f2db40a521f0","attributionAgent":"general-purpose","isSidechain":true,"message":{"id":"m","model":"claude-sonnet-5","content":[],"usage":{"input_tokens":1,"output_tokens":2}}}"#;
        let rec = match parse_line(line) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        // Subagent records share the PARENT's sessionId.
        assert_eq!(rec.session_id, "parent-s");
        assert_eq!(rec.agent_id.as_deref(), Some("a05e8f2db40a521f0"));
        assert_eq!(rec.agent_type.as_deref(), Some("general-purpose"));
    }

    #[test]
    fn subagent_without_attribution_agent_yields_none() {
        let line = r#"{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","sessionId":"p","agentId":"a1","message":{"id":"m","model":"claude-sonnet-5","content":[],"usage":{"input_tokens":1,"output_tokens":1}}}"#;
        let rec = match parse_line(line) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        assert_eq!(rec.agent_id.as_deref(), Some("a1"));
        assert_eq!(rec.agent_type, None);
    }

    #[test]
    fn assistant_record_missing_usage_parses_with_no_usage() {
        let line = r#"{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","sessionId":"s","message":{"id":"m","model":"claude-opus-5","content":[]}}"#;
        match parse_line(line) {
            ParseOutcome::Record(r) => assert!(r.usage.is_none()),
            other => panic!("expected Record, got {:?}", other),
        }
    }

    #[test]
    fn parses_user_record_for_message_counting() {
        let line = r#"{"type":"user","timestamp":"2026-09-10T08:30:00Z","sessionId":"s","message":{"role":"user","content":"hello"}}"#;
        let rec = match parse_line(line) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        assert_eq!(rec.kind, RecordKind::User);
        assert!(rec.usage.is_none());
    }

    #[test]
    fn other_record_types_parse_as_other_for_activity_timestamps() {
        // These still matter: last_ts must reflect ANY record so a session
        // mid-Bash-call is not reported as idle.
        let line = r#"{"type":"file-history-snapshot","timestamp":"2026-09-10T08:40:00Z","sessionId":"s"}"#;
        let rec = match parse_line(line) {
            ParseOutcome::Record(r) => r,
            other => panic!("expected Record, got {:?}", other),
        };
        assert_eq!(rec.kind, RecordKind::Other);
    }

    #[test]
    fn malformed_json_is_reported_not_panicked() {
        assert!(matches!(parse_line("{not json"), ParseOutcome::Malformed));
        assert!(matches!(parse_line(""), ParseOutcome::Skipped));
        assert!(matches!(parse_line("   "), ParseOutcome::Skipped));
    }

    #[test]
    fn record_without_timestamp_or_session_is_skipped_not_fatal() {
        let no_ts = r#"{"type":"assistant","sessionId":"s","message":{"id":"m"}}"#;
        assert!(matches!(parse_line(no_ts), ParseOutcome::Skipped));
        let no_session = r#"{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","message":{"id":"m"}}"#;
        assert!(matches!(parse_line(no_session), ParseOutcome::Skipped));
    }

}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd src-tauri && cargo test usage::record 2>&1 | tail -20
```

Expected: compile errors, `cannot find function parse_line`.

- [ ] **Step 3: Write the minimal implementation**

Prepend to `src-tauri/src/usage/record.rs`:

```rust
use crate::usage::dates::parse_iso8601_ms;
use crate::usage::types::TokenCounts;
use serde::Deserialize;

/// Model id that carries no API cost. Observed 52 times, always with
/// isApiErrorMessage:true and all-zero usage.
const SYNTHETIC_MODEL: &str = "<synthetic>";

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RecordKind {
    Assistant,
    User,
    Other,
}

#[derive(Debug, Clone)]
pub struct ParsedRecord {
    pub kind: RecordKind,
    pub ts_ms: i64,
    pub session_id: String,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    pub message_id: Option<String>,
    pub model: Option<String>,
    /// Record uuid. Dedup key for user messages (72,443 raw vs 57,122 distinct).
    pub uuid: Option<String>,
    /// ids of tool_use blocks on this line. Dedup key for tool calls: each
    /// line carries exactly ONE content block, so message.id cannot serve.
    pub tool_use_ids: Vec<String>,
    /// None for user/other records, for assistant records with no usage
    /// block, and for `<synthetic>` models.
    pub usage: Option<TokenCounts>,
}

#[derive(Debug)]
pub enum ParseOutcome {
    Record(ParsedRecord),
    /// Blank line, or a record we deliberately ignore. Not an error.
    Skipped,
    /// Unparseable. Counted and surfaced as a diagnostic.
    Malformed,
}

// --- Wire types: every field optional so a missing key skips, never fails ---

#[derive(Deserialize)]
struct RawLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "gitBranch")]
    git_branch: Option<String>,
    #[serde(rename = "agentId")]
    agent_id: Option<String>,
    #[serde(rename = "attributionAgent")]
    attribution_agent: Option<String>,
    uuid: Option<String>,
    message: Option<RawMessage>,
}

#[derive(Deserialize)]
struct RawMessage {
    id: Option<String>,
    model: Option<String>,
    #[serde(default)]
    content: Vec<RawBlock>,
    usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct RawBlock {
    #[serde(rename = "type")]
    kind: Option<String>,
    /// Present on every observed tool_use block.
    id: Option<String>,
}

#[derive(Deserialize)]
struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    cache_creation: Option<RawCacheCreation>,
    output_tokens_details: Option<RawOutputDetails>,
    server_tool_use: Option<RawServerToolUse>,
}

#[derive(Deserialize)]
struct RawCacheCreation {
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
}

#[derive(Deserialize)]
struct RawOutputDetails {
    #[serde(default)]
    thinking_tokens: u64,
}

#[derive(Deserialize)]
struct RawServerToolUse {
    #[serde(default)]
    web_search_requests: u64,
    #[serde(default)]
    web_fetch_requests: u64,
}

impl RawUsage {
    fn to_counts(&self) -> TokenCounts {
        TokenCounts {
            input: self.input_tokens,
            output: self.output_tokens,
            cache_read: self.cache_read_input_tokens,
            cache_creation: self.cache_creation_input_tokens,
            cache_1h: self
                .cache_creation
                .as_ref()
                .map(|c| c.ephemeral_1h_input_tokens)
                .unwrap_or(0),
            cache_5m: self
                .cache_creation
                .as_ref()
                .map(|c| c.ephemeral_5m_input_tokens)
                .unwrap_or(0),
            thinking: self
                .output_tokens_details
                .as_ref()
                .map(|d| d.thinking_tokens)
                .unwrap_or(0),
            web_search: self
                .server_tool_use
                .as_ref()
                .map(|s| s.web_search_requests)
                .unwrap_or(0),
            web_fetch: self
                .server_tool_use
                .as_ref()
                .map(|s| s.web_fetch_requests)
                .unwrap_or(0),
        }
    }
}

pub fn parse_line(line: &str) -> ParseOutcome {
    if line.trim().is_empty() {
        return ParseOutcome::Skipped;
    }
    let raw: RawLine = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(_) => return ParseOutcome::Malformed,
    };

    let ts_ms = match raw.timestamp.as_deref().and_then(parse_iso8601_ms) {
        Some(v) => v,
        None => return ParseOutcome::Skipped,
    };
    let session_id = match raw.session_id {
        Some(s) => s,
        None => return ParseOutcome::Skipped,
    };

    let kind = match raw.kind.as_deref() {
        Some("assistant") => RecordKind::Assistant,
        Some("user") => RecordKind::User,
        _ => RecordKind::Other,
    };

    let msg = raw.message;
    let model = msg.as_ref().and_then(|m| m.model.clone());
    let message_id = msg.as_ref().and_then(|m| m.id.clone());

    let tool_use_ids: Vec<String> = msg
        .as_ref()
        .map(|m| {
            m.content
                .iter()
                .filter(|b| b.kind.as_deref() == Some("tool_use"))
                .filter_map(|b| b.id.clone())
                .collect()
        })
        .unwrap_or_default();

    let usage = if kind == RecordKind::Assistant && model.as_deref() != Some(SYNTHETIC_MODEL) {
        msg.as_ref().and_then(|m| m.usage.as_ref()).map(|u| u.to_counts())
    } else {
        None
    };

    ParseOutcome::Record(ParsedRecord {
        kind,
        ts_ms,
        session_id,
        cwd: raw.cwd,
        git_branch: raw.git_branch,
        agent_id: raw.agent_id,
        agent_type: raw.attribution_agent,
        message_id,
        model,
        uuid: raw.uuid,
        tool_use_ids,
        usage,
    })
}
```

Add `pub mod record;` to `src-tauri/src/usage/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd src-tauri && cargo test usage::record 2>&1 | tail -15
```

Expected: `test result: ok. 11 passed`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/usage/record.rs src-tauri/src/usage/mod.rs
git commit -m "feat(usage): parse transcript records with tolerant optional fields"
```

---

### Task 4: Deduplicated ingest — the correctness core

This is the task the whole plan exists for. Get it wrong and every number the app shows is inflated.

**Files:**
- Create: `src-tauri/src/usage/ingest.rs`
- Modify: `src-tauri/src/usage/mod.rs`
- Test: inline in `src-tauri/src/usage/ingest.rs`

**Interfaces:**
- Consumes: `FileEntry`, `TokenCounts`, `DayRollup` (Task 1); `date_key_with_offset` (Task 2); `parse_line`, `ParseOutcome`, `RecordKind` (Task 3).
- Produces: `key_hash(&str) -> u64`, `IngestStats { records: u64, malformed: u64, deduped: u64 }`, `ingest_text(&mut FileEntry, &str, i32) -> IngestStats`.

**Design note for the implementer:** an earlier review suggested taking the max per field across duplicate copies of a message. That would require storing full `TokenCounts` per message id (~45,700 entries), several times the memory of a fingerprint. Since copies are identical in observed data, this uses **first-wins** plus a cheap fingerprint comparison that increments `divergent_copies` when copies disagree. That surfaces divergence as a visible diagnostic instead of paying for it up front. If `divergent_copies` turns out non-zero in the field, revisit with evidence.

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/usage/ingest.rs` with ONLY this test module:

```rust
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
        // The 4.70x inflation regression test. Three block-lines, one message.
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
    fn divergent_duplicate_is_counted_as_a_diagnostic_not_added() {
        let text = format!(
            "{}\n{}\n",
            assistant_line("msg_1", "text", 100),
            assistant_line("msg_1", "text", 999), // same id, different usage
        );
        let mut entry = FileEntry::default();
        ingest_text(&mut entry, &text, 0);
        assert_eq!(entry.divergent_copies, 1);
        assert_eq!(entry.days["2026-09-10"].by_model["claude-opus-5"].output, 100);
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
    fn malformed_lines_are_counted_and_do_not_abort_ingest() {
        let text = format!(
            "{}\n{not json\n{}\n",
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
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd src-tauri && cargo test usage::ingest 2>&1 | tail -20
```

Expected: compile errors, `cannot find function ingest_text`.

- [ ] **Step 3: Write the minimal implementation**

Prepend to `src-tauri/src/usage/ingest.rs`:

```rust
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

/// Apply every complete line in `text` to `entry`. Callers must pass only
/// complete lines (see cursor::split_complete_lines).
pub fn ingest_text(entry: &mut FileEntry, text: &str, tz_offset_minutes: i32) -> IngestStats {
    let mut stats = IngestStats::default();

    for line in text.lines() {
        let rec = match parse_line(line) {
            ParseOutcome::Record(r) => r,
            ParseOutcome::Skipped => continue,
            ParseOutcome::Malformed => {
                stats.malformed += 1;
                entry.malformed_lines += 1;
                continue;
            }
        };
        stats.records += 1;
        apply_record(entry, &rec, tz_offset_minutes, &mut stats);
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

    // --- User messages: dedupe on uuid ---
    if rec.kind == RecordKind::User {
        let counts = match &rec.uuid {
            Some(u) => entry.seen_users.insert(key_hash(u)),
            // No uuid: cannot dedupe, count it rather than silently drop it.
            None => true,
        };
        if counts {
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

    if let Some(&existing_fp) = entry.seen.get(&id_hash) {
        stats.deduped += 1;
        let fp = rec
            .usage
            .as_ref()
            .map(|u| u.usage_fingerprint())
            .unwrap_or(0);
        if fp != 0 && fp != existing_fp {
            entry.divergent_copies += 1;
        }
        return;
    }

    let fp = rec
        .usage
        .as_ref()
        .map(|u| u.usage_fingerprint())
        .unwrap_or(0);
    entry.seen.insert(id_hash, fp);

    day.message_count += 1;
    entry.session.message_count += 1;

    let Some(usage) = rec.usage.as_ref() else {
        return;
    };
    let model = rec.model.clone().unwrap_or_else(|| "unknown".to_string());

    day.by_model.entry(model.clone()).or_default().add(usage);
    entry
        .session
        .by_model
        .entry(model)
        .or_default()
        .add(usage);

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
        agent.tokens.add(usage);
    }
}
```

Add `pub mod ingest;` to `src-tauri/src/usage/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd src-tauri && cargo test usage::ingest 2>&1 | tail -20
```

Expected: `test result: ok. 12 passed`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/usage/ingest.rs src-tauri/src/usage/mod.rs
git commit -m "feat(usage): deduplicated ingest with three independent keys

Tokens dedupe on message.id, tool calls on tool_use block id, user
messages on uuid. Each transcript line carries one content block and
messages are re-emitted, so a single key cannot serve all three.
Regression test pins the 4.70x token inflation at 1x."
```

---

### Task 5: Cursor semantics and complete-line splitting

**Files:**
- Create: `src-tauri/src/usage/cursor.rs`
- Modify: `src-tauri/src/usage/mod.rs`
- Test: inline in `src-tauri/src/usage/cursor.rs`

**Interfaces:**
- Consumes: `FileCursor` (Task 1).
- Produces: `FileMeta { size, mtime_ms, inode }`, `ScanAction`, `decide(Option<&FileCursor>, Option<&FileMeta>) -> ScanAction`, `split_complete_lines(&str) -> (&str, usize)`, `read_meta(&Path) -> Option<FileMeta>`, `read_range(&Path, u64) -> Result<(String, u64), String>`.

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/usage/cursor.rs` with ONLY this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::types::FileCursor;
    use std::io::Write;

    fn cursor(offset: u64, size: u64, mtime_ms: u64, inode: u64) -> FileCursor {
        FileCursor { offset, size, mtime_ms, inode }
    }
    fn meta(size: u64, mtime_ms: u64, inode: u64) -> FileMeta {
        FileMeta { size, mtime_ms, inode }
    }

    #[test]
    fn unchanged_file_is_skipped() {
        let c = cursor(500, 500, 1000, 42);
        assert_eq!(decide(Some(&c), Some(&meta(500, 1000, 42))), ScanAction::Skip);
    }

    #[test]
    fn grown_file_reads_only_the_delta() {
        let c = cursor(500, 500, 1000, 42);
        assert_eq!(
            decide(Some(&c), Some(&meta(900, 2000, 42))),
            ScanAction::Delta { from: 500 }
        );
    }

    #[test]
    fn truncated_file_triggers_full_reingest() {
        let c = cursor(500, 500, 1000, 42);
        assert_eq!(decide(Some(&c), Some(&meta(200, 2000, 42))), ScanAction::Full);
    }

    #[test]
    fn replaced_inode_triggers_full_reingest() {
        let c = cursor(500, 500, 1000, 42);
        assert_eq!(decide(Some(&c), Some(&meta(900, 2000, 99))), ScanAction::Full);
    }

    #[test]
    fn same_size_but_newer_mtime_triggers_full_reingest() {
        // Rewritten in place at identical length: content may differ, so the
        // delta would be wrong. Re-ingest rather than trust the size.
        let c = cursor(500, 500, 1000, 42);
        assert_eq!(decide(Some(&c), Some(&meta(500, 5000, 42))), ScanAction::Full);
    }

    #[test]
    fn unknown_file_is_ingested_whole() {
        assert_eq!(decide(None, Some(&meta(900, 2000, 42))), ScanAction::Full);
    }

    #[test]
    fn vanished_file_is_retired() {
        let c = cursor(500, 500, 1000, 42);
        assert_eq!(decide(Some(&c), None), ScanAction::Retire);
        // Never seen and not present: nothing to do.
        assert_eq!(decide(None, None), ScanAction::Skip);
    }

    #[test]
    fn split_complete_lines_withholds_a_partial_trailing_record() {
        let buf = "{\"a\":1}\n{\"b\":2}\n{\"c\":pa";
        let (complete, consumed) = split_complete_lines(buf);
        assert_eq!(complete, "{\"a\":1}\n{\"b\":2}\n");
        assert_eq!(consumed, 16);
    }

    #[test]
    fn split_complete_lines_handles_exact_newline_ending() {
        let buf = "{\"a\":1}\n";
        let (complete, consumed) = split_complete_lines(buf);
        assert_eq!(complete, "{\"a\":1}\n");
        assert_eq!(consumed, 8);
    }

    #[test]
    fn split_complete_lines_withholds_everything_when_no_newline_yet() {
        let buf = "{\"a\":partial";
        let (complete, consumed) = split_complete_lines(buf);
        assert_eq!(complete, "");
        assert_eq!(consumed, 0);
    }

    #[test]
    fn read_range_returns_only_bytes_after_offset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.jsonl");
        let mut f = std::fs::File::create(&path).expect("create");
        writeln!(f, "{{\"a\":1}}").expect("write");
        writeln!(f, "{{\"b\":2}}").expect("write");
        f.flush().expect("flush");

        let (text, consumed) = read_range(&path, 8).expect("read");
        assert_eq!(text, "{\"b\":2}\n");
        assert_eq!(consumed, 8);
    }

    #[test]
    fn read_range_withholds_a_half_written_trailing_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.jsonl");
        let mut f = std::fs::File::create(&path).expect("create");
        write!(f, "{{\"a\":1}}\n{{\"b\":par").expect("write");
        f.flush().expect("flush");

        let (text, consumed) = read_range(&path, 0).expect("read");
        assert_eq!(text, "{\"a\":1}\n");
        assert_eq!(consumed, 8, "offset must not advance past the partial line");
    }

    #[test]
    fn read_meta_reports_size_and_inode_for_a_real_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.jsonl");
        std::fs::write(&path, b"hello\n").expect("write");
        let m = read_meta(&path).expect("meta");
        assert_eq!(m.size, 6);
        assert!(m.inode > 0);
        assert!(read_meta(&dir.path().join("missing.jsonl")).is_none());
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd src-tauri && cargo test usage::cursor 2>&1 | tail -20
```

Expected: compile errors, `cannot find type FileMeta`.

- [ ] **Step 3: Write the minimal implementation**

Prepend to `src-tauri/src/usage/cursor.rs`:

```rust
use crate::usage::types::FileCursor;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMeta {
    pub size: u64,
    pub mtime_ms: u64,
    pub inode: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanAction {
    /// Nothing changed, or nothing to do.
    Skip,
    /// Read from this byte offset onward.
    Delta { from: u64 },
    /// Discard any stored contribution and ingest the file whole.
    Full,
    /// Source file is gone: keep days, drop dedup sets.
    Retire,
}

pub fn decide(cursor: Option<&FileCursor>, meta: Option<&FileMeta>) -> ScanAction {
    match (cursor, meta) {
        (None, None) => ScanAction::Skip,
        (Some(_), None) => ScanAction::Retire,
        (None, Some(_)) => ScanAction::Full,
        (Some(c), Some(m)) => {
            if m.inode != c.inode {
                return ScanAction::Full;
            }
            if m.size < c.offset {
                return ScanAction::Full;
            }
            if m.size == c.size && m.mtime_ms == c.mtime_ms {
                return ScanAction::Skip;
            }
            if m.size == c.size {
                // Rewritten in place at the same length: contents may differ.
                return ScanAction::Full;
            }
            ScanAction::Delta { from: c.offset }
        }
    }
}

/// Split a buffer into the portion ending at its last newline and the byte
/// count that portion occupies. A trailing partial record is withheld so it is
/// re-read intact on the next pass.
pub fn split_complete_lines(buf: &str) -> (&str, usize) {
    match buf.rfind('\n') {
        Some(idx) => {
            let end = idx + 1;
            (&buf[..end], end)
        }
        None => ("", 0),
    }
}

pub fn read_meta(path: &Path) -> Option<FileMeta> {
    use std::os::unix::fs::MetadataExt;
    let md = std::fs::metadata(path).ok()?;
    let mtime_ms = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    Some(FileMeta {
        size: md.len(),
        mtime_ms,
        inode: md.ino(),
    })
}

/// Read from `offset` to EOF, returning complete lines only plus the number of
/// bytes consumed. Streams via BufReader; never loads a whole 95 MB file.
pub fn read_range(path: &Path, offset: u64) -> Result<(String, u64), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {:?}: {}", path, e))?;
    let mut reader = BufReader::new(file);
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|e| format!("seek {:?}: {}", path, e))?;

    let mut buf = Vec::new();
    reader
        .read_to_end(&mut buf)
        .map_err(|e| format!("read {:?}: {}", path, e))?;

    // Transcripts are UTF-8; a lossy conversion keeps a corrupt byte from
    // aborting the whole pass. Byte counts stay correct because we measure
    // against the raw buffer via the newline position.
    let last_newline = buf.iter().rposition(|b| *b == b'\n');
    let consumed = match last_newline {
        Some(i) => i as u64 + 1,
        None => 0,
    };
    let text = String::from_utf8_lossy(&buf[..consumed as usize]).into_owned();
    Ok((text, consumed))
}
```

Add `pub mod cursor;` to `src-tauri/src/usage/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd src-tauri && cargo test usage::cursor 2>&1 | tail -20
```

Expected: `test result: ok. 13 passed`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/usage/cursor.rs src-tauri/src/usage/mod.rs
git commit -m "feat(usage): cursor scan decisions and partial-line safe reads"
```

---

### Task 6: Recursive transcript discovery

**Files:**
- Create: `src-tauri/src/usage/discovery.rs`
- Modify: `src-tauri/src/usage/mod.rs`
- Test: inline in `src-tauri/src/usage/discovery.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `discover_transcripts(&Path) -> Vec<PathBuf>`.

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/usage/discovery.rs` with ONLY this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &std::path::Path) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, b"{}\n").expect("write");
    }

    #[test]
    fn finds_transcripts_at_every_observed_nesting_depth() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        // The four real shapes, per spec section 2.5.
        touch(&root.join("-proj-a/11111111-1111-1111-1111-111111111111.jsonl"));
        touch(&root.join("-proj-a/11111111-1111-1111-1111-111111111111/subagents/agent-abc.jsonl"));
        touch(&root.join("-proj-b/22222222-2222-2222-2222-222222222222/subagents/workflows/wf_x-1bf/agent-def.jsonl"));
        touch(&root.join("-proj-b/22222222-2222-2222-2222-222222222222/subagents/workflows/wf_x-1bf/journal.jsonl"));
        // Non-transcript files must be ignored.
        touch(&root.join("-proj-a/notes.md"));
        touch(&root.join("-proj-a/memory/scratch.json"));

        let found = discover_transcripts(root);
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().expect("name").to_string_lossy().to_string())
            .collect();

        assert_eq!(found.len(), 4, "found: {:?}", names);
        assert!(names.contains(&"agent-def.jsonl".to_string()), "workflows nesting missed");
        assert!(names.contains(&"journal.jsonl".to_string()), "journal.jsonl missed");
        assert!(!names.contains(&"notes.md".to_string()));
        assert!(!names.contains(&"scratch.json".to_string()));
    }

    #[test]
    fn returns_sorted_paths_for_deterministic_ingest_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        touch(&root.join("z/b.jsonl"));
        touch(&root.join("a/a.jsonl"));
        let found = discover_transcripts(root);
        let mut sorted = found.clone();
        sorted.sort();
        assert_eq!(found, sorted);
    }

    #[test]
    fn missing_root_yields_empty_not_panic() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(discover_transcripts(&dir.path().join("nope")).is_empty());
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd src-tauri && cargo test usage::discovery 2>&1 | tail -20
```

Expected: compile error, `cannot find function discover_transcripts`.

- [ ] **Step 3: Write the minimal implementation**

Prepend to `src-tauri/src/usage/discovery.rs`:

```rust
use std::path::PathBuf;

/// Every `*.jsonl` under `projects_root`, at any depth. Deliberately NOT
/// pattern-matched on path shape: subagent transcripts nest under
/// `<session>/subagents/` and again under `subagents/workflows/wf_*/`, and
/// some are named `journal.jsonl` rather than `agent-*.jsonl`. Attribution
/// comes from record fields, never from the path.
pub fn discover_transcripts(projects_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(projects_root, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return, // unreadable or missing: skip, never fail the pass
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            walk(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}
```

Add `pub mod discovery;` to `src-tauri/src/usage/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd src-tauri && cargo test usage::discovery 2>&1 | tail -15
```

Expected: `test result: ok. 3 passed`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/usage/discovery.rs src-tauri/src/usage/mod.rs
git commit -m "feat(usage): recursive transcript discovery covering workflow nesting"
```

---

### Task 7: Cache persistence and durability

**Files:**
- Create: `src-tauri/src/usage/store.rs`
- Modify: `src-tauri/src/usage/mod.rs`
- Test: inline in `src-tauri/src/usage/store.rs`

**Interfaces:**
- Consumes: `UsageCache`, `SCHEMA_VERSION`, `FileEntry` (Task 1).
- Produces: `LoadOutcome`, `load(&Path, i32) -> LoadOutcome`, `save_atomic(&Path, &UsageCache) -> Result<(), String>`.

**Why this matters:** the dedup sets are the cache's bulk — roughly 157,000 entries across the three keys for a 30-day window (~4 MB of JSON). A half-written cache must never replace a good one, and a corrupt one must self-heal rather than wedging the app.

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/usage/store.rs` with ONLY this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::types::{FileEntry, UsageCache, SCHEMA_VERSION};

    fn cache_with_one_file(tz: i32) -> UsageCache {
        let mut c = UsageCache::new(tz);
        let mut e = FileEntry::default();
        e.session.session_id = "s-1".to_string();
        c.files.insert("/tmp/a.jsonl".into(), e);
        c
    }

    #[test]
    fn saves_and_loads_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("usage-cache.v1.json");
        save_atomic(&path, &cache_with_one_file(420)).expect("save");

        match load(&path, 420) {
            LoadOutcome::Loaded(c) => {
                assert_eq!(c.files.len(), 1);
                assert_eq!(c.tz_offset_minutes, 420);
            }
            other => panic!("expected Loaded, got {:?}", other),
        }
    }

    #[test]
    fn missing_cache_requests_rebuild() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("absent.json");
        assert!(matches!(load(&path, 0), LoadOutcome::Rebuild(RebuildReason::Missing)));
    }

    #[test]
    fn corrupt_cache_is_quarantined_and_rebuilt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("usage-cache.v1.json");
        std::fs::write(&path, b"{ this is not valid json").expect("write");

        assert!(matches!(load(&path, 0), LoadOutcome::Rebuild(RebuildReason::Corrupt)));
        // The bad file is moved aside, not silently deleted, so it can be
        // inspected if this ever happens in the field.
        assert!(path.with_extension("json.corrupt").exists());
        assert!(!path.exists());
    }

    #[test]
    fn schema_bump_forces_rebuild() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("usage-cache.v1.json");
        let mut c = cache_with_one_file(0);
        c.schema = SCHEMA_VERSION + 1;
        std::fs::write(&path, serde_json::to_string(&c).expect("ser")).expect("write");

        assert!(matches!(load(&path, 0), LoadOutcome::Rebuild(RebuildReason::Schema)));
    }

    #[test]
    fn timezone_change_forces_rebuild_of_day_keys() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("usage-cache.v1.json");
        save_atomic(&path, &cache_with_one_file(420)).expect("save");

        // Machine moved from UTC+07 to UTC+00: stored day keys are now wrong.
        assert!(matches!(load(&path, 0), LoadOutcome::Rebuild(RebuildReason::Timezone)));
    }

    #[test]
    fn interrupted_write_leaves_the_previous_cache_intact() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("usage-cache.v1.json");
        save_atomic(&path, &cache_with_one_file(0)).expect("first save");

        // A stray tmp file from a crashed write must not affect the good cache.
        std::fs::write(path.with_extension("json.tmp"), b"garbage").expect("write tmp");
        match load(&path, 0) {
            LoadOutcome::Loaded(c) => assert_eq!(c.files.len(), 1),
            other => panic!("expected Loaded, got {:?}", other),
        }
    }

    #[test]
    fn save_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/deeper/usage-cache.v1.json");
        save_atomic(&path, &cache_with_one_file(0)).expect("save");
        assert!(path.exists());
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd src-tauri && cargo test usage::store 2>&1 | tail -20
```

Expected: compile errors, `cannot find function save_atomic`.

- [ ] **Step 3: Write the minimal implementation**

Prepend to `src-tauri/src/usage/store.rs`:

```rust
use crate::usage::types::{UsageCache, SCHEMA_VERSION};
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
pub enum RebuildReason {
    Missing,
    Corrupt,
    Schema,
    Timezone,
}

#[derive(Debug)]
pub enum LoadOutcome {
    Loaded(UsageCache),
    Rebuild(RebuildReason),
}

/// Load the cache, or say why a full rebuild is required. A rebuild costs one
/// pass over the transcripts (~3-6s for 793 MB), so it is always preferable to
/// serving wrong numbers.
pub fn load(path: &Path, current_tz_offset_minutes: i32) -> LoadOutcome {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return LoadOutcome::Rebuild(RebuildReason::Missing),
    };

    let cache: UsageCache = match serde_json::from_str(&contents) {
        Ok(c) => c,
        Err(_) => {
            // Move aside rather than delete, so a field failure is inspectable.
            let _ = std::fs::rename(path, path.with_extension("json.corrupt"));
            return LoadOutcome::Rebuild(RebuildReason::Corrupt);
        }
    };

    if cache.schema != SCHEMA_VERSION {
        return LoadOutcome::Rebuild(RebuildReason::Schema);
    }
    if cache.tz_offset_minutes != current_tz_offset_minutes {
        return LoadOutcome::Rebuild(RebuildReason::Timezone);
    }

    LoadOutcome::Loaded(cache)
}

/// Write via a temp file plus rename, so an interrupted write can never
/// replace a good cache with a truncated one.
pub fn save_atomic(path: &Path, cache: &UsageCache) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create {:?}: {}", parent, e))?;
    }
    let json = serde_json::to_string(cache).map_err(|e| format!("serialize cache: {}", e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json.as_bytes()).map_err(|e| format!("write {:?}: {}", tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename {:?}: {}", tmp, e))?;
    Ok(())
}
```

Add `pub mod store;` to `src-tauri/src/usage/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd src-tauri && cargo test usage::store 2>&1 | tail -15
```

Expected: `test result: ok. 7 passed`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/usage/store.rs src-tauri/src/usage/mod.rs
git commit -m "feat(usage): durable cache with atomic writes and self-healing rebuilds"
```

---

### Task 8: Config root resolution

**Files:**
- Create: `src-tauri/src/config.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod config;`)
- Test: inline in `src-tauri/src/config.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `ConfigRoots { claude_root, projects, sessions, stats_cache }`, `resolve(Option<PathBuf>) -> ConfigRoots`.

**Why:** `CLAUDE_CONFIG_DIR` can relocate `~/.claude`, and a GUI app launched from Finder cannot see the user's shell environment — so an explicit setting must be able to override.

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/config.rs` with ONLY this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn explicit_override_wins_over_everything() {
        let roots = resolve_with(Some(PathBuf::from("/custom/root")), Some("/env/root"), Some(PathBuf::from("/home/me")));
        assert_eq!(roots.claude_root, PathBuf::from("/custom/root"));
        assert_eq!(roots.projects, PathBuf::from("/custom/root/projects"));
        assert_eq!(roots.sessions, PathBuf::from("/custom/root/sessions"));
        assert_eq!(roots.stats_cache, PathBuf::from("/custom/root/stats-cache.json"));
    }

    #[test]
    fn env_var_used_when_no_explicit_override() {
        let roots = resolve_with(None, Some("/env/root"), Some(PathBuf::from("/home/me")));
        assert_eq!(roots.claude_root, PathBuf::from("/env/root"));
    }

    #[test]
    fn falls_back_to_home_dot_claude() {
        let roots = resolve_with(None, None, Some(PathBuf::from("/home/me")));
        assert_eq!(roots.claude_root, PathBuf::from("/home/me/.claude"));
        assert_eq!(roots.projects, PathBuf::from("/home/me/.claude/projects"));
    }

    #[test]
    fn empty_env_var_is_ignored() {
        let roots = resolve_with(None, Some(""), Some(PathBuf::from("/home/me")));
        assert_eq!(roots.claude_root, PathBuf::from("/home/me/.claude"));
    }

    #[test]
    fn missing_home_yields_relative_fallback_without_panicking() {
        let roots = resolve_with(None, None, None);
        assert_eq!(roots.claude_root, PathBuf::from(".claude"));
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd src-tauri && cargo test config:: 2>&1 | tail -20
```

Expected: compile error, `cannot find function resolve_with`.

- [ ] **Step 3: Write the minimal implementation**

Prepend to `src-tauri/src/config.rs`:

```rust
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ConfigRoots {
    pub claude_root: PathBuf,
    pub projects: PathBuf,
    pub sessions: PathBuf,
    pub stats_cache: PathBuf,
}

/// Precedence: explicit setting > CLAUDE_CONFIG_DIR > ~/.claude.
/// The seam exists so tests never depend on the machine's real env or home.
pub fn resolve_with(
    explicit: Option<PathBuf>,
    env_value: Option<&str>,
    home: Option<PathBuf>,
) -> ConfigRoots {
    let claude_root = explicit
        .or_else(|| {
            env_value
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| match home {
            Some(h) => h.join(".claude"),
            None => PathBuf::from(".claude"),
        });

    ConfigRoots {
        projects: claude_root.join("projects"),
        sessions: claude_root.join("sessions"),
        stats_cache: claude_root.join("stats-cache.json"),
        claude_root,
    }
}

pub fn resolve(explicit: Option<PathBuf>) -> ConfigRoots {
    let env_value = std::env::var("CLAUDE_CONFIG_DIR").ok();
    resolve_with(explicit, env_value.as_deref(), dirs::home_dir())
}
```

Add `mod config;` to `src-tauri/src/lib.rs` beside the other module declarations.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd src-tauri && cargo test config:: 2>&1 | tail -15
```

Expected: `test result: ok. 5 passed`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/config.rs src-tauri/src/lib.rs
git commit -m "feat(config): resolve claude root from setting, env, or home"
```

---

### Task 9: Adapter — derive the existing StatsCache

This is the task that keeps the entire Svelte frontend and its 39 tests working unchanged.

**Files:**
- Create: `src-tauri/src/usage/adapter.rs`
- Modify: `src-tauri/src/usage/mod.rs`
- Test: inline in `src-tauri/src/usage/adapter.rs`

**Interfaces:**
- Consumes: `UsageCache`, `FileEntry`, `TokenCounts` (Task 1); `date_key_with_offset`, `hour_with_offset` (Task 2); `StatsCache`, `DailyActivity`, `DailyModelTokens`, `ModelUsage`, `LongestSession` (existing, `lib.rs:15-75`).
- Produces: `to_stats_cache(&UsageCache) -> StatsCache`.

**Critical detail:** subagent transcripts live in separate files but share the **parent's** `sessionId`. So per-day `session_ids` must be **unioned** across files, never summed, or session counts inflate by the number of subagent files.

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/usage/adapter.rs` with ONLY this test module:

```rust
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
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd src-tauri && cargo test usage::adapter 2>&1 | tail -20
```

Expected: compile error, `cannot find function to_stats_cache`.

- [ ] **Step 3: Write the minimal implementation**

Prepend to `src-tauri/src/usage/adapter.rs`:

```rust
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
```

Add `pub mod adapter;` to `src-tauri/src/usage/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd src-tauri && cargo test usage::adapter 2>&1 | tail -15
```

Expected: `test result: ok. 8 passed`.

- [ ] **Step 5: Verify the whole Rust suite is green**

```bash
cd src-tauri && cargo test 2>&1 | tail -8
```

Expected: all tests pass, including the pre-existing `lib.rs` tests.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/usage/adapter.rs src-tauri/src/usage/mod.rs
git commit -m "feat(usage): derive StatsCache from rollups, unioning sessions across files"
```

---

### Task 10: Legacy seed from the old stats-cache

`~/.claude/stats-cache.json` covers 2026-01-25 -> 2026-03-10 (36 days, 154 sessions) with **no overlap** with the transcript window. Without this task that history is silently lost.

**Files:**
- Modify: `src-tauri/src/usage/legacy.rs` (replaces the Task 1 placeholder)
- Modify: `src-tauri/src/usage/adapter.rs` (merge the legacy rollup)
- Test: inline in `src-tauri/src/usage/legacy.rs`

**Interfaces:**
- Consumes: `StatsCache`, `ModelUsage`, `LongestSession` (existing `lib.rs`); `read_stats`-style parsing.
- Produces: `LegacyRollup { days, model_usage, total_sessions, total_messages, first_session_date, hour_counts, longest_session }`, `LegacyDay { tokens_by_model, message_count, session_count, tool_call_count }`, `seed_from_stats_cache(&Path) -> Option<LegacyRollup>`.

**Note on fidelity:** the old cache stores only a single total per model per day, not the four token classes. So legacy days contribute to `daily_model_tokens` and `daily_activity` but their all-time `model_usage` comes from the old cache's own `modelUsage` map, which does carry the breakdown.

- [ ] **Step 1: Write the failing test**

Replace the contents of `src-tauri/src/usage/legacy.rs` with ONLY this test module:

```rust
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
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd src-tauri && cargo test usage::legacy 2>&1 | tail -20
```

Expected: compile error, `cannot find function seed_from_stats_cache`.

- [ ] **Step 3: Write the minimal implementation**

Prepend to `src-tauri/src/usage/legacy.rs`:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd src-tauri && cargo test usage::legacy 2>&1 | tail -15
```

Expected: `test result: ok. 4 passed`.

- [ ] **Step 5: Write the failing adapter-merge test**

Append this test to the `tests` module inside `src-tauri/src/usage/adapter.rs`:

```rust
    #[test]
    fn merges_legacy_rollup_into_derived_stats() {
        use crate::usage::legacy::{LegacyDay, LegacyRollup};

        let mut cache = UsageCache::new(0);
        cache.files.insert(
            "/a.jsonl".into(),
            entry_with("s1", "2026-09-10", "claude-opus-5", counts(1, 1, 0, 0), 4, 2, 100, 200),
        );

        let mut legacy = LegacyRollup::default();
        legacy.total_sessions = 154;
        legacy.total_messages = 900;
        legacy.first_session_date = Some("2026-01-25".to_string());
        legacy.hour_counts.insert("9".to_string(), 40);
        let mut day = LegacyDay::default();
        day.message_count = 10;
        day.session_count = 2;
        day.tool_call_count = 5;
        day.tokens_by_model.insert("claude-opus-4-8".to_string(), 1000);
        legacy.days.insert("2026-01-25".to_string(), day);
        cache.legacy = Some(legacy);

        let stats = to_stats_cache(&cache);

        // Legacy day appears, and sorts before the transcript day.
        let dates: Vec<&str> = stats.daily_activity.iter().map(|d| d.date.as_str()).collect();
        assert_eq!(dates, vec!["2026-01-25", "2026-09-10"]);
        let legacy_day = &stats.daily_activity[0];
        assert_eq!(legacy_day.session_count, 2);
        assert_eq!(legacy_day.tool_call_count, 5);

        // Totals add on top of the live sessions.
        assert_eq!(stats.total_sessions, 155, "154 legacy + 1 live");
        assert_eq!(stats.total_messages, 904, "900 legacy + 4 live");
        // Earliest date comes from legacy.
        assert_eq!(stats.first_session_date.as_deref(), Some("2026-01-25"));
        assert_eq!(stats.hour_counts.expect("hc")["9"], 40);
    }
```

- [ ] **Step 6: Run it to verify it fails**

```bash
cd src-tauri && cargo test usage::adapter::tests::merges_legacy 2>&1 | tail -15
```

Expected: FAIL — legacy days and totals are absent from the derived stats.

- [ ] **Step 7: Merge legacy in the adapter**

In `src-tauri/src/usage/adapter.rs`, inside `to_stats_cache`, immediately **after** the `for entry in cache.files.values() { ... }` loop, insert:

```rust
    // --- Fold in the one-time legacy seed (§5.4) ---
    let mut legacy_sessions: u64 = 0;
    let mut legacy_messages: u64 = 0;
    let mut legacy_first_date: Option<String> = None;
    let mut legacy_hours: HashMap<String, u64> = HashMap::new();
    let mut legacy_day_activity: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
    let mut legacy_day_tokens: BTreeMap<String, HashMap<String, u64>> = BTreeMap::new();

    if let Some(legacy) = cache.legacy.as_ref() {
        legacy_sessions = legacy.total_sessions;
        legacy_messages = legacy.total_messages;
        legacy_first_date = legacy.first_session_date.clone();
        legacy_hours = legacy.hour_counts.clone();
        for (date, d) in &legacy.days {
            legacy_day_activity
                .insert(date.clone(), (d.message_count, d.session_count, d.tool_call_count));
            legacy_day_tokens.insert(date.clone(), d.tokens_by_model.clone());
            days.entry(date.clone()).or_default();
        }
        for (model, u) in &legacy.model_usage {
            let c = model_usage.entry(model.clone()).or_default();
            c.input += u.input_tokens;
            c.output += u.output_tokens;
            c.cache_read += u.cache_read_input_tokens;
            c.cache_creation += u.cache_creation_input_tokens;
            c.web_search += u.web_search_requests;
        }
    }
```

Then change the two day-vector builders to add the legacy contribution. Replace the `daily_activity` builder with:

```rust
    let daily_activity: Vec<DailyActivity> = days
        .iter()
        .map(|(date, d)| {
            let (lm, ls, lt) = legacy_day_activity.get(date).copied().unwrap_or((0, 0, 0));
            DailyActivity {
                date: date.clone(),
                message_count: d.message_count + lm,
                session_count: d.session_ids.len() as u64 + ls,
                tool_call_count: d.tool_call_count + lt,
            }
        })
        .collect();
```

and the `daily_model_tokens` builder with:

```rust
    let daily_model_tokens: Vec<DailyModelTokens> = days
        .iter()
        .map(|(date, d)| {
            let mut tokens_by_model: HashMap<String, u64> = d
                .by_model
                .iter()
                .map(|(m, c)| (m.clone(), c.billable_total()))
                .collect();
            if let Some(legacy_tokens) = legacy_day_tokens.get(date) {
                for (m, v) in legacy_tokens {
                    *tokens_by_model.entry(m.clone()).or_insert(0) += v;
                }
            }
            DailyModelTokens { date: date.clone(), tokens_by_model }
        })
        .collect();
```

Then replace the totals, first-date and hour-count expressions in the returned `StatsCache`:

```rust
    for (hour, n) in legacy_hours {
        *hour_counts.entry(hour).or_insert(0) += n;
    }

    let first_session_date = match (first_session_date, legacy_first_date) {
        (Some(a), Some(b)) => Some(if a <= b { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
```

and in the struct literal:

```rust
        total_sessions: sessions.len() as u64 + legacy_sessions,
        total_messages: sessions.values().map(|s| s.message_count).sum::<u64>() + legacy_messages,
```

Note: `first_session_date` and `hour_counts` are declared with `let` earlier in the function; change them to `let mut hour_counts` and rebind `first_session_date` as shown so these additions compile.

- [ ] **Step 8: Run the adapter tests to verify they pass**

```bash
cd src-tauri && cargo test usage::adapter 2>&1 | tail -15
```

Expected: `test result: ok. 9 passed`.

- [ ] **Step 9: Commit**

```bash
git add src-tauri/src/usage/legacy.rs src-tauri/src/usage/adapter.rs
git commit -m "feat(usage): seed retired stats-cache history and merge it into stats"
```

---

### Task 11: Scanner and ingest worker

**Files:**
- Create: `src-tauri/src/usage/scanner.rs`
- Create: `src-tauri/src/usage/worker.rs`
- Modify: `src-tauri/src/usage/mod.rs`
- Test: inline in both files

**Interfaces:**
- Consumes: everything from Tasks 1-10.
- Produces:
  - `scanner::ScanReport { files_seen, files_read, files_retired, bytes_read, malformed, deduped }`
  - `scanner::scan_once(&mut UsageCache, &Path, Option<&mut dyn FnMut(usize, usize)>) -> ScanReport`
  - `worker::UsageWorker`, `worker::UsageWorker::new(ConfigRoots, PathBuf) -> UsageWorker`, `.refresh_now() -> ScanReport`, `.snapshot() -> StatsCache`, `.diagnostics() -> Diagnostics`
  - `worker::Diagnostics { malformed_lines, divergent_copies, files_tracked, files_retired, last_scan_ms }`

- [ ] **Step 1: Write the failing scanner test**

Create `src-tauri/src/usage/scanner.rs` with ONLY this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::types::UsageCache;
    use std::io::Write;

    fn line(msg_id: &str, out: u64) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","sessionId":"s-1","cwd":"/p","message":{{"id":"{}","model":"claude-opus-5","content":[{{"type":"text","text":"x"}}],"usage":{{"input_tokens":1,"output_tokens":{},"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#,
            msg_id, out
        )
    }

    fn write_transcript(root: &std::path::Path, rel: &str, lines: &[String]) -> std::path::PathBuf {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let mut f = std::fs::File::create(&path).expect("create");
        for l in lines {
            writeln!(f, "{}", l).expect("write");
        }
        f.flush().expect("flush");
        path
    }

    #[test]
    fn first_scan_ingests_everything() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_transcript(dir.path(), "-p/s.jsonl", &[line("m1", 10), line("m2", 20)]);

        let mut cache = UsageCache::new(0);
        let report = scan_once(&mut cache, dir.path(), None);

        assert_eq!(report.files_seen, 1);
        assert_eq!(report.files_read, 1);
        assert_eq!(cache.files.len(), 1);
        let entry = cache.files.values().next().expect("entry");
        assert_eq!(entry.days["2026-09-10"].by_model["claude-opus-5"].output, 30);
    }

    #[test]
    fn second_scan_reads_nothing_when_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_transcript(dir.path(), "-p/s.jsonl", &[line("m1", 10)]);

        let mut cache = UsageCache::new(0);
        scan_once(&mut cache, dir.path(), None);
        let report = scan_once(&mut cache, dir.path(), None);

        assert_eq!(report.files_seen, 1);
        assert_eq!(report.files_read, 0, "unchanged file must not be re-read");
        assert_eq!(report.bytes_read, 0);
    }

    #[test]
    fn appended_lines_are_ingested_without_recounting_old_ones() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_transcript(dir.path(), "-p/s.jsonl", &[line("m1", 10)]);

        let mut cache = UsageCache::new(0);
        scan_once(&mut cache, dir.path(), None);

        // Append a new message. mtime must differ for the scan to notice.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).expect("open");
        writeln!(f, "{}", line("m2", 20)).expect("append");
        f.flush().expect("flush");

        let report = scan_once(&mut cache, dir.path(), None);
        assert_eq!(report.files_read, 1);
        let entry = cache.files.values().next().expect("entry");
        assert_eq!(entry.days["2026-09-10"].by_model["claude-opus-5"].output, 30);
    }

    #[test]
    fn deleted_file_is_retired_keeping_its_history() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_transcript(dir.path(), "-p/s.jsonl", &[line("m1", 10)]);

        let mut cache = UsageCache::new(0);
        scan_once(&mut cache, dir.path(), None);
        std::fs::remove_file(&path).expect("remove");

        let report = scan_once(&mut cache, dir.path(), None);
        assert_eq!(report.files_retired, 1);

        let entry = cache.files.values().next().expect("entry still tracked");
        assert!(entry.retired);
        assert!(entry.seen.is_empty(), "dedup set must be dropped on retirement");
        assert_eq!(
            entry.days["2026-09-10"].by_model["claude-opus-5"].output, 10,
            "history must survive upstream pruning"
        );
    }

    #[test]
    fn truncated_file_is_reingested_without_double_counting() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_transcript(dir.path(), "-p/s.jsonl", &[line("m1", 10), line("m2", 20)]);

        let mut cache = UsageCache::new(0);
        scan_once(&mut cache, dir.path(), None);

        std::thread::sleep(std::time::Duration::from_millis(20));
        write_transcript(dir.path(), "-p/s.jsonl", &[line("m1", 10)]);
        let _ = path;

        scan_once(&mut cache, dir.path(), None);
        let entry = cache.files.values().next().expect("entry");
        assert_eq!(entry.days["2026-09-10"].by_model["claude-opus-5"].output, 10);
    }

    #[test]
    fn reports_progress_for_each_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_transcript(dir.path(), "-p/a.jsonl", &[line("m1", 1)]);
        write_transcript(dir.path(), "-p/b.jsonl", &[line("m2", 1)]);

        let mut seen: Vec<(usize, usize)> = Vec::new();
        let mut cache = UsageCache::new(0);
        {
            let mut cb = |done: usize, total: usize| seen.push((done, total));
            scan_once(&mut cache, dir.path(), Some(&mut cb));
        }
        assert_eq!(seen, vec![(1, 2), (2, 2)]);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cd src-tauri && cargo test usage::scanner 2>&1 | tail -20
```

Expected: compile error, `cannot find function scan_once`.

- [ ] **Step 3: Implement the scanner**

Prepend to `src-tauri/src/usage/scanner.rs`:

```rust
use crate::usage::cursor::{decide, read_meta, read_range, ScanAction};
use crate::usage::discovery::discover_transcripts;
use crate::usage::ingest::ingest_text;
use crate::usage::types::{FileEntry, UsageCache};
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub files_seen: usize,
    pub files_read: usize,
    pub files_retired: usize,
    pub bytes_read: u64,
    pub malformed: u64,
    pub deduped: u64,
}

/// One full reconciliation pass. Cheap when little changed: unchanged files
/// cost a single `stat` each.
pub fn scan_once(
    cache: &mut UsageCache,
    projects_root: &Path,
    mut progress: Option<&mut dyn FnMut(usize, usize)>,
) -> ScanReport {
    let mut report = ScanReport::default();
    let tz = cache.tz_offset_minutes;

    let paths = discover_transcripts(projects_root);
    let total = paths.len();
    report.files_seen = total;
    let present: HashSet<_> = paths.iter().cloned().collect();

    for (i, path) in paths.into_iter().enumerate() {
        let meta = read_meta(&path);
        let action = decide(cache.files.get(&path).map(|e| &e.cursor), meta.as_ref());

        match action {
            ScanAction::Skip | ScanAction::Retire => {}
            ScanAction::Full | ScanAction::Delta { .. } => {
                let from = match action {
                    ScanAction::Delta { from } => from,
                    _ => 0,
                };
                if matches!(action, ScanAction::Full) {
                    // Discard any prior contribution before re-ingesting.
                    cache.files.insert(path.clone(), FileEntry::default());
                }
                if let Ok((text, consumed)) = read_range(&path, from) {
                    let entry = cache.files.entry(path.clone()).or_default();
                    let stats = ingest_text(entry, &text, tz);
                    report.malformed += stats.malformed;
                    report.deduped += stats.deduped;
                    report.bytes_read += consumed;
                    report.files_read += 1;

                    if let Some(m) = meta.as_ref() {
                        entry.cursor.offset = from + consumed;
                        entry.cursor.size = m.size;
                        entry.cursor.mtime_ms = m.mtime_ms;
                        entry.cursor.inode = m.inode;
                    }
                    entry.retired = false;
                }
            }
        }

        if let Some(cb) = progress.as_mut() {
            cb(i + 1, total);
        }
    }

    // Retire tracked files that no longer exist upstream. Their day rollups
    // stay; only the dedup sets are dropped to bound cache growth.
    for (path, entry) in cache.files.iter_mut() {
        if !present.contains(path) && !entry.retired {
            entry.retired = true;
            entry.seen.clear();
            entry.seen_tools.clear();
            entry.seen_users.clear();
            report.files_retired += 1;
        }
    }

    report
}
```

Add `pub mod scanner;` to `src-tauri/src/usage/mod.rs`.

- [ ] **Step 4: Run the scanner tests to verify they pass**

```bash
cd src-tauri && cargo test usage::scanner 2>&1 | tail -15
```

Expected: `test result: ok. 6 passed`.

- [ ] **Step 5: Write the failing worker test**

Create `src-tauri/src/usage/worker.rs` with ONLY this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::resolve_with;
    use std::io::Write;

    fn setup(dir: &std::path::Path) -> UsageWorker {
        let projects = dir.join("claude/projects");
        std::fs::create_dir_all(&projects).expect("mkdir");
        let path = projects.join("-p/s.jsonl");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let mut f = std::fs::File::create(&path).expect("create");
        writeln!(
            f,
            r#"{{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","sessionId":"s-1","message":{{"id":"m1","model":"claude-opus-5","content":[],"usage":{{"input_tokens":1,"output_tokens":9,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
        )
        .expect("write");
        f.flush().expect("flush");

        let roots = resolve_with(Some(dir.join("claude")), None, None);
        UsageWorker::new(roots, dir.join("cache/usage-cache.v1.json"))
    }

    #[test]
    fn refresh_then_snapshot_exposes_stats() {
        let dir = tempfile::tempdir().expect("tempdir");
        let worker = setup(dir.path());

        let report = worker.refresh_now();
        assert_eq!(report.files_read, 1);

        let stats = worker.snapshot();
        let day = stats
            .daily_model_tokens
            .iter()
            .find(|d| d.date == "2026-09-10")
            .expect("day");
        assert_eq!(day.tokens_by_model["claude-opus-5"], 10); // 1 + 9
    }

    #[test]
    fn persists_and_reloads_without_re_reading_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let worker = setup(dir.path());
        worker.refresh_now();
        worker.persist().expect("persist");

        // A fresh worker over the same cache path must skip the unchanged file.
        let roots = resolve_with(Some(dir.path().join("claude")), None, None);
        let worker2 = UsageWorker::new(roots, dir.path().join("cache/usage-cache.v1.json"));
        let report = worker2.refresh_now();
        assert_eq!(report.files_read, 0, "cache should have been reused");
        assert_eq!(worker2.snapshot().total_sessions, 1);
    }

    #[test]
    fn diagnostics_expose_tracked_files_and_counters() {
        let dir = tempfile::tempdir().expect("tempdir");
        let worker = setup(dir.path());
        worker.refresh_now();
        let d = worker.diagnostics();
        assert_eq!(d.files_tracked, 1);
        assert_eq!(d.files_retired, 0);
        assert_eq!(d.malformed_lines, 0);
        assert_eq!(d.divergent_copies, 0);
    }

    #[test]
    fn concurrent_refreshes_do_not_double_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        let worker = std::sync::Arc::new(setup(dir.path()));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let w = worker.clone();
                std::thread::spawn(move || {
                    w.refresh_now();
                })
            })
            .collect();
        for h in handles {
            h.join().expect("join");
        }

        let stats = worker.snapshot();
        let day = stats
            .daily_model_tokens
            .iter()
            .find(|d| d.date == "2026-09-10")
            .expect("day");
        assert_eq!(day.tokens_by_model["claude-opus-5"], 10, "must not multiply");
    }
}
```

- [ ] **Step 6: Run it to verify it fails**

```bash
cd src-tauri && cargo test usage::worker 2>&1 | tail -20
```

Expected: compile error, `cannot find type UsageWorker`.

- [ ] **Step 7: Implement the worker**

Prepend to `src-tauri/src/usage/worker.rs`:

```rust
use crate::config::ConfigRoots;
use crate::usage::adapter::to_stats_cache;
use crate::usage::dates::current_tz_offset_minutes;
use crate::usage::legacy::seed_from_stats_cache;
use crate::usage::scanner::{scan_once, ScanReport};
use crate::usage::store::{load, save_atomic, LoadOutcome};
use crate::usage::types::UsageCache;
use crate::StatsCache;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    pub malformed_lines: u64,
    pub divergent_copies: u64,
    pub files_tracked: usize,
    pub files_retired: usize,
    pub last_scan_ms: u64,
}

/// Owns the cache. Every mutation goes through one mutex, so the watcher, the
/// fallback poll, manual refresh and `get_stats` cannot race or double-count.
pub struct UsageWorker {
    roots: ConfigRoots,
    cache_path: PathBuf,
    cache: Mutex<UsageCache>,
}

impl UsageWorker {
    pub fn new(roots: ConfigRoots, cache_path: PathBuf) -> Self {
        let tz = current_tz_offset_minutes();
        let mut cache = match load(&cache_path, tz) {
            LoadOutcome::Loaded(c) => c,
            LoadOutcome::Rebuild(reason) => {
                eprintln!("[usage] rebuilding cache: {:?}", reason);
                UsageCache::new(tz)
            }
        };
        // One-time seed of the retired stats-cache.json history.
        if cache.legacy.is_none() {
            cache.legacy = seed_from_stats_cache(&roots.stats_cache);
        }
        UsageWorker { roots, cache_path, cache: Mutex::new(cache) }
    }

    pub fn refresh_now(&self) -> ScanReport {
        self.refresh_with_progress(None)
    }

    pub fn refresh_with_progress(
        &self,
        progress: Option<&mut dyn FnMut(usize, usize)>,
    ) -> ScanReport {
        let started = std::time::Instant::now();
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let report = scan_once(&mut guard, &self.roots.projects, progress);
        let _ = started;
        report
    }

    pub fn snapshot(&self) -> StatsCache {
        let guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        to_stats_cache(&guard)
    }

    pub fn diagnostics(&self) -> Diagnostics {
        let guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        Diagnostics {
            malformed_lines: guard.files.values().map(|e| e.malformed_lines).sum(),
            divergent_copies: guard.files.values().map(|e| e.divergent_copies).sum(),
            files_tracked: guard.files.len(),
            files_retired: guard.files.values().filter(|e| e.retired).count(),
            last_scan_ms: 0,
        }
    }

    pub fn persist(&self) -> Result<(), String> {
        let guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        save_atomic(&self.cache_path, &guard)
    }
}
```

Add `pub mod worker;` to `src-tauri/src/usage/mod.rs`.

- [ ] **Step 8: Run the worker tests to verify they pass**

```bash
cd src-tauri && cargo test usage::worker 2>&1 | tail -15
```

Expected: `test result: ok. 4 passed`.

- [ ] **Step 9: Commit**

```bash
git add src-tauri/src/usage/scanner.rs src-tauri/src/usage/worker.rs src-tauri/src/usage/mod.rs
git commit -m "feat(usage): incremental scanner and single-writer ingest worker"
```

---

### Task 12: Wire the worker into Tauri and fix the month-boundary bug

**Files:**
- Modify: `src-tauri/src/lib.rs` (`current_month_prefix` at `:93-124`, `current_month_tokens` at `:127-135`, `update_tray_from_stats` at `:137-149`, `get_stats` at `:151-154`, `run()` at `:182`)
- Test: inline in `src-tauri/src/lib.rs` (extend the existing `tests` module)

**Interfaces:**
- Consumes: `UsageWorker`, `Diagnostics` (Task 11); `resolve` (Task 8); `local_month_prefix`, `now_ms` (Task 2).
- Produces: Tauri commands `get_stats`, `get_diagnostics`, `refresh_usage`; managed state `Arc<UsageWorker>`.

**The bug being fixed:** `current_month_prefix()` derives the month from UTC epoch days by hand, while `api.ts::getCurrentMonthPrefix()` uses local time. On a UTC+07 machine the tray disagrees with the dashboard for the first 7 hours of every month.

- [ ] **Step 1: Write the failing test**

Add to the existing `mod tests` in `src-tauri/src/lib.rs`:

```rust
    #[test]
    fn current_month_prefix_matches_local_time_not_utc() {
        // The hand-rolled UTC epoch-day math disagreed with the frontend's
        // local-time month for the first hours of each month east of UTC.
        let expected = crate::usage::dates::local_month_prefix(
            crate::usage::dates::now_ms(),
        );
        assert_eq!(current_month_prefix(), expected);
    }

    #[test]
    fn current_month_prefix_is_well_formed() {
        let p = current_month_prefix();
        assert_eq!(p.len(), 7, "expected YYYY-MM, got {}", p);
        assert_eq!(&p[4..5], "-");
        let year: i32 = p[0..4].parse().expect("year");
        let month: u32 = p[5..7].parse().expect("month");
        assert!(year >= 2026, "year was {}", year);
        assert!((1..=12).contains(&month), "month was {}", month);
    }
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cd src-tauri && cargo test current_month_prefix 2>&1 | tail -20
```

Expected: `current_month_prefix_matches_local_time_not_utc` FAILS on any machine east of UTC during the first hours of a month, and compiles-but-passes elsewhere. Either way the next step makes it correct by construction rather than by luck.

- [ ] **Step 3: Replace the hand-rolled month math**

In `src-tauri/src/lib.rs`, delete the entire body of `current_month_prefix` (lines 93-124, the epoch-day loop) and replace the function with:

```rust
pub fn current_month_prefix() -> String {
    usage::dates::local_month_prefix(usage::dates::now_ms())
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cd src-tauri && cargo test current_month_prefix 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 5: Repoint the commands at the worker**

In `src-tauri/src/lib.rs`, replace the `get_stats` command and add two more:

```rust
#[tauri::command]
async fn get_stats(worker: tauri::State<'_, std::sync::Arc<usage::worker::UsageWorker>>) -> Result<StatsCache, String> {
    Ok(worker.snapshot())
}

#[tauri::command]
async fn get_diagnostics(
    worker: tauri::State<'_, std::sync::Arc<usage::worker::UsageWorker>>,
) -> Result<usage::worker::Diagnostics, String> {
    Ok(worker.diagnostics())
}

#[tauri::command]
async fn refresh_usage(
    worker: tauri::State<'_, std::sync::Arc<usage::worker::UsageWorker>>,
) -> Result<(), String> {
    worker.refresh_now();
    worker.persist()
}
```

Replace `update_tray_from_stats` so it reads the worker rather than the dead file:

```rust
pub fn update_tray_from_worker(app: &AppHandle) {
    let worker = match app.try_state::<std::sync::Arc<usage::worker::UsageWorker>>() {
        Some(w) => w.inner().clone(),
        None => return,
    };
    let stats = worker.snapshot();
    let month_tokens = current_month_tokens(&stats);
    let title = format_tokens(month_tokens);
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_title(Some(&title));
    }
    let _ = app.emit("stats-updated", ());
}
```

Keep `read_stats` and `stats_cache_path` — Task 10 uses the old file as the legacy seed, and the existing tests for them stay valid.

- [ ] **Step 6: Build the worker in `run()` and register the commands**

In `run()`, change the `invoke_handler` list to:

```rust
        .invoke_handler(tauri::generate_handler![
            get_stats,
            get_diagnostics,
            refresh_usage,
            update_tray_title,
        ])
```

At the very start of the `.setup(|app| { ... })` closure, before the tray is built:

```rust
            let roots = config::resolve(None);
            let cache_path = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("app data dir: {}", e))?
                .join("usage-cache.v1.json");
            let worker = std::sync::Arc::new(usage::worker::UsageWorker::new(roots, cache_path));
            app.manage(worker.clone());

            // First pass on a background thread so the popover never blocks.
            // Measured: a full 793 MB pass takes seconds, not minutes.
            {
                let handle = app.handle().clone();
                let worker = worker.clone();
                std::thread::spawn(move || {
                    let mut cb = |done: usize, total: usize| {
                        let _ = handle.emit("usage-progress", (done, total));
                    };
                    worker.refresh_with_progress(Some(&mut cb));
                    let _ = worker.persist();
                    update_tray_from_worker(&handle);
                });
            }
```

Replace the later `update_tray_from_stats(&handle);` call with `update_tray_from_worker(&handle);`.

`app.path()` requires `use tauri::Manager;`, which `lib.rs` already imports.

- [ ] **Step 7: Verify the Rust suite and a real build**

```bash
cd src-tauri && cargo test 2>&1 | tail -8
```

Expected: all tests pass.

```bash
cd /Users/nathanaelmcmillan/Projects/claude-token-usage && npx vite build 2>&1 | tail -5
```

Expected: frontend builds (it is unchanged, so this confirms nothing broke).

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat(usage): serve stats from the ingest worker; fix UTC month-prefix bug

current_month_prefix derived the month from UTC epoch days by hand while
the frontend used local time, so the tray disagreed with the dashboard
for the first hours of every month east of UTC."
```

---

### Task 13: Recursive, debounced file watching

**Files:**
- Modify: `src-tauri/src/polling.rs` (whole file)
- Test: inline in `src-tauri/src/polling.rs`

**Interfaces:**
- Consumes: `UsageWorker` (Task 11), `ConfigRoots` (Task 8).
- Produces: `start(AppHandle, ConfigRoots)`, `should_react(&[PathBuf]) -> bool`, `DEBOUNCE`.

**Why:** today's watcher is non-recursive on `~/.claude` and matches one filename, so it would never see a transcript change. And with 5 live sessions transcripts change several times per second — an undebounced handler would rewrite a multi-MB cache thousands of times a day.

- [ ] **Step 1: Write the failing test**

Add to `src-tauri/src/polling.rs` a test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn reacts_to_transcript_changes() {
        let paths = vec![PathBuf::from(
            "/Users/me/.claude/projects/-p/1111.jsonl",
        )];
        assert!(should_react(&paths));
    }

    #[test]
    fn reacts_to_nested_subagent_transcripts() {
        let paths = vec![PathBuf::from(
            "/Users/me/.claude/projects/-p/1111/subagents/workflows/wf_x/agent-y.jsonl",
        )];
        assert!(should_react(&paths));
    }

    #[test]
    fn ignores_unrelated_files() {
        let paths = vec![
            PathBuf::from("/Users/me/.claude/projects/-p/notes.md"),
            PathBuf::from("/Users/me/.claude/history.jsonl.tmp"),
            PathBuf::from("/Users/me/.claude/file-history/x.json"),
        ];
        assert!(!should_react(&paths));
    }

    #[test]
    fn reacts_when_any_path_in_a_batch_matches() {
        let paths = vec![
            PathBuf::from("/Users/me/.claude/projects/-p/notes.md"),
            PathBuf::from("/Users/me/.claude/projects/-p/1111.jsonl"),
        ];
        assert!(should_react(&paths));
    }

    #[test]
    fn debounce_is_long_enough_to_coalesce_bursts() {
        // Five live sessions write several times per second; anything under
        // ~500ms would let a burst trigger repeated multi-MB cache writes.
        assert!(DEBOUNCE >= std::time::Duration::from_millis(500));
        assert!(DEBOUNCE <= std::time::Duration::from_secs(3));
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cd src-tauri && cargo test polling:: 2>&1 | tail -20
```

Expected: compile error, `cannot find function should_react`.

- [ ] **Step 3: Rewrite polling.rs**

Replace the entire contents of `src-tauri/src/polling.rs` with:

```rust
use crate::config::ConfigRoots;
use crate::usage::worker::UsageWorker;
use notify::{Event, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// Coalescing window for filesystem events. Transcripts from several live
/// sessions change many times per second; without this the cache would be
/// rewritten thousands of times a day.
pub const DEBOUNCE: Duration = Duration::from_millis(1500);

/// Fallback refresh when no filesystem event arrives.
const FALLBACK_POLL: Duration = Duration::from_secs(60);

/// True when a batch of changed paths contains at least one transcript.
pub fn should_react(paths: &[PathBuf]) -> bool {
    paths.iter().any(|p| {
        p.extension().and_then(|e| e.to_str()) == Some("jsonl")
            && p.components().any(|c| c.as_os_str() == "projects")
    })
}

pub fn start(app: AppHandle, roots: ConfigRoots) {
    std::thread::spawn(move || {
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        let mut watcher = match notify::recommended_watcher(tx) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("[polling] could not create watcher: {}", e);
                return;
            }
        };

        // Recursive on projects/: transcripts nest several levels deep.
        if let Err(e) = watcher.watch(&roots.projects, RecursiveMode::Recursive) {
            eprintln!("[polling] could not watch {:?}: {}", roots.projects, e);
        }
        // Non-recursive on sessions/: flat, and Phase 2 consumes it.
        if let Err(e) = watcher.watch(&roots.sessions, RecursiveMode::NonRecursive) {
            eprintln!("[polling] could not watch {:?}: {}", roots.sessions, e);
        }

        let mut dirty = false;
        let mut dirty_since = Instant::now();

        loop {
            let timeout = if dirty {
                DEBOUNCE.saturating_sub(dirty_since.elapsed()).max(Duration::from_millis(50))
            } else {
                FALLBACK_POLL
            };

            match rx.recv_timeout(timeout) {
                Ok(Ok(event)) => {
                    if should_react(&event.paths) {
                        if !dirty {
                            dirty = true;
                            dirty_since = Instant::now();
                        }
                    }
                }
                Ok(Err(e)) => eprintln!("[polling] watch error: {}", e),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Either the debounce window closed, or the fallback fired.
                    dirty = false;
                    refresh(&app);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }

            if dirty && dirty_since.elapsed() >= DEBOUNCE {
                dirty = false;
                refresh(&app);
            }
        }
    });
}

fn refresh(app: &AppHandle) {
    let worker = match app.try_state::<Arc<UsageWorker>>() {
        Some(w) => w.inner().clone(),
        None => return,
    };
    worker.refresh_now();
    if let Err(e) = worker.persist() {
        eprintln!("[polling] could not persist cache: {}", e);
    }
    crate::update_tray_from_worker(app);
}

```

In `src-tauri/src/lib.rs`'s `setup` closure, change the watcher start to pass the roots:

```rust
            polling::start(handle.clone(), config::resolve(None));
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd src-tauri && cargo test polling:: 2>&1 | tail -15
```

Expected: `test result: ok. 5 passed`.

- [ ] **Step 5: Verify the whole suite**

```bash
cd src-tauri && cargo test 2>&1 | tail -8
```

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/polling.rs src-tauri/src/lib.rs
git commit -m "feat(polling): recursive debounced transcript watching"
```

---

### Task 14: Frontend — progress, diagnostics, and config root

**Files:**
- Modify: `src/lib/types.ts`
- Modify: `src/lib/api.ts`
- Modify: `src/components/Dashboard.svelte`
- Modify: `src/components/Settings.svelte`
- Test: `src/lib/__tests__/api.test.ts` (extend existing)

**Interfaces:**
- Consumes: Tauri commands `get_stats`, `get_diagnostics`, `refresh_usage` (Task 12); events `stats-updated`, `usage-progress` (Tasks 11-12).
- Produces: `Diagnostics` TS interface, `getDiagnostics()`, `refreshUsage()`.

**Scope note:** the dashboard's own rendering is deliberately untouched — `get_stats` still returns `StatsCache`, so `stats.ts`, `TokenSummary`, `DailyChart`, `ModelBreakdown` and `ActivityStats` all keep working. This task only adds the first-run progress indicator and surfaces the ingest diagnostics.

- [ ] **Step 1: Write the failing test**

Add to `src/lib/__tests__/api.test.ts`:

```ts
  it("getDiagnostics invokes the get_diagnostics command", async () => {
    const invoke = vi.mocked(await import("@tauri-apps/api/core")).invoke;
    invoke.mockResolvedValueOnce({
      malformedLines: 3,
      divergentCopies: 0,
      filesTracked: 1031,
      filesRetired: 12,
      lastScanMs: 4200,
    });

    const d = await getDiagnostics();
    expect(invoke).toHaveBeenCalledWith("get_diagnostics");
    expect(d.filesTracked).toBe(1031);
    expect(d.malformedLines).toBe(3);
  });

  it("refreshUsage invokes the refresh_usage command", async () => {
    const invoke = vi.mocked(await import("@tauri-apps/api/core")).invoke;
    invoke.mockResolvedValueOnce(undefined);
    await refreshUsage();
    expect(invoke).toHaveBeenCalledWith("refresh_usage");
  });
```

and extend that file's import from `../api` to include `getDiagnostics` and `refreshUsage`.

- [ ] **Step 2: Run it to verify it fails**

```bash
npm test 2>&1 | tail -20
```

Expected: FAIL — `getDiagnostics is not a function`.

- [ ] **Step 3: Add the types**

Append to `src/lib/types.ts`:

```ts
// --- Ingest diagnostics (from the Rust usage worker) ---

export interface Diagnostics {
  malformedLines: number;
  divergentCopies: number;
  filesTracked: number;
  filesRetired: number;
  lastScanMs: number;
}
```

- [ ] **Step 4: Add the API functions**

Append to `src/lib/api.ts`:

```ts
export async function getDiagnostics(): Promise<Diagnostics> {
  return invoke<Diagnostics>("get_diagnostics");
}

export async function refreshUsage(): Promise<void> {
  return invoke("refresh_usage");
}
```

and extend the existing type import at the top of the file:

```ts
import type { StatsCache, Diagnostics } from "./types";
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
npm test 2>&1 | tail -12
```

Expected: all tests pass — the original 39 plus the 2 new ones.

- [ ] **Step 6: Show first-run progress in the dashboard**

In `src/components/Dashboard.svelte`, inside the existing `<script lang="ts">` block, add alongside the current state declarations:

```ts
  import { listen } from "@tauri-apps/api/event";

  let progress = $state<{ done: number; total: number } | null>(null);

  $effect(() => {
    const unlisten = listen<[number, number]>("usage-progress", (e) => {
      const [done, total] = e.payload;
      progress = done >= total ? null : { done, total };
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  });
```

and immediately after the component's outermost opening element in the markup, add:

```svelte
{#if progress}
  <div
    class="px-4 py-2 text-xs text-gray-500 dark:text-gray-400 border-b border-gray-200 dark:border-gray-800"
  >
    Reading transcripts… {progress.done} / {progress.total}
  </div>
{/if}
```

- [ ] **Step 7: Surface diagnostics and the config root in Settings**

In `src/components/Settings.svelte`, inside the existing `<script lang="ts">` block:

```ts
  import { getDiagnostics, refreshUsage } from "../lib/api";
  import type { Diagnostics } from "../lib/types";

  let diagnostics = $state<Diagnostics | null>(null);
  let refreshing = $state(false);

  $effect(() => {
    getDiagnostics().then((d) => (diagnostics = d));
  });

  async function rescan() {
    refreshing = true;
    try {
      await refreshUsage();
      diagnostics = await getDiagnostics();
    } finally {
      refreshing = false;
    }
  }
```

and add this section to its markup, following the file's existing section markup style:

```svelte
<div class="px-4 py-3 border-t border-gray-200 dark:border-gray-800">
  <h3 class="text-xs font-semibold uppercase tracking-wide text-gray-500 dark:text-gray-400">
    Data
  </h3>
  {#if diagnostics}
    <dl class="mt-2 space-y-1 text-xs text-gray-600 dark:text-gray-300">
      <div class="flex justify-between">
        <dt>Transcripts tracked</dt>
        <dd>{diagnostics.filesTracked}</dd>
      </div>
      <div class="flex justify-between">
        <dt>Pruned upstream (history kept)</dt>
        <dd>{diagnostics.filesRetired}</dd>
      </div>
      <p class="pt-1 text-gray-500 dark:text-gray-400">
        Menu bar total sums input, output and cache tokens for the current
        month. Cache reads usually dominate it.
      </p>
      {#if diagnostics.malformedLines > 0}
        <div class="flex justify-between text-amber-600 dark:text-amber-400">
          <dt>Unreadable records</dt>
          <dd>{diagnostics.malformedLines}</dd>
        </div>
      {/if}
      {#if diagnostics.divergentCopies > 0}
        <div class="flex justify-between text-amber-600 dark:text-amber-400">
          <dt>Conflicting duplicates</dt>
          <dd>{diagnostics.divergentCopies}</dd>
        </div>
      {/if}
    </dl>
  {/if}
  <button
    class="mt-2 text-xs text-blue-600 dark:text-blue-400 disabled:opacity-50"
    disabled={refreshing}
    onclick={rescan}
  >
    {refreshing ? "Rescanning…" : "Rescan transcripts"}
  </button>
</div>
```

- [ ] **Step 8: Verify build and both suites**

```bash
npm test 2>&1 | tail -8 && npx vite build 2>&1 | tail -5 && cd src-tauri && cargo test 2>&1 | tail -6
```

Expected: 41 frontend tests pass, frontend builds, all Rust tests pass.

- [ ] **Step 9: Commit**

```bash
git add src/lib/types.ts src/lib/api.ts src/lib/__tests__/api.test.ts src/components/Dashboard.svelte src/components/Settings.svelte
git commit -m "feat(ui): first-run progress and ingest diagnostics"
```

---

### Task 15: Manual verification against real data

Tests cannot prove we read the real Claude Code correctly. This task has no code — it is the gate before Phase 1 is called done.

**Files:** none (findings recorded in the commit message or an issue).

- [ ] **Step 1: Capture ground truth independently**

```bash
python3 - <<'PY'
import json,glob,os,collections
out=collections.Counter(); ids=set()
for p in glob.glob(os.path.expanduser('~/.claude/projects/**/*.jsonl'),recursive=True):
    for line in open(p,errors='replace'):
        if '"type":"assistant"' not in line: continue
        try: d=json.loads(line)
        except: continue
        m=d.get('message') or {}
        u=m.get('usage'); mid=m.get('id')
        if not u or mid in ids: continue
        ids.add(mid)
        if m.get('model')=='<synthetic>': continue
        out['input']+=u.get('input_tokens',0)
        out['output']+=u.get('output_tokens',0)
        out['cache_read']+=u.get('cache_read_input_tokens',0)
        out['cache_creation']+=u.get('cache_creation_input_tokens',0)
print('distinct messages:',len(ids))
for k,v in out.items(): print(k,v)
print('billable total:',sum(out.values()))
PY
```

Record these numbers.

- [ ] **Step 2: Run the app and compare**

```bash
npx tauri dev
```

Open the popover. Confirm:
- The tray title is no longer a stale March figure.
- The dashboard's all-time token total matches Step 1's billable total (small drift is expected only if a session wrote during the run).
- **Critically:** the total is NOT roughly 4.7x Step 1's output figure. That multiple means dedup regressed.
- Settings shows a plausible "Transcripts tracked" (~1030) and zero or near-zero unreadable records.

- [ ] **Step 3: Verify incrementality**

With the app running, note the time, then in another terminal start a session (`cd /tmp && claude`), send one message, and quit it. Within ~2 s of the transcript being written, the dashboard should update without a visible full rescan, and the tray title should tick up.

- [ ] **Step 4: Verify the cache**

```bash
ls -lh ~/Library/Application\ Support/com.claudetokenusage.dev/usage-cache.v1.json
```

Expected: file exists, single-digit MB. If it is far larger, the dedup sets are not being pruned on retirement.

- [ ] **Step 5: Verify rebuild resilience**

```bash
CACHE=~/Library/Application\ Support/com.claudetokenusage.dev/usage-cache.v1.json
echo "corrupt" > "$CACHE"
```

Restart the app. It must rebuild silently (a `.corrupt` sibling appears) and show the same numbers, not an error state.

- [ ] **Step 6: Record the outcome**

```bash
git commit --allow-empty -m "test: verify Phase 1 pipeline against real transcripts

Ground-truth totals matched the dashboard; incremental refresh confirmed;
cache rebuilt cleanly from a corrupted file."
```

---

## Definition of done

- [ ] `cargo test` green, `npm test` green (41 tests), `npx vite build` succeeds.
- [ ] Dashboard totals match an independent dedup-aware count of the transcripts.
- [ ] An unchanged refresh reads zero bytes.
- [ ] A pruned transcript keeps its history in the cache.
- [ ] A corrupt cache self-heals.
- [ ] Tray title agrees with the dashboard's month figure.
- [ ] Nothing under `~/.claude` was written.

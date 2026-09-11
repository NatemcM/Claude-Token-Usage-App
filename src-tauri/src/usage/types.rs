use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

/// Bump to force a full rebuild of the cache on next load.
pub const SCHEMA_VERSION: u32 = 1;

/// Used to omit zero-valued count fields from serialized `TokenCounts`: five
/// of nine fields are usually zero, so this roughly halves the cache file.
/// `default` on the field is required so an omitted field still deserializes
/// as 0 rather than failing.
fn is_zero(v: &u64) -> bool {
    *v == 0
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCounts {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub input: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub output: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_read: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_creation: u64,
    /// Detail slice of cache_creation, not additive with it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_1h: u64,
    /// Detail slice of cache_creation, not additive with it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_5m: u64,
    /// Detail slice of output, not additive with it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub thinking: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub web_search: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
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

    pub fn is_empty(&self) -> bool {
        self.input == 0 && self.output == 0 && self.cache_read == 0
            && self.cache_creation == 0 && self.cache_1h == 0 && self.cache_5m == 0
            && self.thinking == 0 && self.web_search == 0 && self.web_fetch == 0
    }

    /// Raise every field of `self` to at least the matching field of `other`,
    /// returning the positive delta that was applied, or None if `other` adds
    /// nothing. This is how duplicate copies of one message.id are folded in:
    /// early copies are written mid-stream with partial counts, so the correct
    /// total is the per-field max, and only the delta may be added to rollups.
    pub fn raise_to(&mut self, other: &TokenCounts) -> Option<TokenCounts> {
        fn bump(cur: &mut u64, new: u64, delta: &mut u64, any: &mut bool) {
            if new > *cur {
                *delta = new - *cur;
                *cur = new;
                *any = true;
            }
        }
        let mut d = TokenCounts::default();
        let mut any = false;
        bump(&mut self.input, other.input, &mut d.input, &mut any);
        bump(&mut self.output, other.output, &mut d.output, &mut any);
        bump(&mut self.cache_read, other.cache_read, &mut d.cache_read, &mut any);
        bump(&mut self.cache_creation, other.cache_creation, &mut d.cache_creation, &mut any);
        bump(&mut self.cache_1h, other.cache_1h, &mut d.cache_1h, &mut any);
        bump(&mut self.cache_5m, other.cache_5m, &mut d.cache_5m, &mut any);
        bump(&mut self.thinking, other.thinking, &mut d.thinking, &mut any);
        bump(&mut self.web_search, other.web_search, &mut d.web_search, &mut any);
        bump(&mut self.web_fetch, other.web_fetch, &mut d.web_fetch, &mut any);
        if any { Some(d) } else { None }
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
    /// hash(message.id) -> usage credited so far. Token dedup by per-field max:
    /// a later copy contributes only its positive delta. Cleared when retired.
    pub seen: HashMap<u64, TokenCounts>,
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
    /// Messages whose totals were revised upward by a later copy. Expect this
    /// to be large (~30% of messages); it is normal, not an error.
    pub revised_messages: u64,
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
    fn raise_to_returns_only_the_positive_per_field_delta() {
        // The real case: a message first seen mid-stream with output 7, then
        // re-emitted complete with output 156. We must credit the extra 149,
        // not 156 and not 7.
        let mut credited = TokenCounts { input: 2, output: 7, cache_read: 100, ..Default::default() };
        let complete = TokenCounts { input: 2, output: 156, cache_read: 100, ..Default::default() };

        let delta = credited.raise_to(&complete).expect("a delta");
        assert_eq!(delta.output, 149);
        assert_eq!(delta.input, 0);
        assert_eq!(delta.cache_read, 0);
        // credited is now the per-field max.
        assert_eq!(credited.output, 156);
    }

    #[test]
    fn raise_to_returns_none_when_a_copy_adds_nothing() {
        let mut credited = TokenCounts { output: 156, ..Default::default() };
        // An identical copy, and a smaller one, both add nothing.
        assert!(credited.raise_to(&TokenCounts { output: 156, ..Default::default() }).is_none());
        assert!(credited.raise_to(&TokenCounts { output: 7, ..Default::default() }).is_none());
        assert_eq!(credited.output, 156, "must never be lowered");
    }

    #[test]
    fn is_empty_detects_a_zero_usage_block() {
        assert!(TokenCounts::default().is_empty());
        assert!(!TokenCounts { thinking: 1, ..Default::default() }.is_empty());
    }

    #[test]
    fn usage_cache_serde_roundtrip_preserves_entries() {
        let mut cache = UsageCache::new(420);
        let mut entry = FileEntry::default();
        entry.session.session_id = "s1".to_string();
        entry.seen.insert(7u64, TokenCounts { output: 99, ..Default::default() });
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
        let credited = e.seen.get(&7).expect("credited");
        assert_eq!(credited.output, 99);
        // The other eight fields are 0 and therefore omitted from the JSON
        // (skip_serializing_if = "is_zero"); `default` on each field must
        // still bring them back as 0, not fail deserialization.
        assert_eq!(credited.input, 0);
        assert_eq!(credited.cache_read, 0);
        assert_eq!(credited.cache_creation, 0);
        assert!(e.seen_tools.contains(&11));
        assert!(e.seen_users.contains(&13));
        assert!(e.days.contains_key("2026-09-10"));
    }
}

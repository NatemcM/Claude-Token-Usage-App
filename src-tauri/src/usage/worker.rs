use crate::config::ConfigRoots;
use crate::usage::adapter::to_stats_cache;
use crate::usage::dates::current_tz_offset_minutes;
use crate::usage::legacy::seed_from_stats_cache;
use crate::usage::scanner::{scan_once, ScanReport};
use crate::usage::store::{load, salvage_retired, save_atomic, LoadOutcome, RebuildReason};
use crate::usage::types::UsageCache;
use crate::StatsCache;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    pub malformed_lines: u64,
    /// Messages whose totals a later copy revised upward. Expect ~30%.
    pub revised_messages: u64,
    pub files_tracked: usize,
    pub files_retired: usize,
    pub last_scan_ms: u64,
    /// The last date key present in the one-time legacy seed's days, i.e. the
    /// last day the retired stats-cache.json pipeline covers. None when no
    /// legacy seed exists. Lets the UI mark that earliest range as imported
    /// history rather than presenting it as live transcript data (spec §5.4).
    pub legacy_through: Option<String>,
    /// Display form of the projects root actually in use, so the UI never has
    /// to hardcode `~/.claude/projects/` (which is wrong whenever
    /// CLAUDE_CONFIG_DIR is set).
    pub projects_root: String,
}

/// One subagent's contribution to a session. Read-only: subagents have no pid
/// of their own and cannot be acted on individually.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsage {
    pub agent_id: String,
    /// Absent on some subagent records, so genuinely optional.
    pub agent_type: Option<String>,
    pub tokens: u64,
    pub last_ts: i64,
}

/// Per-session view of the usage cache, merged across every file that belongs
/// to the session. A session's records live in its own transcript PLUS one file
/// per subagent, all sharing the parent's sessionId, so a per-file view would
/// undercount and split the activity timeline.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsage {
    pub session_id: String,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub first_ts: i64,
    /// Last record of ANY type: this is what idle age is measured from.
    pub last_ts: i64,
    pub message_count: u64,
    /// Billable total (input + output + cache read + cache creation).
    pub tokens: u64,
    pub agents: Vec<AgentUsage>,
}

/// Minimum gap between cache writes. The cache is ~9.6 MB and transcripts can
/// change many times a minute, so an ungated persist would write tens of GB a
/// day under steady activity. Losing a queued write here is cheap: `days` and
/// the file cursors always roll back together (the cursor only advances after
/// its bytes are folded into `days`), so on the next scan those bytes are
/// simply re-read and the same totals re-derived — it is re-work, not data
/// loss. `persist()` still runs unconditionally on quit and on an explicit
/// user rescan, so a crash between throttled writes loses at most this
/// window's worth of ingest work, never a whole session's history.
const MIN_PERSIST_INTERVAL: Duration = Duration::from_secs(300);

/// Owns the cache. Every mutation goes through one mutex, so the watcher, the
/// fallback poll, manual refresh and `get_stats` cannot race or double-count.
pub struct UsageWorker {
    roots: ConfigRoots,
    cache_path: PathBuf,
    cache: Mutex<UsageCache>,
    /// Set when a scan actually changed something; cleared on persist.
    dirty: AtomicBool,
    last_scan_ms: AtomicU64,
    last_persist: Mutex<Option<Instant>>,
}

impl UsageWorker {
    pub fn new(roots: ConfigRoots, cache_path: PathBuf) -> Self {
        let tz = current_tz_offset_minutes();
        let mut cache = match load(&cache_path, tz) {
            LoadOutcome::Loaded(c) => c,
            LoadOutcome::Rebuild(reason) => {
                eprintln!("[usage] rebuilding cache: {:?}", reason);
                // Retired entries describe transcripts upstream has already
                // pruned: a rebuild cannot re-derive them, so carry them over
                // when the file we still have is structurally readable.
                //
                // Salvage applies to Schema/Timezone rebuilds only. A Corrupt
                // cache is by definition unparseable by the same serde call
                // salvage_retired uses, so nothing can be recovered from it:
                // retired entries whose transcripts upstream has already
                // pruned are lost. Guarded against by save_atomic's
                // tmp+rename; a format change (one entry per line) would make
                // partial recovery possible and is tracked as Phase 2
                // follow-up.
                let retained = match reason {
                    RebuildReason::Schema | RebuildReason::Timezone => {
                        salvage_retired(&cache_path)
                    }
                    RebuildReason::Corrupt | RebuildReason::Missing => HashMap::new(),
                };
                let mut fresh = UsageCache::new(tz);
                if !retained.is_empty() {
                    eprintln!("[usage] salvaged {} retired entries", retained.len());
                    fresh.files = retained;
                }
                fresh
            }
        };
        // One-time seed of the retired stats-cache.json history.
        if cache.legacy.is_none() {
            cache.legacy = seed_from_stats_cache(&roots.stats_cache);
        }
        UsageWorker {
            roots,
            cache_path,
            cache: Mutex::new(cache),
            dirty: AtomicBool::new(false),
            last_scan_ms: AtomicU64::new(0),
            last_persist: Mutex::new(None),
        }
    }

    pub fn refresh_now(&self) -> ScanReport {
        self.refresh_with_progress(None)
    }

    pub fn refresh_with_progress(
        &self,
        progress: Option<&mut dyn FnMut(usize, usize)>,
    ) -> ScanReport {
        let started = Instant::now();
        let report = {
            let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            scan_once(&mut guard, &self.roots.projects, progress)
        };
        // `.max(1)`: a scan over a handful of small fixture files can finish
        // in well under 1ms, which would truncate to a literal 0 and violate
        // "last_scan_ms must be recorded from a real elapsed measurement, not
        // left 0". Any completed scan took SOME nonzero time, so floor at 1.
        let elapsed_ms = (started.elapsed().as_millis() as u64).max(1);
        self.last_scan_ms.store(elapsed_ms, Ordering::Relaxed);
        if report.files_read > 0 || report.files_retired > 0 {
            self.dirty.store(true, Ordering::Relaxed);
        }
        report
    }

    pub fn snapshot(&self) -> StatsCache {
        let guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        to_stats_cache(&guard)
    }

    pub fn diagnostics(&self) -> Diagnostics {
        let guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let legacy_through = guard
            .legacy
            .as_ref()
            .and_then(|l| l.days.keys().next_back().cloned());
        Diagnostics {
            malformed_lines: guard.files.values().map(|e| e.malformed_lines).sum(),
            revised_messages: guard.files.values().map(|e| e.revised_messages).sum(),
            files_tracked: guard.files.len(),
            files_retired: guard.files.values().filter(|e| e.retired).count(),
            last_scan_ms: self.last_scan_ms.load(Ordering::Relaxed),
            legacy_through,
            projects_root: self.roots.projects.display().to_string(),
        }
    }

    /// Unconditional write. Use on quit and for an explicit user rescan.
    pub fn persist(&self) -> Result<(), String> {
        let guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        save_atomic(&self.cache_path, &guard)?;
        drop(guard);
        self.dirty.store(false, Ordering::Relaxed);
        *self.last_persist.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        Ok(())
    }

    /// Write only if a scan changed something AND enough time has passed.
    /// Returns whether a write happened.
    pub fn maybe_persist(&self) -> Result<bool, String> {
        if !self.dirty.load(Ordering::Relaxed) {
            return Ok(false);
        }
        {
            let last = self.last_persist.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(t) = *last {
                if t.elapsed() < MIN_PERSIST_INTERVAL {
                    return Ok(false);
                }
            }
        }
        self.persist()?;
        Ok(true)
    }

    /// Per-session usage, keyed by session id, merged across files.
    pub fn session_usage(&self) -> HashMap<String, SessionUsage> {
        use std::collections::hash_map::Entry;

        let guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: HashMap<String, SessionUsage> = HashMap::new();
        // agent_id -> (tokens, last_ts, agent_type) per session.
        let mut agents: HashMap<String, HashMap<String, AgentUsage>> = HashMap::new();

        for entry in guard.files.values() {
            let sid = &entry.session.session_id;
            if sid.is_empty() {
                continue;
            }

            let tokens: u64 = entry
                .session
                .by_model
                .values()
                .map(|c| c.billable_total())
                .sum();

            match out.entry(sid.clone()) {
                Entry::Vacant(v) => {
                    v.insert(SessionUsage {
                        session_id: sid.clone(),
                        cwd: entry.session.cwd.clone(),
                        git_branch: entry.session.git_branch.clone(),
                        first_ts: entry.session.first_ts,
                        last_ts: entry.session.last_ts,
                        message_count: entry.session.message_count,
                        tokens,
                        agents: Vec::new(),
                    });
                }
                Entry::Occupied(mut o) => {
                    let s = o.get_mut();
                    if s.cwd.is_none() {
                        s.cwd = entry.session.cwd.clone();
                    }
                    if s.git_branch.is_none() {
                        s.git_branch = entry.session.git_branch.clone();
                    }
                    if entry.session.first_ts != 0
                        && (s.first_ts == 0 || entry.session.first_ts < s.first_ts)
                    {
                        s.first_ts = entry.session.first_ts;
                    }
                    if entry.session.last_ts > s.last_ts {
                        s.last_ts = entry.session.last_ts;
                    }
                    s.message_count += entry.session.message_count;
                    s.tokens += tokens;
                }
            }

            let per_session = agents.entry(sid.clone()).or_default();
            for (agent_id, a) in &entry.agents {
                let slot = per_session.entry(agent_id.clone()).or_insert(AgentUsage {
                    agent_id: agent_id.clone(),
                    agent_type: a.agent_type.clone(),
                    tokens: 0,
                    last_ts: 0,
                });
                if slot.agent_type.is_none() {
                    slot.agent_type = a.agent_type.clone();
                }
                slot.tokens += a.tokens.billable_total();
                if a.last_ts > slot.last_ts {
                    slot.last_ts = a.last_ts;
                }
            }
        }

        for (sid, per_session) in agents {
            if let Some(s) = out.get_mut(&sid) {
                let mut list: Vec<AgentUsage> = per_session.into_values().collect();
                // Most recently active first — that is the useful ordering when
                // asking "what is this session doing right now".
                list.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
                s.agents = list;
            }
        }

        out
    }
}

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

    /// A second, untouched transcript on a different day than `setup`'s, so
    /// tests that delete `setup`'s file don't leave the projects root
    /// entirely empty of jsonl files. Fix 1(b) deliberately treats a fully
    /// empty discovery result as an implausible transient failure rather
    /// than "everything vanished", so without this companion file the
    /// deletion in those tests would never actually retire anything.
    fn write_stable_companion(dir: &std::path::Path) {
        let path = dir.join("claude/projects/-p/companion.jsonl");
        let mut f = std::fs::File::create(&path).expect("create");
        writeln!(
            f,
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","sessionId":"s-companion","message":{{"id":"c1","model":"claude-opus-5","content":[],"usage":{{"input_tokens":1,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
        )
        .expect("write");
        f.flush().expect("flush");
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
        assert_eq!(d.revised_messages, 0);
        assert!(d.last_scan_ms > 0, "scan duration must be recorded, not left 0");
    }

    #[test]
    fn a_schema_rebuild_keeps_retired_history() {
        let dir = tempfile::tempdir().expect("tempdir");
        let worker = setup(dir.path());
        write_stable_companion(dir.path());
        worker.refresh_now();

        // Retire the transcript from `setup`, then persist that state.
        std::fs::remove_file(dir.path().join("claude/projects/-p/s.jsonl")).expect("rm");
        worker.refresh_now();
        worker.persist().expect("persist");
        assert_eq!(worker.diagnostics().files_retired, 1);
        let tokens_before = worker.snapshot();

        // Force a schema rebuild by bumping the stored schema on disk.
        let cache_path = dir.path().join("cache/usage-cache.v1.json");
        let raw = std::fs::read_to_string(&cache_path).expect("read");
        let mut v: serde_json::Value = serde_json::from_str(&raw).expect("parse");
        v["schema"] = serde_json::json!(9999);
        std::fs::write(&cache_path, v.to_string()).expect("write");

        let roots = resolve_with(Some(dir.path().join("claude")), None, None);
        let worker2 = UsageWorker::new(roots, cache_path);
        // Salvage only carries over RETIRED entries, so the still-present
        // companion file isn't in worker2's cache yet. A real startup always
        // scans immediately after constructing the worker (see setup() in
        // lib.rs), which is what repopulates it; do the same here.
        worker2.refresh_now();
        assert_eq!(
            worker2.diagnostics().files_retired, 1,
            "a rebuild must not discard history it cannot re-derive"
        );
        assert_eq!(
            worker2.snapshot().daily_model_tokens.len(),
            tokens_before.daily_model_tokens.len()
        );
    }

    #[test]
    fn maybe_persist_writes_once_then_throttles() {
        let dir = tempfile::tempdir().expect("tempdir");
        let worker = setup(dir.path());

        // Nothing scanned yet: nothing to write.
        assert!(!worker.maybe_persist().expect("no-op"), "clean cache must not write");

        worker.refresh_now();
        assert!(worker.maybe_persist().expect("first"), "a changed cache must persist");
        // Immediately after, both the dirty flag and the throttle block a write.
        assert!(!worker.maybe_persist().expect("second"), "must not rewrite immediately");
    }

    #[test]
    fn an_unchanged_rescan_does_not_mark_the_cache_dirty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let worker = setup(dir.path());
        worker.refresh_now();
        worker.persist().expect("persist");

        // Second scan reads nothing, so there is nothing new to write. This is
        // what keeps the 60s fallback poll from rewriting megabytes all day.
        let report = worker.refresh_now();
        assert_eq!(report.files_read, 0);
        assert!(!worker.maybe_persist().expect("no-op"));
    }

    #[test]
    // This documents a known limitation, not a desired outcome: a Corrupt
    // cache cannot be salvaged (see the doc comment on the salvage branch in
    // `UsageWorker::new`), so retired history is forfeited when the cache
    // file itself is corrupted. Contrast with
    // `a_schema_rebuild_keeps_retired_history`, which IS the guarantee we
    // make — salvage applies to Schema/Timezone rebuilds, never Corrupt.
    fn a_corrupt_cache_rebuilds_and_forfeits_retired_history() {
        let dir = tempfile::tempdir().expect("tempdir");
        let worker = setup(dir.path());
        write_stable_companion(dir.path());
        worker.refresh_now();

        // Retire the transcript from `setup`, then persist that state.
        std::fs::remove_file(dir.path().join("claude/projects/-p/s.jsonl")).expect("rm");
        worker.refresh_now();
        worker.persist().expect("persist");
        assert_eq!(worker.diagnostics().files_retired, 1);

        // Corrupt the cache on disk. `load()` will move it aside to
        // `usage-cache.v1.json.corrupt` and request a Corrupt rebuild.
        let cache_path = dir.path().join("cache/usage-cache.v1.json");
        std::fs::write(&cache_path, b"{ not json").expect("write");

        let roots = resolve_with(Some(dir.path().join("claude")), None, None);
        let worker2 = UsageWorker::new(roots, cache_path.clone());
        assert_eq!(
            worker2.diagnostics().files_retired, 0,
            "a Corrupt cache cannot be salvaged: history was NOT recovered"
        );
        assert!(
            cache_path.with_extension("json.corrupt").exists(),
            "the corrupt cache must be moved aside, not deleted"
        );
    }

    #[test]
    fn a_retired_file_that_reappears_with_more_content_is_unretired() {
        let dir = tempfile::tempdir().expect("tempdir");
        let worker = setup(dir.path());
        write_stable_companion(dir.path());
        worker.refresh_now();

        let path = dir.path().join("claude/projects/-p/s.jsonl");
        std::fs::remove_file(&path).expect("rm");
        worker.refresh_now();
        assert_eq!(worker.diagnostics().files_retired, 1);

        // Recreate at the same path with strictly more content than before.
        // mtime must differ for the scan to notice.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut f = std::fs::File::create(&path).expect("create");
        writeln!(
            f,
            r#"{{"type":"assistant","timestamp":"2026-09-10T08:32:59Z","sessionId":"s-1","message":{{"id":"m1","model":"claude-opus-5","content":[],"usage":{{"input_tokens":1,"output_tokens":9,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
        )
        .expect("write");
        writeln!(
            f,
            r#"{{"type":"assistant","timestamp":"2026-09-10T08:33:00Z","sessionId":"s-1","message":{{"id":"m2","model":"claude-opus-5","content":[],"usage":{{"input_tokens":1,"output_tokens":19,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
        )
        .expect("write");
        f.flush().expect("flush");

        worker.refresh_now();
        assert_eq!(
            worker.diagnostics().files_retired, 0,
            "a reappeared file must have its retired flag cleared"
        );

        let stats = worker.snapshot();
        let day = stats
            .daily_model_tokens
            .iter()
            .find(|d| d.date == "2026-09-10")
            .expect("day");
        assert_eq!(
            day.tokens_by_model["claude-opus-5"], 30,
            "tokens must reflect the reappeared file's content (10 + 20)"
        );
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

    #[test]
    fn session_usage_merges_a_session_split_across_parent_and_subagent_files() {
        use crate::usage::types::{AgentRollup, FileEntry, TokenCounts, UsageCache};

        let dir = tempfile::tempdir().expect("tempdir");
        let roots = resolve_with(Some(dir.path().join("claude")), None, None);
        let worker = UsageWorker::new(roots, dir.path().join("cache/c.json"));

        // Hand-build a cache: one session across two files, plus an unrelated one.
        {
            let mut guard = worker.cache.lock().expect("lock");
            *guard = UsageCache::new(0);

            let mut parent = FileEntry::default();
            parent.session.session_id = "s-1".to_string();
            parent.session.cwd = Some("/p/proj".to_string());
            parent.session.git_branch = Some("main".to_string());
            parent.session.first_ts = 1_000;
            parent.session.last_ts = 5_000;
            parent.session.message_count = 4;
            parent.session.by_model.insert(
                "claude-opus-5".to_string(),
                TokenCounts { input: 1, output: 2, cache_read: 3, cache_creation: 4, ..Default::default() },
            );

            let mut sub = FileEntry::default();
            sub.session.session_id = "s-1".to_string(); // same session!
            sub.session.first_ts = 2_000;
            sub.session.last_ts = 9_000;               // later than the parent
            sub.session.message_count = 3;
            sub.session.by_model.insert(
                "claude-sonnet-5".to_string(),
                TokenCounts { input: 10, output: 20, ..Default::default() },
            );
            sub.agents.insert(
                "a1".to_string(),
                AgentRollup {
                    agent_id: "a1".to_string(),
                    agent_type: Some("Explore".to_string()),
                    last_ts: 8_000,
                    tokens: TokenCounts { output: 50, ..Default::default() },
                },
            );

            let mut other = FileEntry::default();
            other.session.session_id = "s-2".to_string();
            other.session.last_ts = 7_000;

            guard.files.insert("/parent.jsonl".into(), parent);
            guard.files.insert("/parent/subagents/a.jsonl".into(), sub);
            guard.files.insert("/other.jsonl".into(), other);
        }

        let usage = worker.session_usage();
        assert_eq!(usage.len(), 2, "two distinct sessions");

        let s1 = usage.get("s-1").expect("s-1");
        assert_eq!(s1.first_ts, 1_000, "earliest across both files");
        assert_eq!(s1.last_ts, 9_000, "latest across both files");
        assert_eq!(s1.message_count, 7, "4 + 3");
        // billable totals: (1+2+3+4) + (10+20) = 10 + 30
        assert_eq!(s1.tokens, 40);
        assert_eq!(s1.cwd.as_deref(), Some("/p/proj"));
        assert_eq!(s1.git_branch.as_deref(), Some("main"));
        assert_eq!(s1.agents.len(), 1);
        assert_eq!(s1.agents[0].agent_id, "a1");
        assert_eq!(s1.agents[0].agent_type.as_deref(), Some("Explore"));
        assert_eq!(s1.agents[0].tokens, 50);
        assert_eq!(s1.agents[0].last_ts, 8_000);
    }

    #[test]
    fn session_usage_merges_one_agent_appearing_in_two_files() {
        use crate::usage::types::{AgentRollup, FileEntry, TokenCounts, UsageCache};

        let dir = tempfile::tempdir().expect("tempdir");
        let roots = resolve_with(Some(dir.path().join("claude")), None, None);
        let worker = UsageWorker::new(roots, dir.path().join("cache/c.json"));

        {
            let mut guard = worker.cache.lock().expect("lock");
            *guard = UsageCache::new(0);
            for (path, tokens, last) in [("/a.jsonl", 10u64, 100i64), ("/b.jsonl", 5, 300)] {
                let mut e = FileEntry::default();
                e.session.session_id = "s-1".to_string();
                e.agents.insert(
                    "a1".to_string(),
                    AgentRollup {
                        agent_id: "a1".to_string(),
                        agent_type: None,
                        last_ts: last,
                        tokens: TokenCounts { output: tokens, ..Default::default() },
                    },
                );
                guard.files.insert(path.into(), e);
            }
        }

        let usage = worker.session_usage();
        let agents = &usage.get("s-1").expect("s-1").agents;
        assert_eq!(agents.len(), 1, "same agent_id must merge, not duplicate");
        assert_eq!(agents[0].tokens, 15);
        assert_eq!(agents[0].last_ts, 300, "latest wins");
    }

    #[test]
    fn session_usage_skips_entries_with_no_session_id() {
        use crate::usage::types::{FileEntry, UsageCache};

        let dir = tempfile::tempdir().expect("tempdir");
        let roots = resolve_with(Some(dir.path().join("claude")), None, None);
        let worker = UsageWorker::new(roots, dir.path().join("cache/c.json"));
        {
            let mut guard = worker.cache.lock().expect("lock");
            *guard = UsageCache::new(0);
            guard.files.insert("/empty.jsonl".into(), FileEntry::default());
        }
        assert!(worker.session_usage().is_empty());
    }

    #[test]
    fn session_usage_sorts_agents_by_last_activity_descending() {
        use crate::usage::types::{AgentRollup, FileEntry, TokenCounts, UsageCache};

        let dir = tempfile::tempdir().expect("tempdir");
        let roots = resolve_with(Some(dir.path().join("claude")), None, None);
        let worker = UsageWorker::new(roots, dir.path().join("cache/c.json"));
        {
            let mut guard = worker.cache.lock().expect("lock");
            *guard = UsageCache::new(0);
            let mut e = FileEntry::default();
            e.session.session_id = "s-1".to_string();
            for (id, last) in [("old", 100i64), ("newest", 900), ("mid", 500)] {
                e.agents.insert(
                    id.to_string(),
                    AgentRollup {
                        agent_id: id.to_string(),
                        agent_type: None,
                        last_ts: last,
                        tokens: TokenCounts::default(),
                    },
                );
            }
            guard.files.insert("/a.jsonl".into(), e);
        }
        let ids: Vec<String> = worker
            .session_usage()
            .get("s-1")
            .expect("s-1")
            .agents
            .iter()
            .map(|a| a.agent_id.clone())
            .collect();
        assert_eq!(ids, vec!["newest", "mid", "old"], "most recent first");
    }
}

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
}

/// Minimum gap between cache writes. The cache is several MB and transcripts
/// change many times per second, so an ungated persist would write gigabytes
/// a day.
const MIN_PERSIST_INTERVAL: Duration = Duration::from_secs(30);

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
        Diagnostics {
            malformed_lines: guard.files.values().map(|e| e.malformed_lines).sum(),
            revised_messages: guard.files.values().map(|e| e.revised_messages).sum(),
            files_tracked: guard.files.len(),
            files_retired: guard.files.values().filter(|e| e.retired).count(),
            last_scan_ms: self.last_scan_ms.load(Ordering::Relaxed),
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
        worker.refresh_now();

        // Retire the only transcript, then persist that state.
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
        worker.refresh_now();

        // Retire the only transcript, then persist that state.
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
}

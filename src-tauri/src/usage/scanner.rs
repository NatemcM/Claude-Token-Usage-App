use crate::usage::cursor::{decide, read_meta, stream_lines_from, ScanAction};
use crate::usage::discovery::discover_transcripts;
use crate::usage::ingest::{ingest_line, IngestStats};
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
        let mut action = decide(cache.files.get(&path).map(|e| &e.cursor), meta.as_ref());
        // A retired entry has had its dedup sets cleared. An incremental Delta would
        // then treat pre-cursor re-emissions as first sightings and double-count, and
        // a Skip would leave the sets empty for the next append to do the same. Any
        // resurrection must re-ingest the whole file.
        if matches!(action, ScanAction::Skip | ScanAction::Delta { .. })
            && cache.files.get(&path).map(|e| e.retired).unwrap_or(false)
        {
            action = ScanAction::Full;
        }

        match action {
            ScanAction::Skip => {}
            // ScanAction::Retire is produced only when a cursor exists but
            // `meta` is absent. Every path in this loop came from
            // `discover_transcripts`, i.e. it exists on disk right now, so
            // this arm is not reachable here; retirement of files discovery
            // no longer finds is handled exclusively by the trailing loop
            // below. Folded into the wildcard rather than given its own arm.
            ScanAction::Full => {
                // Ingest into a FRESH entry and install it only on success, so
                // a transient read error cannot destroy this file's history.
                let mut work = FileEntry::default();
                let mut stats = IngestStats::default();
                let result = stream_lines_from(&path, 0, |line| {
                    ingest_line(&mut work, line, tz, &mut stats)
                });
                match result {
                    Ok(consumed) => {
                        if let Some(m) = meta.as_ref() {
                            work.cursor.offset = consumed;
                            work.cursor.size = m.size;
                            work.cursor.mtime_ms = m.mtime_ms;
                            work.cursor.inode = m.inode;
                        }
                        report.malformed += stats.malformed;
                        report.deduped += stats.deduped;
                        report.bytes_read += consumed;
                        report.files_read += 1;
                        cache.files.insert(path.clone(), work);
                    }
                    Err(e) => eprintln!("[usage] full ingest of {:?} failed: {}", path, e),
                }
            }
            ScanAction::Delta { from } => {
                // Safe to mutate in place: the cursor advances only on success,
                // so a mid-stream failure just re-reads the same bytes next
                // pass, and dedup makes re-application idempotent.
                let Some(entry) = cache.files.get_mut(&path) else {
                    continue;
                };
                let mut stats = IngestStats::default();
                let result = stream_lines_from(&path, from, |line| {
                    ingest_line(entry, line, tz, &mut stats)
                });
                match result {
                    Ok(consumed) => {
                        if let Some(m) = meta.as_ref() {
                            entry.cursor.offset = from + consumed;
                            entry.cursor.size = m.size;
                            entry.cursor.mtime_ms = m.mtime_ms;
                            entry.cursor.inode = m.inode;
                        }
                        entry.retired = false;
                        report.malformed += stats.malformed;
                        report.deduped += stats.deduped;
                        report.bytes_read += consumed;
                        report.files_read += 1;
                    }
                    Err(e) => eprintln!("[usage] delta ingest of {:?} failed: {}", path, e),
                }
            }
            _ => {}
        }

        if let Some(cb) = progress.as_mut() {
            cb(i + 1, total);
        }
    }

    // A read_dir failure anywhere under projects/ yields an empty list. Retiring
    // every tracked file on that basis would clear all dedup sets and set up a
    // double-count on the next append, so treat "discovered nothing while
    // tracking something" as a transient failure rather than mass deletion.
    let discovery_plausible = total != 0 || cache.files.is_empty();

    // Retire tracked files that no longer exist upstream. Their day rollups
    // stay; only the dedup sets are dropped to bound cache growth.
    if discovery_plausible {
        for (path, entry) in cache.files.iter_mut() {
            if !present.contains(path) && !entry.retired {
                entry.retired = true;
                entry.seen.clear();
                entry.seen_tools.clear();
                entry.seen_users.clear();
                report.files_retired += 1;
            }
        }
    }

    report
}

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
        // A second, untouched file so the projects root is not left totally
        // empty by the deletion below: an empty discovery result is treated
        // (correctly, per Fix 1(b)) as an implausible transient failure
        // rather than "every file vanished", which would otherwise suppress
        // retirement here too.
        write_transcript(dir.path(), "-p/other.jsonl", &[line("other", 1)]);

        let mut cache = UsageCache::new(0);
        scan_once(&mut cache, dir.path(), None);
        std::fs::remove_file(&path).expect("remove");

        let report = scan_once(&mut cache, dir.path(), None);
        assert_eq!(report.files_retired, 1);

        let entry = cache.files.get(&path).expect("entry still tracked");
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
    fn a_resurrected_file_reingests_instead_of_skipping() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_transcript(dir.path(), "-p/s.jsonl", &[line("m1", 10)]);
        // A second, untouched file keeps discovery from ever looking
        // "implausibly empty" (Fix 1(b)) while the first file is moved away
        // below, so this test exercises the Fix 1(a) Skip/Delta guard
        // specifically rather than the (b) empty-discovery guard.
        write_transcript(dir.path(), "-p/other.jsonl", &[line("other", 1)]);

        let mut cache = UsageCache::new(0);
        scan_once(&mut cache, dir.path(), None);
        {
            let entry = cache.files.get(&path).expect("entry");
            assert!(!entry.seen.is_empty(), "sanity: seen populated after first ingest");
        }

        // Move the file out from under the root: discovery no longer finds
        // it, so the trailing loop legitimately retires it and clears its
        // dedup sets, mirroring what the mass-retire bug also does (just via
        // a different trigger).
        let elsewhere = dir.path().join("elsewhere.jsonl");
        std::fs::rename(&path, &elsewhere).expect("rename out");
        let report = scan_once(&mut cache, dir.path(), None);
        assert_eq!(report.files_retired, 1);
        {
            let entry = cache.files.get(&path).expect("entry");
            assert!(entry.retired);
            assert!(entry.seen.is_empty(), "dedup set must be cleared on retirement");
        }

        // Restore it at the SAME path. A rename preserves inode, size and
        // mtime, so without the Fix 1(a) guard `decide` would return Skip
        // here and the entry would stay stuck with an empty `seen`.
        std::fs::rename(&elsewhere, &path).expect("rename back");
        scan_once(&mut cache, dir.path(), None);

        let entry = cache.files.get(&path).expect("entry");
        assert!(!entry.retired, "must be un-retired");
        assert!(
            !entry.seen.is_empty(),
            "a retired entry must fully re-ingest, not Skip"
        );
        assert_eq!(
            entry.days["2026-09-10"].by_model["claude-opus-5"].output, 10,
            "re-ingest of identical content must not double the total"
        );
    }

    #[test]
    fn an_empty_discovery_does_not_retire_tracked_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_transcript(dir.path(), "-p/s.jsonl", &[line("m1", 10)]);

        let mut cache = UsageCache::new(0);
        scan_once(&mut cache, dir.path(), None);

        // A projects root that does not exist at all makes
        // `discover_transcripts` return an empty list, exactly like a
        // transient read_dir failure would. That must not be mistaken for
        // "every tracked file vanished".
        let missing_root = dir.path().join("does-not-exist");
        let report = scan_once(&mut cache, &missing_root, None);

        assert_eq!(report.files_retired, 0);
        let entry = cache.files.values().next().expect("entry");
        assert!(!entry.retired, "must not be mass-retired");
        assert!(!entry.seen.is_empty(), "dedup set must survive an implausible empty discovery");
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

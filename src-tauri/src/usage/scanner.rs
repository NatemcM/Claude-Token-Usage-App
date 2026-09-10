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
        let action = decide(cache.files.get(&path).map(|e| &e.cursor), meta.as_ref());

        match action {
            ScanAction::Retire => {}
            ScanAction::Skip => {
                // A retired file that reappeared unchanged: clear the flag so
                // diagnostics do not drift.
                if let Some(entry) = cache.files.get_mut(&path) {
                    entry.retired = false;
                }
            }
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

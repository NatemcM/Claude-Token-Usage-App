use crate::usage::types::FileCursor;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
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

/// Stream complete lines from `offset` to EOF, calling `on_line` for each and
/// returning the number of bytes consumed. A trailing partial line (a live
/// session mid-write) is withheld so it is re-read intact next pass.
///
/// Only one line is buffered at a time: the largest observed transcript is
/// 95 MB, and holding it plus a UTF-8 copy would peak near 190 MB.
pub fn stream_lines_from(
    path: &Path,
    offset: u64,
    mut on_line: impl FnMut(&str),
) -> Result<u64, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {:?}: {}", path, e))?;
    let mut reader = BufReader::new(file);
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|e| format!("seek {:?}: {}", path, e))?;

    let mut consumed: u64 = 0;
    let mut buf: Vec<u8> = Vec::new();

    loop {
        buf.clear();
        let read = reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| format!("read {:?}: {}", path, e))?;
        if read == 0 {
            break; // EOF
        }
        if buf.last() != Some(&b'\n') {
            break; // partial trailing line: withhold it
        }
        consumed += read as u64;
        // Lossy so one corrupt byte cannot abort the whole pass; the line is
        // then very likely counted as malformed downstream, which is correct.
        let line = String::from_utf8_lossy(&buf[..read - 1]);
        on_line(line.as_ref());
    }

    Ok(consumed)
}

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
    fn streams_only_lines_after_the_offset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.jsonl");
        let mut f = std::fs::File::create(&path).expect("create");
        writeln!(f, "{{\"a\":1}}").expect("write");
        writeln!(f, "{{\"b\":2}}").expect("write");
        f.flush().expect("flush");

        let mut got = Vec::new();
        // First line is 8 bytes including its newline.
        let consumed = stream_lines_from(&path, 8, |l| got.push(l.to_string())).expect("read");
        assert_eq!(got, vec!["{\"b\":2}".to_string()]);
        assert_eq!(consumed, 8);
    }

    #[test]
    fn streams_lines_one_at_a_time_in_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.jsonl");
        let mut f = std::fs::File::create(&path).expect("create");
        writeln!(f, "one").expect("write");
        writeln!(f, "two").expect("write");
        writeln!(f, "three").expect("write");
        f.flush().expect("flush");

        let mut got = Vec::new();
        let consumed = stream_lines_from(&path, 0, |l| got.push(l.to_string())).expect("read");
        assert_eq!(got, vec!["one".to_string(), "two".to_string(), "three".to_string()]);
        assert_eq!(consumed, 14); // 4 + 4 + 6
    }

    #[test]
    fn withholds_a_half_written_trailing_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.jsonl");
        let mut f = std::fs::File::create(&path).expect("create");
        write!(f, "{{\"a\":1}}\n{{\"b\":par").expect("write");
        f.flush().expect("flush");

        let mut got = Vec::new();
        let consumed = stream_lines_from(&path, 0, |l| got.push(l.to_string())).expect("read");
        assert_eq!(got, vec!["{\"a\":1}".to_string()], "partial line must be withheld");
        assert_eq!(consumed, 8, "offset must not advance past the partial line");
    }

    #[test]
    fn streaming_a_file_with_no_complete_line_consumes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.jsonl");
        std::fs::write(&path, b"{\"a\":partial").expect("write");

        let mut got = Vec::new();
        let consumed = stream_lines_from(&path, 0, |l| got.push(l.to_string())).expect("read");
        assert!(got.is_empty());
        assert_eq!(consumed, 0);
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

use crate::usage::types::{FileEntry, UsageCache, SCHEMA_VERSION};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

/// Recover the entries a rebuild cannot re-derive: those whose source
/// transcript has been pruned upstream. Called before discarding a cache on a
/// schema or timezone rebuild. Best-effort by design — a corrupt cache yields
/// nothing, which is the same position we would be in without it.
///
/// Note the day keys of salvaged entries were computed under the OLD timezone
/// offset. Keeping a slightly mis-bucketed month of history beats deleting it.
pub fn salvage_retired(path: &Path) -> HashMap<PathBuf, FileEntry> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    // Deserialize leniently: we only need the retired entries, and the schema
    // that failed validation may still parse structurally.
    let Ok(cache) = serde_json::from_str::<UsageCache>(&contents) else {
        return HashMap::new();
    };
    cache
        .files
        .into_iter()
        .filter(|(_, e)| e.retired)
        .collect()
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
    fn a_rebuild_preserves_retired_history_it_cannot_re_derive() {
        // Retired entries describe transcripts upstream has already deleted.
        // A schema bump or timezone change must NOT discard them, or the
        // cache's whole purpose as a long-term record is defeated.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("usage-cache.v1.json");

        let mut c = UsageCache::new(420);
        let mut live = FileEntry::default();
        live.session.session_id = "live".to_string();
        let mut gone = FileEntry::default();
        gone.session.session_id = "gone".to_string();
        gone.retired = true;
        gone.days.insert("2026-08-01".to_string(), Default::default());
        c.files.insert("/live.jsonl".into(), live);
        c.files.insert("/gone.jsonl".into(), gone);
        c.schema = SCHEMA_VERSION + 1; // force a schema rebuild
        std::fs::write(&path, serde_json::to_string(&c).expect("ser")).expect("write");

        let salvaged = salvage_retired(&path);
        assert_eq!(salvaged.len(), 1, "only the retired entry is salvageable");
        let e = salvaged.get(std::path::Path::new("/gone.jsonl")).expect("retired entry");
        assert!(e.retired);
        assert!(e.days.contains_key("2026-08-01"));
    }

    #[test]
    fn salvage_returns_empty_for_a_corrupt_or_absent_cache() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(salvage_retired(&dir.path().join("absent.json")).is_empty());
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, b"{ nope").expect("write");
        assert!(salvage_retired(&bad).is_empty());
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

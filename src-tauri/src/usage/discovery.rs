use std::path::{Path, PathBuf};

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

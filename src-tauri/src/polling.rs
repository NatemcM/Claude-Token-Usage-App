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
    let report = worker.refresh_now();
    // maybe_persist writes only when a scan changed something and the throttle
    // window has passed. Persisting unconditionally here would write several MB
    // on every 60s fallback tick — gigabytes a day at idle.
    if let Err(e) = worker.maybe_persist() {
        eprintln!("[polling] could not persist cache: {}", e);
    }
    if report.files_read > 0 || report.files_retired > 0 {
        crate::update_tray_from_worker(app);
    }
}

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
    fn ignores_our_own_cache_file() {
        // The cache lives outside ~/.claude, but be explicit: a watcher that
        // reacted to its own writes would spin forever.
        let paths = vec![PathBuf::from(
            "/Users/me/Library/Application Support/com.claudetokenusage.dev/usage-cache.v1.json",
        )];
        assert!(!should_react(&paths));
    }

    #[test]
    fn debounce_is_long_enough_to_coalesce_bursts() {
        // Five live sessions write several times per second; anything under
        // ~500ms would let a burst trigger repeated multi-MB cache writes.
        assert!(DEBOUNCE >= std::time::Duration::from_millis(500));
        assert!(DEBOUNCE <= std::time::Duration::from_secs(3));
    }
}

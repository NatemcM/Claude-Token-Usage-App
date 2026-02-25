use notify::{Watcher, RecursiveMode, Event, EventKind};
use tauri::AppHandle;
use std::sync::mpsc;
use std::time::Duration;

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let stats_path = crate::stats_cache_path();
        let watch_dir = stats_path.parent().unwrap().to_path_buf();

        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();

        let mut watcher = notify::recommended_watcher(tx)
            .expect("Failed to create file watcher");

        watcher
            .watch(&watch_dir, RecursiveMode::NonRecursive)
            .expect("Failed to watch ~/.claude directory");

        // Keep watcher alive and process events
        loop {
            match rx.recv_timeout(Duration::from_secs(60)) {
                Ok(Ok(event)) => {
                    // Only react to modifications of the stats file
                    let is_stats_file = event.paths.iter().any(|p| {
                        p.file_name()
                            .map(|n| n == "stats-cache.json")
                            .unwrap_or(false)
                    });

                    if is_stats_file {
                        if let EventKind::Modify(_) | EventKind::Create(_) = event.kind {
                            crate::update_tray_from_stats(&app);
                        }
                    }
                }
                Ok(Err(e)) => {
                    eprintln!("[polling] Watch error: {}", e);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Periodic refresh even without file changes
                    crate::update_tray_from_stats(&app);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
            }
        }
    });
}

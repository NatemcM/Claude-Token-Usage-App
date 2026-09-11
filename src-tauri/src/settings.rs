use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    /// Append the live session count to the menu bar title.
    #[serde(default = "default_true")]
    pub tray_show_sessions: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            tray_show_sessions: true,
        }
    }
}

/// Never fails: a missing or unreadable settings file means defaults, because
/// a broken preference must not stop the app from starting.
pub fn load(path: &Path) -> AppSettings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Wrapper so Tauri's `TypeId`-keyed state cannot collide with anything else
/// that manages a bare `PathBuf`.
pub struct SettingsPath(pub std::path::PathBuf);

pub fn save(path: &Path, settings: &AppSettings) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {parent:?}: {e}"))?;
    }
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| format!("write {path:?}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_showing_the_session_count() {
        assert!(AppSettings::default().tray_show_sessions);
    }

    #[test]
    fn missing_file_yields_defaults_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let s = load(&dir.path().join("absent.json"));
        assert!(s.tray_show_sessions);
    }

    #[test]
    fn corrupt_file_yields_defaults_rather_than_failing_startup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("settings.json");
        std::fs::write(&p, b"{ not json").expect("write");
        assert!(load(&p).tray_show_sessions);
    }

    #[test]
    fn saves_and_reloads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("nested/settings.json");
        save(&p, &AppSettings { tray_show_sessions: false }).expect("save");
        assert!(!load(&p).tray_show_sessions);
    }
}

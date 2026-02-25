use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};
use tauri_plugin_positioner::{Position, WindowExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

mod polling;

// --- Stats Cache Types (matches ~/.claude/stats-cache.json) ---

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StatsCache {
    pub version: u32,
    pub last_computed_date: String,
    pub daily_activity: Vec<DailyActivity>,
    pub daily_model_tokens: Vec<DailyModelTokens>,
    pub model_usage: HashMap<String, ModelUsage>,
    pub total_sessions: u64,
    pub total_messages: u64,
    pub longest_session: Option<LongestSession>,
    pub first_session_date: Option<String>,
    pub hour_counts: Option<HashMap<String, u64>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DailyActivity {
    pub date: String,
    pub message_count: u64,
    pub session_count: u64,
    pub tool_call_count: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DailyModelTokens {
    pub date: String,
    pub tokens_by_model: HashMap<String, u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub web_search_requests: u64,
    #[serde(default)]
    pub cost_usd: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LongestSession {
    pub session_id: String,
    pub duration: u64,
    pub message_count: u64,
    pub timestamp: String,
}

// --- Helpers ---

pub fn stats_cache_path() -> PathBuf {
    let home = dirs::home_dir().expect("Could not find home directory");
    home.join(".claude").join("stats-cache.json")
}

pub fn read_stats() -> Result<StatsCache, String> {
    let path = stats_cache_path();
    let contents = std::fs::read_to_string(&path)
        .map_err(|e| format!("Could not read stats file at {:?}: {}", path, e))?;
    serde_json::from_str(&contents)
        .map_err(|e| format!("Could not parse stats file: {}", e))
}

pub fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000_000 {
        format!("{:.1}B", tokens as f64 / 1_000_000_000.0)
    } else if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

pub fn current_month_prefix() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let days = now / 86400;
    let mut y = 1970i32;
    let mut remaining = days as i32;
    loop {
        let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
        let days_in_year = if leap { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];
    let mut m = 1usize;
    for &md in &month_days {
        if remaining < md {
            break;
        }
        remaining -= md;
        m += 1;
    }
    format!("{:04}-{:02}", y, m)
}

pub fn current_month_tokens(stats: &StatsCache) -> u64 {
    let prefix = current_month_prefix();
    stats
        .daily_model_tokens
        .iter()
        .filter(|d| d.date.starts_with(&prefix))
        .flat_map(|d| d.tokens_by_model.values())
        .sum()
}

pub fn update_tray_from_stats(app: &AppHandle) {
    if let Ok(stats) = read_stats() {
        let month_tokens = current_month_tokens(&stats);
        let title = format_tokens(month_tokens);
        if let Some(tray) = app.tray_by_id("main-tray") {
            let _ = tray.set_title(Some(&title));
        }
        // Notify frontend
        let _ = app.emit("stats-updated", ());
    }
}

// --- Tauri Commands ---

#[tauri::command]
async fn get_stats() -> Result<StatsCache, String> {
    read_stats()
}

#[tauri::command]
async fn update_tray_title(app: AppHandle, title: String) -> Result<(), String> {
    if let Some(tray) = app.tray_by_id("main-tray") {
        tray.set_title(Some(&title))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// --- Tray & Window Setup ---

fn toggle_popover(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            let _ = window.as_ref().window().move_window(Position::TrayCenter);
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

// --- App Entry Point ---

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_positioner::init())
        .invoke_handler(tauri::generate_handler![
            get_stats,
            update_tray_title,
        ])
        .setup(|app| {
            // Create tray icon from dedicated template image
            let tray_icon = tauri::image::Image::from_bytes(
                include_bytes!("../icons/tray-icon.png"),
            )?;

            let _tray = TrayIconBuilder::with_id("main-tray")
                .tooltip("Claude Token Usage")
                .title("---")
                .icon_as_template(true)
                .icon(tray_icon)
                .on_tray_icon_event(|tray, event| {
                    tauri_plugin_positioner::on_tray_event(tray.app_handle(), &event);

                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle_popover(tray.app_handle());
                    }
                })
                .build(app)?;

            // Hide window when it loses focus
            if let Some(window) = app.get_webview_window("main") {
                let window_clone = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::Focused(false) = event {
                        let _ = window_clone.hide();
                    }
                });
            }

            // Set initial tray title
            let handle = app.handle().clone();
            update_tray_from_stats(&handle);

            // Watch stats file for changes
            polling::start(handle);

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // Hide dock icon - tray only app
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    app.run(|_app_handle, _event| {});
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_model_usage(input: u64, output: u64, cache_read: u64, cache_create: u64) -> ModelUsage {
        ModelUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: cache_read,
            cache_creation_input_tokens: cache_create,
            web_search_requests: 0,
            cost_usd: 0.0,
        }
    }

    fn make_stats(
        daily_model_tokens: Vec<DailyModelTokens>,
        model_usage: HashMap<String, ModelUsage>,
    ) -> StatsCache {
        StatsCache {
            version: 1,
            last_computed_date: "2026-02-25".to_string(),
            daily_activity: vec![],
            daily_model_tokens,
            model_usage,
            total_sessions: 0,
            total_messages: 0,
            longest_session: None,
            first_session_date: None,
            hour_counts: None,
        }
    }

    // --- format_tokens ---

    #[test]
    fn format_tokens_zero() {
        assert_eq!(format_tokens(0), "0");
    }

    #[test]
    fn format_tokens_under_thousand() {
        assert_eq!(format_tokens(1), "1");
        assert_eq!(format_tokens(999), "999");
    }

    #[test]
    fn format_tokens_thousands() {
        assert_eq!(format_tokens(1000), "1.0K");
        assert_eq!(format_tokens(1500), "1.5K");
        assert_eq!(format_tokens(999_999), "1000.0K");
    }

    #[test]
    fn format_tokens_millions() {
        assert_eq!(format_tokens(1_000_000), "1.0M");
        assert_eq!(format_tokens(2_500_000), "2.5M");
        assert_eq!(format_tokens(999_999_999), "1000.0M");
    }

    #[test]
    fn format_tokens_billions() {
        assert_eq!(format_tokens(1_000_000_000), "1.0B");
        assert_eq!(format_tokens(3_700_000_000), "3.7B");
    }

    // --- current_month_prefix ---

    #[test]
    fn current_month_prefix_format() {
        let prefix = current_month_prefix();
        assert_eq!(prefix.len(), 7);
        assert_eq!(&prefix[4..5], "-");
        let year: i32 = prefix[..4].parse().unwrap();
        assert!(year >= 2024 && year <= 2030);
        let month: u32 = prefix[5..7].parse().unwrap();
        assert!((1..=12).contains(&month));
    }

    // --- current_month_tokens ---

    #[test]
    fn current_month_tokens_empty() {
        let stats = make_stats(vec![], HashMap::new());
        assert_eq!(current_month_tokens(&stats), 0);
    }

    #[test]
    fn current_month_tokens_filters_by_month() {
        let prefix = current_month_prefix();
        let mut tokens = HashMap::new();
        tokens.insert("claude-opus-4-6".to_string(), 5000u64);
        tokens.insert("claude-sonnet-4-5".to_string(), 3000u64);

        let mut other_tokens = HashMap::new();
        other_tokens.insert("claude-opus-4-6".to_string(), 9999u64);

        let stats = make_stats(
            vec![
                DailyModelTokens {
                    date: format!("{}-15", prefix),
                    tokens_by_model: tokens,
                },
                DailyModelTokens {
                    date: "2020-01-01".to_string(),
                    tokens_by_model: other_tokens,
                },
            ],
            HashMap::new(),
        );

        assert_eq!(current_month_tokens(&stats), 8000);
    }

    // --- Serde round-trip ---

    #[test]
    fn stats_cache_serde_roundtrip() {
        let mut model_usage = HashMap::new();
        model_usage.insert(
            "claude-opus-4-6".to_string(),
            make_model_usage(1000, 2000, 500, 300),
        );

        let stats = make_stats(vec![], model_usage);
        let json = serde_json::to_string(&stats).unwrap();
        let parsed: StatsCache = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.last_computed_date, "2026-02-25");
        let usage = &parsed.model_usage["claude-opus-4-6"];
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.output_tokens, 2000);
        assert_eq!(usage.cache_read_input_tokens, 500);
        assert_eq!(usage.cache_creation_input_tokens, 300);
    }

    // --- JSON parsing ---

    #[test]
    fn parse_valid_stats_json() {
        let json = r#"{
            "version": 1,
            "lastComputedDate": "2026-02-25",
            "dailyActivity": [],
            "dailyModelTokens": [],
            "modelUsage": {
                "claude-opus-4-6": {
                    "inputTokens": 100,
                    "outputTokens": 200,
                    "cacheReadInputTokens": 50,
                    "cacheCreationInputTokens": 25,
                    "webSearchRequests": 0,
                    "costUsd": 0.01
                }
            },
            "totalSessions": 5,
            "totalMessages": 42,
            "longestSession": null,
            "firstSessionDate": "2026-01-01",
            "hourCounts": null
        }"#;

        let parsed: StatsCache = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.total_sessions, 5);
        assert_eq!(parsed.total_messages, 42);
        assert_eq!(parsed.model_usage.len(), 1);

        let opus = &parsed.model_usage["claude-opus-4-6"];
        assert_eq!(opus.input_tokens, 100);
        assert_eq!(opus.output_tokens, 200);
    }

    #[test]
    fn reject_invalid_json() {
        let result: Result<StatsCache, _> = serde_json::from_str("not valid json");
        assert!(result.is_err());
    }

    #[test]
    fn parse_stats_from_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("stats-cache.json");
        let mut file = std::fs::File::create(&file_path).unwrap();
        write!(
            file,
            r#"{{
                "version": 1,
                "lastComputedDate": "2026-02-25",
                "dailyActivity": [],
                "dailyModelTokens": [],
                "modelUsage": {{}},
                "totalSessions": 3,
                "totalMessages": 10,
                "longestSession": null,
                "firstSessionDate": null,
                "hourCounts": null
            }}"#
        )
        .unwrap();

        let contents = std::fs::read_to_string(&file_path).unwrap();
        let parsed: StatsCache = serde_json::from_str(&contents).unwrap();
        assert_eq!(parsed.total_sessions, 3);
        assert_eq!(parsed.total_messages, 10);
        assert!(parsed.model_usage.is_empty());
    }
}

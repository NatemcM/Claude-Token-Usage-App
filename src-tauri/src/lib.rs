use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};
use tauri_plugin_positioner::{Position, WindowExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

mod polling;
mod usage;
mod config;
mod sessions;

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
    usage::dates::local_month_prefix(usage::dates::now_ms())
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

pub fn update_tray_from_worker(app: &AppHandle) {
    let worker = match app.try_state::<std::sync::Arc<usage::worker::UsageWorker>>() {
        Some(w) => w.inner().clone(),
        None => return,
    };
    let stats = worker.snapshot();
    let month_tokens = current_month_tokens(&stats);
    let title = format_tokens(month_tokens);
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_title(Some(&title));
    }
    let _ = app.emit("stats-updated", ());
}

// --- Tauri Commands ---

#[tauri::command]
async fn get_stats(worker: tauri::State<'_, std::sync::Arc<usage::worker::UsageWorker>>) -> Result<StatsCache, String> {
    Ok(worker.snapshot())
}

#[tauri::command]
async fn get_diagnostics(
    worker: tauri::State<'_, std::sync::Arc<usage::worker::UsageWorker>>,
) -> Result<usage::worker::Diagnostics, String> {
    Ok(worker.diagnostics())
}

#[tauri::command]
async fn refresh_usage(
    worker: tauri::State<'_, std::sync::Arc<usage::worker::UsageWorker>>,
) -> Result<(), String> {
    let worker = worker.inner().clone();
    // A full rescan plus a ~10 MB write would otherwise block a tokio worker
    // thread for the duration; run it off the async runtime instead.
    tauri::async_runtime::spawn_blocking(move || {
        worker.refresh_now();
        // Explicit user action: write immediately rather than waiting for the
        // throttle window.
        worker.persist()
    })
    .await
    .map_err(|e| format!("refresh_usage task panicked: {}", e))?
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
            get_diagnostics,
            refresh_usage,
            update_tray_title,
        ])
        .setup(|app| {
            let roots = config::resolve(None);
            let cache_path = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("app data dir: {}", e))?
                .join("usage-cache.v1.json");
            let worker = std::sync::Arc::new(usage::worker::UsageWorker::new(roots, cache_path));
            app.manage(worker.clone());

            // First pass on a background thread so app startup is not blocked.
            // Measured: a full 793 MB pass takes seconds. Note the worker mutex
            // IS held for that pass, so a popover opened during it waits for
            // the scan to finish rather than showing a partial figure.
            {
                let handle = app.handle().clone();
                let worker = worker.clone();
                std::thread::spawn(move || {
                    let mut cb = |done: usize, total: usize| {
                        let _ = handle.emit("usage-progress", (done, total));
                    };
                    worker.refresh_with_progress(Some(&mut cb));
                    let _ = worker.persist();
                    update_tray_from_worker(&handle);
                });
            }

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

            // Initial tray title is set by the spawned first-pass thread above
            // once its scan completes (it calls update_tray_from_worker
            // itself). A synchronous call here would block the Tauri event
            // loop on the worker mutex until that ~800 MB first pass finishes.
            let handle = app.handle().clone();

            // Watch stats file for changes
            polling::start(handle, config::resolve(None));

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // Hide dock icon - tray only app
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(worker) =
                app_handle.try_state::<std::sync::Arc<usage::worker::UsageWorker>>()
            {
                let _ = worker.inner().persist();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn current_month_prefix_matches_local_time_not_utc() {
        // The hand-rolled UTC epoch-day math disagreed with the frontend's
        // local-time month for the first hours of each month east of UTC.
        let expected = crate::usage::dates::local_month_prefix(
            crate::usage::dates::now_ms(),
        );
        assert_eq!(current_month_prefix(), expected);
    }

    #[test]
    fn current_month_prefix_is_well_formed() {
        let p = current_month_prefix();
        assert_eq!(p.len(), 7, "expected YYYY-MM, got {}", p);
        assert_eq!(&p[4..5], "-");
        let year: i32 = p[0..4].parse().expect("year");
        let month: u32 = p[5..7].parse().expect("month");
        assert!(year >= 2026, "year was {}", year);
        assert!((1..=12).contains(&month), "month was {}", month);
    }

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
}

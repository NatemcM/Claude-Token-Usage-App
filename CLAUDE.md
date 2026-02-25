# Claude Code Agent Guide

This document helps LLM agents understand and work on this project.

## Overview

macOS menu bar app that shows Claude Code token usage by reading `~/.claude/stats-cache.json`. No API keys or network requests - purely local data.

## Architecture

**Rust backend** (`src-tauri/src/`) handles:
- System tray icon with token count title
- Popover window creation and positioning
- Reading/parsing the stats JSON file
- File watching for live updates
- Exposing `get_stats` and `update_tray_title` as Tauri commands

**Svelte frontend** (`src/`) handles:
- Dashboard UI with token summary, charts, model breakdown, activity stats
- Calls Rust backend via `invoke()` from `@tauri-apps/api/core`
- Listens for `"stats-updated"` events emitted by the backend

## Key Files

- `src-tauri/src/lib.rs` - Core backend: types, stats reading, tray icon, window management, Tauri commands
- `src-tauri/src/polling.rs` - File watcher (notify crate) + 60s fallback poll
- `src/components/Dashboard.svelte` - Main view, computes derived stats from raw data
- `src/components/DailyChart.svelte` - ECharts bar chart (uses tree-shaken imports from `echarts/core`)
- `src/lib/types.ts` - TypeScript interfaces matching the JSON structure
- `src-tauri/tauri.conf.json` - Window config (transparent, no decorations, always on top)

## Data Source

`~/.claude/stats-cache.json` structure:
- `dailyActivity[]` - Per-day message count, session count, tool call count
- `dailyModelTokens[]` - Per-day token counts broken down by model
- `modelUsage{}` - All-time per-model: inputTokens, outputTokens, cacheReadInputTokens, cacheCreationInputTokens
- `totalSessions`, `totalMessages`, `longestSession`, `firstSessionDate`
- `hourCounts{}` - Session start times by hour

The file is written by Claude Code when sessions end. The `lastComputedDate` field indicates the most recent data.

## Build Commands

```bash
npm install              # Install frontend deps
npx tauri dev            # Dev mode with hot-reload
npx tauri build --bundles app  # Production build
npx vite build           # Frontend-only build (for debugging)
```

## Known Issues & Gotchas

- **Vite build hangs with `@tailwindcss/vite` plugin** - Use `@tailwindcss/postcss` instead (see `postcss.config.js`)
- **Tailwind v4 source scanning** - Must use `@import "tailwindcss" source("../src")` in `app.css` to prevent scanning node_modules
- **ECharts bundle size** - Use tree-shaken imports from `echarts/core`, `echarts/charts`, etc. Do NOT use `import * as echarts from "echarts"`
- **Tray icon** - Must be black on transparent PNG (8-bit RGBA). macOS template icons use alpha channel for shape. Set `icon_as_template(true)`
- **Window transparency** - Requires `"macOSPrivateApi": true` in tauri.conf.json and `"transparent": true` on the window config. Cannot use `.transparent()` on `WebviewWindowBuilder` in Tauri v2
- **Rust `time` crate** - If build fails requiring Rust 1.88+, pin with `cargo update time@0.3.47 --precise 0.3.41`
- **DMG bundling may fail** - Use `--bundles app` to skip DMG and build just the `.app`

## Conventions

- **Svelte 5 runes**: Uses `$state`, `$derived`, `$derived.by()`, `$props`, `$effect`
- **Styling**: Tailwind CSS v4 utility classes, dark mode via `dark:` prefix
- **Rust serde**: All stats types use `#[serde(rename_all = "camelCase")]` to match the JSON
- **No API keys or network**: Everything is local file reads
- **Tray-only app**: `ActivationPolicy::Accessory` hides the dock icon
- **Window hides on focus loss**: The popover auto-hides when clicking elsewhere

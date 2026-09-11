import { invoke } from "@tauri-apps/api/core";
import type { StatsCache, Diagnostics } from "./types";

export async function getStats(): Promise<StatsCache> {
  return invoke<StatsCache>("get_stats");
}

export async function updateTrayTitle(title: string): Promise<void> {
  return invoke("update_tray_title", { title });
}

export function getCurrentMonthPrefix(): string {
  const now = new Date();
  const y = now.getFullYear();
  const m = String(now.getMonth() + 1).padStart(2, "0");
  return `${y}-${m}`;
}

export async function getDiagnostics(): Promise<Diagnostics> {
  return invoke<Diagnostics>("get_diagnostics");
}

export async function refreshUsage(): Promise<void> {
  return invoke("refresh_usage");
}

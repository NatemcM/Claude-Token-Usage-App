/** Compact duration: 45s, 12m, 3h 20m, 2d 4h. */
export function formatDuration(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  const remM = m % 60;
  if (h < 24) return remM > 0 ? `${h}h ${remM}m` : `${h}h`;
  const d = Math.floor(h / 24);
  const remH = h % 24;
  return remH > 0 ? `${d}d ${remH}h` : `${d}d`;
}

/**
 * Idle age for a row. `null` means the session has written no transcript yet,
 * which is a real state — a registered session with no activity.
 */
export function formatIdle(secs: number | null): string {
  if (secs === null) return "no activity yet";
  if (secs < 60) return "active now";
  return `idle ${formatDuration(secs)}`;
}

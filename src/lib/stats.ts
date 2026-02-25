import type {
  ModelUsage,
  ModelSummary,
  DailyModelTokens,
  DailyTokens,
  DailyActivity,
} from "./types";

export function computeTotalTokens(
  modelUsage: Record<string, ModelUsage>,
): number {
  return Object.values(modelUsage).reduce(
    (sum, m) =>
      sum +
      m.inputTokens +
      m.outputTokens +
      m.cacheReadInputTokens +
      m.cacheCreationInputTokens,
    0,
  );
}

export function computeInputTokens(
  modelUsage: Record<string, ModelUsage>,
): number {
  return Object.values(modelUsage).reduce(
    (sum, m) => sum + m.inputTokens,
    0,
  );
}

export function computeOutputTokens(
  modelUsage: Record<string, ModelUsage>,
): number {
  return Object.values(modelUsage).reduce(
    (sum, m) => sum + m.outputTokens,
    0,
  );
}

export function computeCacheTokens(
  modelUsage: Record<string, ModelUsage>,
): number {
  return Object.values(modelUsage).reduce(
    (sum, m) => sum + m.cacheReadInputTokens + m.cacheCreationInputTokens,
    0,
  );
}

export function computeModelSummaries(
  modelUsage: Record<string, ModelUsage>,
): ModelSummary[] {
  return Object.entries(modelUsage)
    .map(([model, usage]) => ({
      model,
      inputTokens: usage.inputTokens,
      outputTokens: usage.outputTokens,
      cacheReadTokens: usage.cacheReadInputTokens,
      cacheCreationTokens: usage.cacheCreationInputTokens,
      totalTokens:
        usage.inputTokens +
        usage.outputTokens +
        usage.cacheReadInputTokens +
        usage.cacheCreationInputTokens,
    }))
    .sort((a, b) => b.totalTokens - a.totalTokens);
}

export function computeDailyTokens(
  dailyModelTokens: DailyModelTokens[],
  monthPrefix: string,
): DailyTokens[] {
  return dailyModelTokens
    .filter((d) => d.date.startsWith(monthPrefix))
    .map((d) => ({
      date: d.date,
      tokens: Object.values(d.tokensByModel).reduce((s, v) => s + v, 0),
    }));
}

export function computeMonthMessages(
  dailyActivity: DailyActivity[],
  monthPrefix: string,
): number {
  return dailyActivity
    .filter((d) => d.date.startsWith(monthPrefix))
    .reduce((sum, d) => sum + d.messageCount, 0);
}

export function computeMonthSessions(
  dailyActivity: DailyActivity[],
  monthPrefix: string,
): number {
  return dailyActivity
    .filter((d) => d.date.startsWith(monthPrefix))
    .reduce((sum, d) => sum + d.sessionCount, 0);
}

export function computeMonthToolCalls(
  dailyActivity: DailyActivity[],
  monthPrefix: string,
): number {
  return dailyActivity
    .filter((d) => d.date.startsWith(monthPrefix))
    .reduce((sum, d) => sum + d.toolCallCount, 0);
}

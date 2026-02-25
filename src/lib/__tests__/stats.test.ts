import { describe, it, expect } from "vitest";
import type { ModelUsage, DailyModelTokens, DailyActivity } from "../types";
import {
  computeTotalTokens,
  computeInputTokens,
  computeOutputTokens,
  computeCacheTokens,
  computeModelSummaries,
  computeDailyTokens,
  computeMonthMessages,
  computeMonthSessions,
  computeMonthToolCalls,
} from "../stats";

// --- Test fixtures ---

const emptyUsage: Record<string, ModelUsage> = {};

const singleModel: Record<string, ModelUsage> = {
  "claude-opus-4-6": {
    inputTokens: 1000,
    outputTokens: 2000,
    cacheReadInputTokens: 500,
    cacheCreationInputTokens: 300,
    webSearchRequests: 0,
    costUsd: 0.05,
  },
};

const multipleModels: Record<string, ModelUsage> = {
  "claude-opus-4-6": {
    inputTokens: 1000,
    outputTokens: 2000,
    cacheReadInputTokens: 500,
    cacheCreationInputTokens: 300,
    webSearchRequests: 0,
    costUsd: 0.05,
  },
  "claude-sonnet-4-5": {
    inputTokens: 3000,
    outputTokens: 4000,
    cacheReadInputTokens: 1000,
    cacheCreationInputTokens: 200,
    webSearchRequests: 2,
    costUsd: 0.03,
  },
};

const dailyModelTokens: DailyModelTokens[] = [
  { date: "2026-02-20", tokensByModel: { "claude-opus-4-6": 5000, "claude-sonnet-4-5": 3000 } },
  { date: "2026-02-21", tokensByModel: { "claude-opus-4-6": 2000 } },
  { date: "2026-01-15", tokensByModel: { "claude-opus-4-6": 9000 } },
];

const dailyActivity: DailyActivity[] = [
  { date: "2026-02-20", messageCount: 10, sessionCount: 2, toolCallCount: 5 },
  { date: "2026-02-21", messageCount: 20, sessionCount: 3, toolCallCount: 15 },
  { date: "2026-01-15", messageCount: 50, sessionCount: 5, toolCallCount: 30 },
];

// --- Token aggregation tests ---

describe("computeTotalTokens", () => {
  it("returns 0 for empty usage", () => {
    expect(computeTotalTokens(emptyUsage)).toBe(0);
  });

  it("sums all token types for a single model", () => {
    expect(computeTotalTokens(singleModel)).toBe(1000 + 2000 + 500 + 300);
  });

  it("sums across multiple models", () => {
    expect(computeTotalTokens(multipleModels)).toBe(
      1000 + 2000 + 500 + 300 + 3000 + 4000 + 1000 + 200,
    );
  });
});

describe("computeInputTokens", () => {
  it("returns 0 for empty usage", () => {
    expect(computeInputTokens(emptyUsage)).toBe(0);
  });

  it("sums only input tokens", () => {
    expect(computeInputTokens(multipleModels)).toBe(1000 + 3000);
  });
});

describe("computeOutputTokens", () => {
  it("returns 0 for empty usage", () => {
    expect(computeOutputTokens(emptyUsage)).toBe(0);
  });

  it("sums only output tokens", () => {
    expect(computeOutputTokens(multipleModels)).toBe(2000 + 4000);
  });
});

describe("computeCacheTokens", () => {
  it("returns 0 for empty usage", () => {
    expect(computeCacheTokens(emptyUsage)).toBe(0);
  });

  it("sums cache read + cache creation tokens", () => {
    expect(computeCacheTokens(multipleModels)).toBe(500 + 300 + 1000 + 200);
  });
});

// --- Model summaries ---

describe("computeModelSummaries", () => {
  it("returns empty array for empty usage", () => {
    expect(computeModelSummaries(emptyUsage)).toEqual([]);
  });

  it("creates summary with correct total", () => {
    const summaries = computeModelSummaries(singleModel);
    expect(summaries).toHaveLength(1);
    expect(summaries[0].model).toBe("claude-opus-4-6");
    expect(summaries[0].totalTokens).toBe(3800);
    expect(summaries[0].inputTokens).toBe(1000);
    expect(summaries[0].outputTokens).toBe(2000);
    expect(summaries[0].cacheReadTokens).toBe(500);
    expect(summaries[0].cacheCreationTokens).toBe(300);
  });

  it("sorts by totalTokens descending", () => {
    const summaries = computeModelSummaries(multipleModels);
    expect(summaries).toHaveLength(2);
    // sonnet has 3000+4000+1000+200 = 8200, opus has 3800
    expect(summaries[0].model).toBe("claude-sonnet-4-5");
    expect(summaries[1].model).toBe("claude-opus-4-6");
  });
});

// --- Daily tokens ---

describe("computeDailyTokens", () => {
  it("returns empty array for empty data", () => {
    expect(computeDailyTokens([], "2026-02")).toEqual([]);
  });

  it("filters by month prefix and sums per day", () => {
    const result = computeDailyTokens(dailyModelTokens, "2026-02");
    expect(result).toHaveLength(2);
    expect(result[0]).toEqual({ date: "2026-02-20", tokens: 8000 });
    expect(result[1]).toEqual({ date: "2026-02-21", tokens: 2000 });
  });

  it("excludes entries from other months", () => {
    const result = computeDailyTokens(dailyModelTokens, "2026-01");
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ date: "2026-01-15", tokens: 9000 });
  });

  it("returns empty when no entries match the month", () => {
    expect(computeDailyTokens(dailyModelTokens, "2025-12")).toEqual([]);
  });
});

// --- Monthly activity aggregations ---

describe("computeMonthMessages", () => {
  it("returns 0 for empty data", () => {
    expect(computeMonthMessages([], "2026-02")).toBe(0);
  });

  it("sums messages for the specified month", () => {
    expect(computeMonthMessages(dailyActivity, "2026-02")).toBe(30);
  });

  it("excludes other months", () => {
    expect(computeMonthMessages(dailyActivity, "2026-01")).toBe(50);
  });
});

describe("computeMonthSessions", () => {
  it("returns 0 for empty data", () => {
    expect(computeMonthSessions([], "2026-02")).toBe(0);
  });

  it("sums sessions for the specified month", () => {
    expect(computeMonthSessions(dailyActivity, "2026-02")).toBe(5);
  });
});

describe("computeMonthToolCalls", () => {
  it("returns 0 for empty data", () => {
    expect(computeMonthToolCalls([], "2026-02")).toBe(0);
  });

  it("sums tool calls for the specified month", () => {
    expect(computeMonthToolCalls(dailyActivity, "2026-02")).toBe(20);
  });

  it("sums tool calls for a different month", () => {
    expect(computeMonthToolCalls(dailyActivity, "2026-01")).toBe(30);
  });
});

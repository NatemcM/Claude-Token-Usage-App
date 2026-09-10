import { describe, it, expect, vi, afterEach } from "vitest";
import { getCurrentMonthPrefix } from "../api";
import { invoke } from "@tauri-apps/api/core";
import { getDiagnostics, refreshUsage } from "../api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("getCurrentMonthPrefix", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("returns YYYY-MM format for current date", () => {
    const result = getCurrentMonthPrefix();
    expect(result).toMatch(/^\d{4}-\d{2}$/);
  });

  it("zero-pads single-digit months", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-01-15"));
    expect(getCurrentMonthPrefix()).toBe("2026-01");
    vi.useRealTimers();
  });

  it("handles December correctly", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-12-31"));
    expect(getCurrentMonthPrefix()).toBe("2026-12");
    vi.useRealTimers();
  });

  it("handles February in a leap year", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2028-02-29"));
    expect(getCurrentMonthPrefix()).toBe("2028-02");
    vi.useRealTimers();
  });

  it("handles year boundaries", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2027-01-01"));
    expect(getCurrentMonthPrefix()).toBe("2027-01");
    vi.useRealTimers();
  });
});

describe("usage commands", () => {
  afterEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("getDiagnostics invokes the get_diagnostics command", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({
      malformedLines: 3,
      revisedMessages: 13982,
      filesTracked: 1031,
      filesRetired: 12,
      lastScanMs: 4200,
    });

    const d = await getDiagnostics();
    expect(invoke).toHaveBeenCalledWith("get_diagnostics");
    expect(d.filesTracked).toBe(1031);
    expect(d.malformedLines).toBe(3);
  });

  it("refreshUsage invokes the refresh_usage command", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(undefined);
    await refreshUsage();
    expect(invoke).toHaveBeenCalledWith("refresh_usage");
  });
});

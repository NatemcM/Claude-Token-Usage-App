import { describe, it, expect, vi, afterEach } from "vitest";
import { getCurrentMonthPrefix } from "../api";

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

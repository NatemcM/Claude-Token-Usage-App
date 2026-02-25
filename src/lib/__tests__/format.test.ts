import { describe, it, expect } from "vitest";
import {
  formatTokens,
  formatCostCents,
  formatNumber,
  formatModelName,
} from "../format";

describe("formatTokens", () => {
  it("returns raw number for values under 1000", () => {
    expect(formatTokens(0)).toBe("0");
    expect(formatTokens(1)).toBe("1");
    expect(formatTokens(999)).toBe("999");
  });

  it("formats thousands with K suffix", () => {
    expect(formatTokens(1000)).toBe("1.0K");
    expect(formatTokens(1500)).toBe("1.5K");
    expect(formatTokens(999999)).toBe("1000.0K");
  });

  it("formats millions with M suffix", () => {
    expect(formatTokens(1_000_000)).toBe("1.0M");
    expect(formatTokens(2_500_000)).toBe("2.5M");
    expect(formatTokens(999_999_999)).toBe("1000.0M");
  });

  it("formats billions with B suffix", () => {
    expect(formatTokens(1_000_000_000)).toBe("1.0B");
    expect(formatTokens(3_700_000_000)).toBe("3.7B");
  });
});

describe("formatCostCents", () => {
  it("converts cents to dollar string", () => {
    expect(formatCostCents(0)).toBe("$0.00");
    expect(formatCostCents(100)).toBe("$1.00");
    expect(formatCostCents(12345)).toBe("$123.45");
    expect(formatCostCents(1)).toBe("$0.01");
  });

  it("handles negative values", () => {
    expect(formatCostCents(-500)).toBe("$-5.00");
  });
});

describe("formatNumber", () => {
  it("formats numbers with locale separators", () => {
    expect(formatNumber(0)).toBe("0");
    expect(formatNumber(1000)).toMatch(/1.000/);
    expect(formatNumber(1000000)).toMatch(/1.000.000/);
  });
});

describe("formatModelName", () => {
  it("removes claude- prefix and title-cases parts", () => {
    expect(formatModelName("claude-opus-4-6")).toBe("Opus 4 6");
    expect(formatModelName("claude-sonnet-4-5")).toBe("Sonnet 4 5");
    expect(formatModelName("claude-haiku-3-5")).toBe("Haiku 3 5");
  });

  it("handles models without claude- prefix", () => {
    expect(formatModelName("gpt-4")).toBe("Gpt 4");
  });

  it("handles single-part model names", () => {
    expect(formatModelName("claude-opus")).toBe("Opus");
  });
});

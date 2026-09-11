import { describe, it, expect } from "vitest";
import { formatDuration, formatIdle } from "../duration";

describe("formatDuration", () => {
  it("renders seconds under a minute", () => {
    expect(formatDuration(0)).toBe("0s");
    expect(formatDuration(45)).toBe("45s");
  });

  it("renders minutes under an hour", () => {
    expect(formatDuration(60)).toBe("1m");
    expect(formatDuration(3599)).toBe("59m");
  });

  it("renders hours and minutes under a day", () => {
    expect(formatDuration(3600)).toBe("1h");
    expect(formatDuration(3660)).toBe("1h 1m");
    expect(formatDuration(86399)).toBe("23h 59m");
  });

  it("renders days and hours beyond a day", () => {
    expect(formatDuration(86400)).toBe("1d");
    expect(formatDuration(90000)).toBe("1d 1h");
  });
});

describe("formatIdle", () => {
  it("says active now inside the first minute", () => {
    expect(formatIdle(0)).toBe("active now");
    expect(formatIdle(59)).toBe("active now");
  });

  it("renders an age beyond a minute", () => {
    expect(formatIdle(60)).toBe("idle 1m");
    expect(formatIdle(7200)).toBe("idle 2h");
  });

  it("handles an unknown age", () => {
    expect(formatIdle(null)).toBe("no activity yet");
  });
});

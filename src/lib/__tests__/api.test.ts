import { describe, it, expect, vi, afterEach } from "vitest";
import { getCurrentMonthPrefix } from "../api";
import { invoke } from "@tauri-apps/api/core";
import {
  getDiagnostics,
  refreshUsage,
  listSessions,
  removeStaleRegistration,
  getAppSettings,
  setTrayShowSessions,
} from "../api";

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
      legacyThrough: "2026-03-10",
      projectsRoot: "/home/me/.claude/projects",
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

describe("session commands", () => {
  afterEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("listSessions invokes list_sessions and decodes rows", async () => {
    vi.mocked(invoke).mockResolvedValueOnce([
      {
        state: "live",
        pid: 12158,
        sessionId: "s-1",
        name: "my-app-42",
        cwd: "/Users/me/Projects/my-app",
        project: "my-app",
        gitBranch: "main",
        entrypoint: "claude-vscode",
        version: "2.1.267",
        startedAtMs: 1789029119710,
        uptimeSecs: 3600,
        lastActivityMs: 1789029179076,
        idleSecs: 120,
        tokens: 1234567,
        messageCount: 12,
        isActive: true,
        removable: false,
        agents: [
          { agentId: "a1", agentType: "Explore", tokens: 210000, idleSecs: 5, killable: false },
        ],
      },
    ]);

    const rows = await listSessions();
    expect(invoke).toHaveBeenCalledWith("list_sessions");
    expect(rows).toHaveLength(1);
    expect(rows[0].project).toBe("my-app");
    expect(rows[0].agents[0].killable).toBe(false);
  });

  it("removeStaleRegistration passes the pid", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({ removed: ["/x/4242.json"], skipped: [] });
    const r = await removeStaleRegistration(4242);
    expect(invoke).toHaveBeenCalledWith("remove_stale_registration", { pid: 4242 });
    expect(r.removed).toHaveLength(1);
  });

  it("getAppSettings and setTrayShowSessions map to their commands", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({ trayShowSessions: true });
    expect((await getAppSettings()).trayShowSessions).toBe(true);
    expect(invoke).toHaveBeenCalledWith("get_app_settings");

    vi.mocked(invoke).mockResolvedValueOnce(undefined);
    await setTrayShowSessions(false);
    expect(invoke).toHaveBeenCalledWith("set_tray_show_sessions", { enabled: false });
  });
});

// --- Local Stats Cache (matches ~/.claude/stats-cache.json) ---

export interface DailyActivity {
  date: string;
  messageCount: number;
  sessionCount: number;
  toolCallCount: number;
}

export interface DailyModelTokens {
  date: string;
  tokensByModel: Record<string, number>;
}

export interface ModelUsage {
  inputTokens: number;
  outputTokens: number;
  cacheReadInputTokens: number;
  cacheCreationInputTokens: number;
  webSearchRequests: number;
  costUsd: number;
}

export interface LongestSession {
  sessionId: string;
  duration: number;
  messageCount: number;
  timestamp: string;
}

export interface StatsCache {
  version: number;
  lastComputedDate: string;
  dailyActivity: DailyActivity[];
  dailyModelTokens: DailyModelTokens[];
  modelUsage: Record<string, ModelUsage>;
  totalSessions: number;
  totalMessages: number;
  longestSession: LongestSession | null;
  firstSessionDate: string | null;
  hourCounts: Record<string, number> | null;
}

// --- Derived UI Types ---

export interface ModelSummary {
  model: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  totalTokens: number;
}

export interface DailyTokens {
  date: string;
  tokens: number;
}

export interface DailyMessages {
  date: string;
  messages: number;
  toolCalls: number;
}

// --- Ingest diagnostics (from the Rust usage worker) ---

export interface Diagnostics {
  malformedLines: number;
  /** Messages a later copy revised upward. ~30% is normal, not an error. */
  revisedMessages: number;
  filesTracked: number;
  filesRetired: number;
  lastScanMs: number;
  /** Last date key covered by the one-time legacy seed, or null if none exists. */
  legacyThrough: string | null;
  /** Display form of the projects root actually in use. */
  projectsRoot: string;
}

// --- Sessions (Phase 2) ---

export interface AgentRow {
  agentId: string;
  /** Absent on some subagent records. */
  agentType: string | null;
  tokens: number;
  idleSecs: number;
  /** Always false: subagents share their parent's process. */
  killable: boolean;
}

export interface SessionRow {
  state: "live" | "stale";
  pid: number;
  sessionId: string;
  name: string;
  cwd: string | null;
  /** Last path component of cwd. */
  project: string;
  gitBranch: string | null;
  entrypoint: string | null;
  version: string | null;
  startedAtMs: number;
  /** null for a stale row: the process is gone. */
  uptimeSecs: number | null;
  /** null when the session has written no transcript yet. */
  lastActivityMs: number | null;
  idleSecs: number | null;
  tokens: number;
  messageCount: number;
  isActive: boolean;
  /** Only stale rows can be cleared, and only their registration files. */
  removable: boolean;
  agents: AgentRow[];
}

export interface RemovalReport {
  removed: string[];
  skipped: string[];
}

export interface AppSettings {
  trayShowSessions: boolean;
}

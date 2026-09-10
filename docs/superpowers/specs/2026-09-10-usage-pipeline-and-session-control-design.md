# Usage pipeline rebuild + session control

**Date:** 2026-09-10
**Status:** Approved design, pending implementation plan
**Reviewed by:** Fable (adversarial design review), findings incorporated

## 1. Why

`claude-token-usage` reads `~/.claude/stats-cache.json`. That file was last written
**2026-03-11** (`lastComputedDate: 2026-03-10`). Claude Code no longer maintains it, so the
app has displayed six-month-stale numbers since roughly March. The code itself is healthy —
39 frontend tests pass and the architecture matches `CLAUDE.md`; the upstream data contract
simply went away.

Separately, the author repeatedly hits Claude Code sessions that stay alive after their
editor or terminal is gone, with no way to see or stop them from outside.

This design replaces the dead data source with the live transcripts, and adds a sessions
view that can see and terminate running sessions.

## 2. Verified facts

All measured on the author's machine on 2026-09-10. Figures matter because several design
choices depend on them.

### 2.1 Usage data location

`~/.claude/projects/<project-slug>/<session-id>.jsonl`, one JSON object per line.
Assistant records carrying `message.usage` provide:

- `message.usage`: `input_tokens`, `output_tokens`, `cache_read_input_tokens`,
  `cache_creation_input_tokens`, `cache_creation.{ephemeral_1h,ephemeral_5m}_input_tokens`,
  `output_tokens_details.thinking_tokens`,
  `server_tool_use.{web_search,web_fetch}_requests`, `service_tier`
- `message.model`, `message.id`
- top level: `timestamp` (ISO8601 UTC), `sessionId`, `cwd`, `gitBranch`, `version`,
  `entrypoint`, `requestId`, `uuid`, `parentUuid`, `isSidechain`, `type`, `apiBlockIndex`

### 2.2 Scale and retention

~1031 `.jsonl` files, 793 MB, largest single file 95 MB. All counts here are a live
snapshot: sessions were actively writing during measurement, so totals drift by a file
or two between figures. A 95 MB line-scan with full JSON
parse takes 0.63 s in Python, so a full pass is seconds, not minutes.

**Transcripts are a rolling ~30-day window.** Oldest *main* transcript is 2026-08-13 (28
days); `cleanupPeriodDays` is unset, so Claude Code's default of 30 applies. Files will
disappear between ingest passes as a matter of routine.

### 2.3 Duplication — the single most important fact

| Measure | Value |
|---|---|
| Usage-bearing assistant lines | 124,510 |
| Distinct `(file, message.id)` | 45,873 |
| Line duplication | 2.72x |
| **Output-token inflation if undeduped** | **2.56x** (98.5M raw vs 38.6M actual) |
| **Undercount if deduped first-wins** | **-45%** (21.2M vs 38.6M actual) |

Each line carries exactly **one** content block, not the full array. Two duplication
mechanisms: every content block emits its own line, and whole messages are re-emitted
non-consecutively later in the same file, usually with different `uuid`s.

**Copies are NOT identical, and this is the trap.** For 13,982 of 45,873 messages (30%) the
first copy seen carries a *smaller* `output_tokens` than later copies: Claude Code writes the
first block line while the response is still streaming (that first block is `thinking` in
28,601 cases, `tool_use` in 11,804, `text` in 5,468) and the final block carries the true
total. One measured example went 7 -> 156.

Therefore dedup must take the **per-field maximum** across copies, not first-wins. First-wins
is not a conservative approximation — it silently loses 45% of output tokens, and it looks
plausible, which is worse. Any verification script must also take the max, or it will
"confirm" the undercount. `input_tokens` and cache fields diverge in only 3 records.

`message.id` never appears in more than one file. `requestId` is 1:1 with `message.id` but is
**absent on 18 records**, so it must not form part of the dedup key.

### 2.4 Models

Only 7 models appear in usage-bearing records:

```
claude-opus-5 77553, claude-sonnet-5 23217, claude-fable-5 14455,
claude-fable-5-1 7354, claude-haiku-4-5-20251001 1870, <synthetic> 52, claude-opus-4-8 13
```

Bare aliases (`sonnet`, `opus`, `fable`, `haiku`) exist in the files but **never** in a
usage-bearing record. `<synthetic>` records all carry `isApiErrorMessage: true` and all-zero
usage; excluding them is correct but numerically a no-op.

### 2.5 Transcript path shapes

```
797  <session-id>/subagents/agent-<id>.jsonl
 82  <session-id>.jsonl
148  <session-id>/subagents/workflows/wf_<id>/agent-<id>.jsonl
  5  <session-id>/subagents/workflows/wf_<id>/journal.jsonl
```

Subagent records add `agentId` and `attributionAgent` (agent type; absent on ~1,255 records)
and share the **parent's** `sessionId`.

### 2.6 Live session registry

`~/.claude/sessions/<pid>.json`:

```json
{ "pid": 12158, "sessionId": "36500922-...", "cwd": "/Users/.../claude-token-usage",
  "startedAt": 1789029119710, "procStart": "Thu Sep 10 08:31:58 2026", "version": "2.1.267",
  "kind": "interactive", "entrypoint": "claude-vscode", "pidDomain": "darwin",
  "name": "claude-token-usage-32", "messagingSocketPath": "/tmp/cc-socks/12158.sock" }
```

A sibling `<pid>.<sha>.key` file (mode 0600) exists per session. We never read it, but a
stale-registration removal should delete it alongside the `.json`.

`startedAt` (epoch ms) is 1-8 s **after** `procStart`. `procStart` is rendered in **UTC**
while `ps -o lstart` renders **local** — the same instant, 7 h apart as text on this machine.
A live session may have **no transcript file at all** (observed: pid 8772).

### 2.7 Leak evidence

`~/.claude/session-env/` holds 135 directories against 5 live sessions. They are **empty**
(0 bytes reclaimable), every name is a real session id, and the oldest is 30 days old — i.e.
Claude Code does prune them. `sessions/` (5) and `/tmp/cc-socks` (5) are clean.

## 3. Non-goals

- **No dollar cost estimates.** Price tables rot as models ship, and the app is
  no-network by principle. `ModelUsage.costUsd` in the existing TS type is populated as 0
  and hidden. (Revisit only on explicit request.)
- **No bulk cleanup UI.** The only leak reclaims ~0 bytes and upstream already prunes it.
  Superseded by per-row stale-registration removal (Phase 2).
- **No sidecar daemon.** Ingestion is cheap enough to run in-process.
- **No macOS App Sandbox.** It would break both signalling other processes and reading
  `~/.claude`. This is a hard constraint on any future notarization work.
- **No individual subagent termination.** Subagents have no PID of their own.

## 4. Data model

Our cache lives at
`~/Library/Application Support/com.claudetokenusage.dev/usage-cache.v1.json`.
We own it, so we may version, rebuild or delete it freely. `~/.claude` is **read-only**
except the explicit stale-registration removal in Phase 2.

**Per-file rollups are the unit of storage.** A global `daily` aggregate cannot be
subtracted from, so a pruned or truncated file would silently corrupt it — and pruning
happens daily (§2.2).

```rust
struct UsageCache {
  schema: u32,                        // bump => full rebuild
  tz_offset_minutes: i32,             // TZ at ingest; change => rebuild day keys
  files: HashMap<PathBuf, FileEntry>,
  legacy: Option<LegacyRollup>,       // one-time stats-cache.json seed (§5.4)
}

struct FileEntry {
  cursor: FileCursor,
  seen: HashSet<u64>,                 // hash(message.id); dropped when retired
  retired: bool,                      // source file pruned; days retained
  days: BTreeMap<String, DayRollup>,  // local date -> per-model counts + activity
  session: SessionRollup,
  agents: HashMap<String, AgentRollup>,
}

struct FileCursor { offset: u64, size: u64, mtime_ms: u64, inode: u64 }

struct TokenCounts {
  input: u64, output: u64, cache_read: u64, cache_creation: u64,
  cache_1h: u64, cache_5m: u64, thinking: u64, web_search: u64, web_fetch: u64,
}

struct DayRollup {
  by_model: HashMap<String, TokenCounts>,   // raw model ids, never families
  message_count: u32, tool_call_count: u32, // sources §5.3
  session_ids: HashSet<String>,
}

struct SessionRollup {
  session_id: String, cwd: PathBuf, git_branch: Option<String>,
  first_ts: i64, last_ts: i64,        // last record of ANY type (§5.3)
  last_usage_ts: i64, message_count: u32,
  by_model: HashMap<String, TokenCounts>,
}

struct AgentRollup {
  agent_id: String, agent_type: Option<String>,
  last_ts: i64, tokens: TokenCounts,
}
```

`daily` and `sessions` views are **derived by summation at load** across ~1,000 entries —
cheap, and it makes retirement and re-ingest correct by construction.

`TokenCounts` is a superset of the existing `ModelUsage`, and the derived daily view maps
onto the existing `dailyModelTokens`, so `src/lib/stats.ts` and its tests continue to apply
against richer input rather than being rewritten.

## 5. Phase 1 — usage pipeline

Ships an accurate, live dashboard. Independently valuable: it fixes the app's core identity.

### 5.1 Discovery

Walk `~/.claude/projects/` **recursively** for `*.jsonl`. Do not pattern-match paths — the
`workflows/` nesting and `journal.jsonl` (§2.5) would be missed. Attribution comes from
record fields (`sessionId`, `agentId`, `attributionAgent`) only.

The config root must be overridable in Settings: `CLAUDE_CONFIG_DIR` can relocate `~/.claude`
and a GUI app cannot see the user's shell environment.

### 5.2 Incremental ingest

Per file, compare `stat` against `FileCursor`:

| Condition | Action |
|---|---|
| `size` and `mtime` unchanged | skip, zero reads |
| grown | `seek(offset)`, read only the delta |
| `size < offset` or `inode` changed | discard entry, re-ingest whole |
| file absent | `retired = true`, keep `days`, **drop `seen`** |

Retiring rather than deleting is what turns this cache into a durable long-term store: usage
history accumulates from install onward and outlives the upstream 30-day window.

Dropping `seen` on retirement bounds cache growth. Live `seen` is ~45,700 ids (~1 MB as JSON)
for a 30-day window and grows ~1,500 ids/day; pruning on retirement caps it near 1 MB rather
than ~11 MB/year.

**Partial-line safety:** advance the stored offset only to the **last complete newline**, so a
record half-written by a live session is re-read intact next pass instead of being dropped or
counted malformed.

Read with `BufReader` from `offset`. Never `read_to_string` a 95 MB file. Pre-filter each line
with a byte search for `"type":"assistant"` before parsing, and deserialize into a struct that
leaves `content` lazy.

### 5.3 Parsing rules

- **Dedup on `hash(message.id)` alone**, per file (`message.id` never spans files), taking the
  **per-field maximum** across copies. This is mandatory, not defensive: early copies are
  written mid-stream with partial `output_tokens` (§2.3), so first-wins undercounts by 45%.
  Implementation keeps the credited counts per message id and adds only the positive
  per-field delta when a later copy exceeds them.
- **Tool calls dedupe on the `tool_use` block's own `id`**, and **user messages on the record
  `uuid`** — one key cannot serve all three metrics, because each line carries a single
  content block. Observed: 68,492 raw tool_use blocks vs 54,229 distinct ids; 72,443 user
  records vs 57,122 distinct uuids.
- **User records whose content is only `tool_result` blocks are not messages.** 68,600 of
  70,308 array-content user records are tool-result echoes; counting them would report
  ~100k "messages" a month beside legacy days of ~25.
- Count `<synthetic>` as excluded.
- **Keep raw model ids.** Family grouping (Opus/Sonnet/Haiku/Fable) is a display-level toggle
  at most; collapsing `claude-opus-4-8` into `claude-opus-5` would destroy detail
  `ModelBreakdown` renders today.
- Subagent tokens roll into `AgentRollup` **and** the parent's day totals, never twice.
- **Activity metrics require non-usage records.** `stats.ts` depends on
  `dailyActivity.{messageCount,sessionCount,toolCallCount}`, `totalSessions`, `totalMessages`.
  Sources: tool calls = `tool_use` blocks in assistant `content` (deduped by `message.id`);
  messages = `user` records + distinct assistant ids; sessions/day = distinct `sessionId`
  active that day; `hourCounts` = each session's first timestamp hour; `longestSession` =
  max(`last_ts - first_ts`).
- Malformed lines are counted, never fatal.
- Bucket by **local** date, recording `tz_offset_minutes` so a TZ change triggers a rebuild
  rather than yielding mixed-timezone days.

### 5.4 Legacy seed

`stats-cache.json` covers 2026-01-25 -> 2026-03-10 (36 days, 154 sessions) with **no overlap**
with transcripts. Import it once into a `legacy` retired rollup so that history is preserved
rather than silently dropped. Mark it plainly as legacy in the UI's earliest range.

### 5.5 Concurrency and durability

Today's watcher calls its handler per event; with 5 live sessions transcripts change several
times per second, so a naive design would rewrite a multi-MB cache thousands of times a day.

- **Debounce** filesystem events ~1-2 s.
- **Single ingest worker** behind a mutex, with a dirty flag. The watcher, the 60 s fallback
  poll, manual refresh, and `get_stats` must not race.
- **Persist at most every ~30 s and on quit**, via tmp-file + atomic rename.
- **Parse failure on load** -> rename to `.corrupt` and rebuild. `schema` covers version
  changes only, not corruption.
- Emit `usage-progress` during the first full pass; emit `stats-updated` as today.

`polling.rs` grows from a non-recursive watch of one filename in `~/.claude` to a recursive
watch of `projects/` (filtered `*.jsonl`) plus `sessions/` (filtered `*.json`).

### 5.6 Fix the existing month-boundary bug

`lib.rs::current_month_prefix()` derives the month from UTC epoch days by hand;
`api.ts::getCurrentMonthPrefix()` uses local time. On any non-UTC machine the tray disagrees
with the dashboard at every month boundary (7 h on this machine). Adopt `chrono` with `Local`
in both, and state in the UI that the tray figure sums **all** token classes — it is dominated
by cache reads (one session showed 655M cache-read against 2M output).

## 6. Phase 2 — sessions tab (read-only)

### 6.1 Reconciliation

| State | Evidence | Action available |
|---|---|---|
| `Live` | session file + live PID + start time matches | none in this phase |
| `Stale` | session file, no live PID | remove registration (`.json` + `.key` + sock) |
| `Orphan` | live claude-like PID, no session file | **deferred to Phase 4** |

Process probing uses the `sysinfo` crate rather than parsing `ps`: `start_time()` returns
unix seconds, avoiding the `procStart` timezone trap entirely, and it supplies `cmd()` and
signalling. Ignore any session whose `pidDomain != "darwin"`.

Idle age = `now - last_ts` where `last_ts` is the last record of **any** type; a session
midway through a ten-minute Bash call is not idle. Sessions with no transcript (§2.6) render
as "registered, no activity yet" rather than erroring or showing a bogus age.

### 6.2 UI

Three-tab shell in the existing 400x600 popover: Usage (existing dashboard, repointed),
Sessions, Settings behind the existing gear.

Row: status dot (green active <5 min, amber idle, red stale), friendly `name`, tokens,
project basename + git branch, uptime and idle age. Subagents appear as nested read-only rows
(name/type, tokens, last activity), marked not individually killable.

Stale rows group below live ones with a per-row remove action.

While the popover is visible, sessions re-poll every 2 s (5 `stat` calls plus a `sysinfo`
refresh); polling pauses when hidden.

Tray title keeps the month token count and appends live session count when non-zero
(`2.4M · 5`), with a Settings toggle, default on.

## 7. Phase 3 — termination

Confirm sheet shows `name`, `cwd`, PID, uptime, idle age, tokens, **and child-process count**
— SIGTERM to `claude` does not terminate children it spawned (dev servers, MCP servers), which
would otherwise surprise the user.

1. **Re-verify identity at signal time.** Re-read `sessions/<pid>.json`; require PID alive and
   `|sysinfo.start_time - startedAt/1000| <= 120 s`. `procStart` parsed as UTC is a *secondary*
   accept path, never the sole check — a wrong sign there would silently disable every kill.
   The `cmd` check may only **refuse** (clearly-not-claude), never authorize: binary paths vary
   across VSCode extension, npm global (argv[0] is `node`), native installer and homebrew
   installs. Session files are rewritten in place, so retry once on a half-written parse before
   refusing.
2. `SIGTERM`, letting Claude Code flush its transcript.
3. Poll liveness every 250 ms for 5 s.
4. Gone -> success, refresh. Alive -> return `StillRunning`; the sheet swaps its button to
   **Force kill (SIGKILL)**.
5. `SIGKILL` -> poll 2 s -> report honestly if it still survives (stuck in a syscall).
6. **Clean up after ourselves.** A SIGKILLed session cannot remove its own
   `sessions/<pid>.json`, `.key` or sock, so remove them immediately post-kill rather than
   manufacturing Stale rows.

Hard rules: never signal our own PID; never signal a PID that failed step 1.

## 8. Phase 4 — orphan handling (conditional)

Build only if a real need appears. Legitimate claude processes lack a session file:
`claude -p`/SDK batch runs, sessions spawned by hooks or the Agent SDK as children of a live
session, and transient `claude --version` probes. Presenting these as killable invites killing
a live session's child job.

If built: require exe basename `claude` **or** an argv ending `claude-code/cli.js`; report the
ppid chain and flag "child of live session X"; exclude `-p`/`--print` processes.

## 9. Testing

The destructive paths are abstracted so tests never touch reality:

```rust
trait ProcessProbe { fn is_alive(&self, pid: u32) -> bool;
                     fn start_time(&self, pid: u32) -> Option<i64>;
                     fn cmd(&self, pid: u32) -> Option<Vec<String>>;
                     fn signal(&self, pid: u32, sig: Signal) -> Result<(), String>; }
trait FsOps       { fn remove_file(&self, p: &Path) -> Result<(), String>; }
```

A fake probe scripting "alive, alive, alive, dead" asserts the SIGTERM -> poll -> SIGKILL
sequence deterministically, with no timing flakiness and nothing at risk.

Rust unit tests (TDD, `tempfile` fixtures — dev-dep already present):

- *Cursor:* grown -> delta only; truncated -> full re-ingest; inode change -> full re-ingest;
  unchanged -> zero reads; absent -> retired with `days` kept and `seen` dropped.
- *Dedup:* one `message.id` across k `apiBlockIndex` lines counts once; a message re-emitted
  5x non-consecutively counts once; copies straddling two incremental passes count once; a
  record with `requestId: null` still dedupes.
- *Inflation regression:* a fixture reproducing the 2.72x line duplication must yield 1x
  token totals.
- *Partial line:* a mid-record ending advances the offset only to the last newline, and the
  record is counted exactly once next pass.
- *Parsing:* `<synthetic>` excluded; raw model ids preserved; malformed line skipped and
  counted; cache 1h/5m, thinking, web-search extracted; `attributionAgent` absent -> `None`.
- *Activity:* tool-call, message, session/day, `hourCounts` and `longestSession` derivation.
- *Bucketing:* a UTC timestamp near midnight lands in the correct local day; a changed
  `tz_offset_minutes` triggers rebuild.
- *Discovery:* `workflows/wf_*/agent-*.jsonl` and `journal.jsonl` are both found.
- *Durability:* corrupt cache -> `.corrupt` + rebuild; schema bump -> rebuild; interrupted
  write leaves the previous cache intact.
- *Reconciliation:* Live / Stale matrix; `pidDomain != darwin` ignored; live session with no
  transcript renders without error.
- *Kill safety:* alive but `start_time` mismatch -> refuse (recycled PID); own PID -> refuse;
  clearly-not-claude `cmd` -> refuse; half-written session file -> retry then refuse;
  `node .../cli.js` -> accepted.
- *Post-kill:* successful SIGKILL removes `.json`, `.key` and sock.

Frontend vitest keeps the existing 39 and adds idle-age/uptime formatting (including the
"active now" boundary), Live/Stale grouping and ordering, subagent nesting, the
no-transcript row, and the kill confirm state machine through Force kill.

**Manual verification** (tests cannot prove we read the real Claude Code):
`npx tauri dev`; confirm the live list matches `ps` exactly; confirm dashboard totals are
plausible against a per-field-max count rather than ~2.5x inflated or 45% short; start a
throwaway session
(`cd /tmp/scratch && claude`), confirm it appears with correct project and idle age, kill it
from the panel, and confirm both process and row disappear along with its `sessions/<pid>.*`.

## 10. Implementation plan granularity

One implementation plan per phase, written and executed in order. The immediate next
artifact covers **Phase 1 only** (§5) — it is independently shippable and restores the
app's core function. Phases 2 and 3 get their own plans once Phase 1 is verified against
real data; Phase 4 may never be written.

## 11. Open risks

- **Undocumented upstream format.** Every field here is observed behavior of Claude Code
  2.1.267, not a published contract. Ingestion must degrade to "missing field -> skip record,
  count it" rather than failing, and the UI should surface a malformed-record count so silent
  drift is visible.
- **Duplication semantics could change.** If upstream stops re-emitting blocks, dedup becomes
  a no-op — harmless. If it changes `message.id` reuse, totals break; the inflation regression
  test is the tripwire.
- **Day bucketing uses a single fixed UTC offset**, captured at ingest. In a DST zone the
  offset changes twice a year, so messages within an hour of local midnight can land in the
  neighbouring day, and the offset change forces a (non-destructive) rebuild. Accepted:
  per-timestamp `Local` bucketing would fix it but makes every ingest test depend on the
  machine's timezone. Revisit if day boundaries ever matter more than they do for a
  token dashboard.
- **`sysinfo` on a future sandboxed/notarized build** may lose visibility of other processes.
  Recorded as the hard constraint in §3.
- **A corrupt cache forfeits retired history.** Discovered during implementation: `load`
  returns `Corrupt` exactly when `serde_json::from_str::<UsageCache>` fails, and any salvage
  pass over the same bytes uses the same call — so nothing can be recovered from a corrupt
  cache. Retired entries (whose source transcripts upstream has already pruned) are therefore
  unrecoverable in that case. Mitigated by `save_atomic`'s tmp+rename, which makes a torn
  write near-impossible, so the realistic causes are disk error or tampering. The behaviour is
  pinned by a test named for the limitation rather than left implicit.
  **Phase 2 follow-up:** storing the cache as one JSON object per line would make partial
  recovery possible (unparseable lines skipped, the rest salvaged) and would also make appends
  cheaper. Salvage on the Schema and Timezone rebuilds — the deliberate, routine triggers —
  works today and is tested.

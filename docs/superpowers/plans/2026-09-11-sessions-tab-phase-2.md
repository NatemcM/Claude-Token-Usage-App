# Sessions Tab (Phase 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a read-only Sessions tab that shows every running Claude Code session — project, uptime, idle age, tokens burned, nested subagents — and lets the user clear the debris left by sessions that died without cleaning up.

**Architecture:** A new `sessions` module reconciles three sources: the registry Claude Code maintains at `~/.claude/sessions/<pid>.json`, live process state via `sysinfo`, and per-session activity from the usage cache built in Phase 1. Process access sits behind a `ProcessProbe` trait so every test runs against a fake and never touches real processes. The frontend's two-view shell becomes a three-tab shell.

**Tech Stack:** Rust (Tauri v2, sysinfo, serde), Svelte 5 runes, Vitest, `cargo test`.

**Spec:** `docs/superpowers/specs/2026-09-10-usage-pipeline-and-session-control-design.md` — this plan implements **§6 only**. Termination (§7) is Phase 3; orphan handling (§8) is Phase 4 and conditional.

**Predecessor:** Phase 1 (`docs/superpowers/plans/2026-09-10-usage-pipeline-phase-1.md`) is merged. The usage pipeline, cache, worker and `StatsCache` adapter all exist and are tested; 121 Rust tests and 41 frontend tests pass on `main`.

## Global Constraints

- **`sysinfo` MUST be pinned to `0.35`.** Measured on this machine: `sysinfo` 0.37.2 requires rustc **1.88**, 0.39.6 requires rustc **1.95**, and the installed toolchain is rustc **1.86.0**. 0.35.2 resolves and builds. Use `sysinfo = "0.35"` and do not let it float.
- **NO TERMINATION IN THIS PHASE.** Do not add a kill button, a signal call, or a `signal` method on any trait. Phase 3 owns that. A `Live` row has no actions.
- **Orphan detection is OUT OF SCOPE.** Only `Live` and `Stale` states exist here. Do not scan for claude-like processes lacking a session file.
- **Exactly one write is permitted under `~/.claude`:** removing a *verified-dead* session's registration files. Every other path in this phase is read-only.
- **That removal must be path-allowlisted.** Only `<sessions_dir>/<pid>.json`, `<sessions_dir>/<pid>.<hex>.key`, and a socket path that is both declared by that session's own JSON and located under `/tmp/cc-socks/`. Never glob, never recurse, never delete a directory.
- **Identity is `startedAt`, not `procStart`.** Compare `startedAt / 1000` against `sysinfo`'s `start_time()` — both are unix **seconds**, so no timezone parsing is involved. Measured skew across 6 live sessions: **0–8 s**; tolerance is **120 s**. The `procStart` string is rendered in UTC while `ps` renders local, which is a trap the spec documents; do not parse it.
- **Ignore any session whose `pidDomain != "darwin"`** — its pid refers to another machine's namespace.
- **Idle age comes from `last_ts`** (the last transcript record of ANY type), never `last_usage_ts`. A session midway through a ten-minute Bash call is not idle.
- **A live session may have no transcript at all** (observed). It must render as "no activity yet", never error and never show a bogus age.
- **Subagents are read-only rows.** They have no pid and cannot be acted on individually.
- Types crossing the Tauri boundary use `#[serde(rename_all = "camelCase")]`; internal types stay snake_case.
- **Existing suites must stay green:** `cargo test` (121) and `npm test` (41).

---

## File Structure

| File | Responsibility |
|---|---|
| `src-tauri/src/sessions/mod.rs` | Module wiring |
| `src-tauri/src/sessions/probe.rs` | `ProcInfo`, `ProcessProbe` trait, `SysinfoProbe`, `FakeProbe` |
| `src-tauri/src/sessions/registry.rs` | Parse `~/.claude/sessions/*.json` into `SessionFile` |
| `src-tauri/src/sessions/reconcile.rs` | Classify Live/Stale; build `SessionRow` from registry + probe + rollups |
| `src-tauri/src/sessions/registration.rs` | Allowlisted removal of a dead session's registration files |
| `src-tauri/src/settings.rs` | Tiny persisted app settings (the tray toggle) |
| `src-tauri/src/usage/worker.rs` | **Modified:** expose per-session rollups merged across files |
| `src-tauri/src/lib.rs` | **Modified:** new commands, tray session count |
| `src/components/SessionsList.svelte` | Grouped Live/Stale list, polling lifecycle |
| `src/components/SessionRow.svelte` | One row plus its nested subagent rows |
| `src/App.svelte` | **Modified:** two tabs plus a full-screen Settings overlay |
| `src/components/Settings.svelte` | **Modified:** tray toggle |
| `src/lib/api.ts`, `src/lib/types.ts` | **Modified:** new commands and types |
| `CLAUDE.md` | **Modified:** record the sysinfo toolchain pin |

---

### Task 1: Process probe behind a trait

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `CLAUDE.md`
- Create: `src-tauri/src/sessions/mod.rs`
- Create: `src-tauri/src/sessions/probe.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod sessions;`)
- Test: inline in `src-tauri/src/sessions/probe.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `ProcInfo { start_time_secs, exe_basename }`, `trait ProcessProbe { fn probe(&self, pids: &[u32]) -> HashMap<u32, ProcInfo> }`, `SysinfoProbe::new()`, `FakeProbe` (test-only; `alive(pid, start_time_secs, exe_basename)` and `dead(pid)` builders).

**Why a trait:** every later task needs to answer "is this pid alive and is it the process the registry thinks it is". Doing that against real processes in tests would make them machine-dependent and, once Phase 3 lands, dangerous. `probe()` takes the pid list and returns only the LIVE ones, so absence means dead — there is no separate `is_alive` to fall out of sync.

- [ ] **Step 1: Pin the dependency and record why**

In `src-tauri/Cargo.toml` under `[dependencies]`, after the `chrono` line:

```toml
sysinfo = "0.35"
```

Then add this bullet to the "Known Issues & Gotchas" list in `CLAUDE.md`, next to the existing `time` crate note:

```markdown
- **`sysinfo` must stay pinned to 0.35** - 0.37 requires rustc 1.88 and 0.39 requires rustc 1.95; the toolchain here is rustc 1.86. Bumping it fails the build with `requires rustc 1.xx`. Same class of problem as the `time` crate pin above.
```

- [ ] **Step 2: Write the failing test**

First wire the module in, so the test is actually part of the crate and the
next step fails with a type error rather than silently running zero tests.
Create `src-tauri/src/sessions/mod.rs` containing `pub mod probe;`, and add
`mod sessions;` to `src-tauri/src/lib.rs` beside the existing `mod usage;` and
`mod config;`.

Then create `src-tauri/src/sessions/probe.rs` with ONLY this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_probe_reports_only_the_pids_declared_alive() {
        let probe = FakeProbe::new()
            .alive(100, 1_789_000_000, "claude")
            .alive(200, 1_789_000_500, "node")
            .dead(300);

        let got = probe.probe(&[100, 200, 300, 400]);

        assert_eq!(got.len(), 2, "dead and unknown pids must be absent, not present-and-false");
        assert_eq!(got[&100].start_time_secs, 1_789_000_000);
        assert_eq!(got[&100].exe_basename.as_deref(), Some("claude"));
        assert_eq!(got[&200].exe_basename.as_deref(), Some("node"));
        assert!(!got.contains_key(&300));
        assert!(!got.contains_key(&400));
    }

    #[test]
    fn fake_probe_returns_empty_for_an_empty_pid_list() {
        let probe = FakeProbe::new().alive(100, 1, "claude");
        assert!(probe.probe(&[]).is_empty());
    }

    #[test]
    fn sysinfo_probe_finds_our_own_process_and_not_a_bogus_pid() {
        // The one test that touches real processes. It only inspects; it never
        // signals. Our own pid is guaranteed alive, so this is not flaky.
        let probe = SysinfoProbe::new();
        let me = std::process::id();
        let got = probe.probe(&[me, 999_999]);

        let ours = got.get(&me).expect("our own process must be visible");
        assert!(ours.start_time_secs > 1_600_000_000, "start time looks wrong: {}", ours.start_time_secs);
        assert!(ours.exe_basename.is_some());
        assert!(!got.contains_key(&999_999), "a bogus pid must be absent");
    }
}
```

- [ ] **Step 3: Run the test to verify it fails**

```bash
cd src-tauri && cargo test sessions::probe 2>&1 | tail -20
```

Expected: compile errors — `cannot find type FakeProbe`.

- [ ] **Step 4: Write the minimal implementation**

Prepend to `src-tauri/src/sessions/probe.rs`:

```rust
use std::collections::HashMap;

/// What we need to know about a live process. Absence of a `ProcInfo` for a
/// pid means the process is not alive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    /// Unix SECONDS. Directly comparable with a session file's
    /// `startedAt / 1000` — no timezone parsing, which is the whole reason we
    /// use this rather than the `procStart` string.
    pub start_time_secs: i64,
    /// Basename of the executable, e.g. "claude" or "node". Phase 2 does not
    /// gate on it — start-time identity is the strong check — but it makes the
    /// real-process test meaningful and Phase 3 will use it to REFUSE a
    /// clearly-wrong process.
    pub exe_basename: Option<String>,
}

/// Read-only view of process state. Behind a trait so tests never depend on
/// the machine's real process table.
pub trait ProcessProbe: Send + Sync {
    /// Returns entries ONLY for pids that are currently alive.
    fn probe(&self, pids: &[u32]) -> HashMap<u32, ProcInfo>;
}

/// The real implementation.
///
/// Holds ONE `System` for the life of the probe. `System::new()` acquires a
/// mach port on macOS which sysinfo 0.35 never releases, and this probe is
/// called on every UI poll (~30/min) plus every tray update — so constructing
/// one per call would leak a port reference each time.
pub struct SysinfoProbe {
    sys: std::sync::Mutex<sysinfo::System>,
}

impl SysinfoProbe {
    pub fn new() -> Self {
        SysinfoProbe {
            sys: std::sync::Mutex::new(sysinfo::System::new()),
        }
    }
}

impl Default for SysinfoProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessProbe for SysinfoProbe {
    fn probe(&self, pids: &[u32]) -> HashMap<u32, ProcInfo> {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, UpdateKind};

        if pids.is_empty() {
            return HashMap::new();
        }
        let wanted: Vec<Pid> = pids.iter().map(|p| Pid::from_u32(*p)).collect();

        let mut sys = self.sys.lock().unwrap_or_else(|e| e.into_inner());
        // Refresh only these pids, and only the field we read. The `true`
        // prunes requested pids that have since died, so a reused `System`
        // cannot report a stale process as alive.
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&wanted),
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
        );

        let mut out = HashMap::new();
        for pid in pids {
            let Some(p) = sys.process(Pid::from_u32(*pid)) else {
                continue; // not alive
            };
            out.insert(
                *pid,
                ProcInfo {
                    start_time_secs: p.start_time() as i64,
                    exe_basename: p
                        .exe()
                        .and_then(|e| e.file_name())
                        .map(|n| n.to_string_lossy().to_string()),
                },
            );
        }
        out
    }
}

/// Test double. Build with `.alive(..)` / `.dead(..)`.
/// `#[cfg(test)]` because nothing in the shipping binary constructs it, and
/// without the gate clippy reports its three builders as dead code.
#[cfg(test)]
#[derive(Default)]
pub struct FakeProbe {
    live: HashMap<u32, ProcInfo>,
}

#[cfg(test)]
impl FakeProbe {
    pub fn new() -> Self {
        FakeProbe::default()
    }

    pub fn alive(mut self, pid: u32, start_time_secs: i64, exe_basename: &str) -> Self {
        self.live.insert(
            pid,
            ProcInfo {
                start_time_secs,
                exe_basename: Some(exe_basename.to_string()),
            },
        );
        self
    }

    /// Explicit for readability; a pid simply never added behaves the same.
    pub fn dead(self, _pid: u32) -> Self {
        self
    }
}

#[cfg(test)]
impl ProcessProbe for FakeProbe {
    fn probe(&self, pids: &[u32]) -> HashMap<u32, ProcInfo> {
        pids.iter()
            .filter_map(|p| self.live.get(p).map(|i| (*p, i.clone())))
            .collect()
    }
}
```

(`mod.rs` and the `mod sessions;` line were added in Step 2.)

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cd src-tauri && cargo test sessions::probe 2>&1 | tail -12
```

Expected: `test result: ok. 3 passed`.

- [ ] **Step 6: Confirm nothing regressed**

```bash
cd src-tauri && cargo test 2>&1 | grep -E "^test result: ok" | head -1
```

Expected: 124 passed (121 prior + 3 new).

- [ ] **Step 7: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/sessions src-tauri/src/lib.rs CLAUDE.md
git commit -m "feat(sessions): add process probe behind a trait

sysinfo pinned to 0.35: 0.37 needs rustc 1.88 and 0.39 needs 1.95, and
the toolchain here is 1.86. Recorded in CLAUDE.md."
```

---

### Task 2: Session registry reader

**Files:**
- Create: `src-tauri/src/sessions/registry.rs`
- Modify: `src-tauri/src/sessions/mod.rs`
- Test: inline in `src-tauri/src/sessions/registry.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `SessionFile { pid, session_id, started_at_ms, cwd, version, entrypoint, pid_domain, name, messaging_socket_path }`, `read_registry(&Path) -> Vec<SessionFile>`.

`kind` is present in the real JSON but nothing in Phase 2 renders it, so it is deliberately not carried — an unread field trips clippy's dead-code lint.

**Real shape** (all keys observed on this machine; `updatedAt`, `nameSince`, `nameSource`, `peerProtocol`, `peerFeatures`, `bridgeSessionId` also appear and are ignored):

```json
{ "pid": 12158, "sessionId": "36500922-…", "cwd": "/Users/…/claude-token-usage",
  "startedAt": 1789029119710, "procStart": "Thu Sep 10 08:31:58 2026",
  "version": "2.1.267", "kind": "interactive", "entrypoint": "claude-vscode",
  "pidDomain": "darwin", "name": "claude-token-usage-32",
  "messagingSocketPath": "/tmp/cc-socks/12158.sock" }
```

- [ ] **Step 1: Write the failing test**

Add `pub mod registry;` to `src-tauri/src/sessions/mod.rs` FIRST, so the next
step fails with a missing-function error rather than running zero tests. Then
create `src-tauri/src/sessions/registry.rs` with ONLY this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = r#"{
      "pid": 12158, "sessionId": "36500922-9f66-474d-a4db-95f06b891d4c",
      "cwd": "/Users/me/Projects/claude-token-usage",
      "startedAt": 1789029119710, "procStart": "Thu Sep 10 08:31:58 2026",
      "version": "2.1.267", "peerProtocol": 1, "peerFeatures": ["notify_idle"],
      "kind": "interactive", "entrypoint": "claude-vscode", "pidDomain": "darwin",
      "messagingSocketPath": "/tmp/cc-socks/12158.sock",
      "name": "claude-token-usage-32", "nameSource": "derived",
      "nameSince": 1789029119710, "updatedAt": 1789029200000,
      "bridgeSessionId": "session_01CzX5LMvcCkQQqgu9ezv4y9"
    }"#;

    fn write(dir: &std::path::Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).expect("write");
    }

    #[test]
    fn reads_a_real_session_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "12158.json", REAL);

        let found = read_registry(dir.path());
        assert_eq!(found.len(), 1);
        let s = &found[0];
        assert_eq!(s.pid, 12158);
        assert_eq!(s.session_id, "36500922-9f66-474d-a4db-95f06b891d4c");
        assert_eq!(s.cwd.as_deref(), Some("/Users/me/Projects/claude-token-usage"));
        assert_eq!(s.started_at_ms, 1789029119710);
        assert_eq!(s.version.as_deref(), Some("2.1.267"));
        assert_eq!(s.entrypoint.as_deref(), Some("claude-vscode"));
        assert_eq!(s.pid_domain.as_deref(), Some("darwin"));
        assert_eq!(s.name.as_deref(), Some("claude-token-usage-32"));
        assert_eq!(s.messaging_socket_path.as_deref(), Some("/tmp/cc-socks/12158.sock"));
    }

    #[test]
    fn ignores_non_json_siblings_including_key_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "12158.json", REAL);
        // The 0600 .key sibling must never be read or parsed.
        write(dir.path(), "12158.d69849c9.key", "secret-material");
        write(dir.path(), "notes.txt", "hello");

        assert_eq!(read_registry(dir.path()).len(), 1);
    }

    #[test]
    fn skips_a_malformed_file_without_losing_the_others() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "12158.json", REAL);
        write(dir.path(), "9999.json", "{ not json");

        let found = read_registry(dir.path());
        assert_eq!(found.len(), 1, "one bad file must not sink the whole read");
        assert_eq!(found[0].pid, 12158);
    }

    #[test]
    fn skips_a_file_missing_the_fields_we_cannot_work_without() {
        let dir = tempfile::tempdir().expect("tempdir");
        // No pid.
        write(dir.path(), "a.json", r#"{"sessionId":"s","startedAt":1}"#);
        // No sessionId.
        write(dir.path(), "b.json", r#"{"pid":5,"startedAt":1}"#);
        // No startedAt: identity cannot be verified, so the row is useless.
        write(dir.path(), "c.json", r#"{"pid":5,"sessionId":"s"}"#);
        assert!(read_registry(dir.path()).is_empty());
    }

    #[test]
    fn returns_results_sorted_by_pid_for_stable_ordering() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "300.json", &REAL.replace("12158", "300"));
        write(dir.path(), "100.json", &REAL.replace("12158", "100"));
        write(dir.path(), "200.json", &REAL.replace("12158", "200"));

        let pids: Vec<u32> = read_registry(dir.path()).iter().map(|s| s.pid).collect();
        assert_eq!(pids, vec![100, 200, 300]);
    }

    #[test]
    fn missing_directory_yields_empty_not_panic() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(read_registry(&dir.path().join("nope")).is_empty());
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd src-tauri && cargo test sessions::registry 2>&1 | tail -20
```

Expected: compile error, `cannot find function read_registry`.

- [ ] **Step 3: Write the minimal implementation**

Prepend to `src-tauri/src/sessions/registry.rs`:

```rust
use serde::Deserialize;
use std::path::Path;

/// One entry from `~/.claude/sessions/<pid>.json`. Every optional field is
/// genuinely optional in the wild; the three required ones are the minimum
/// needed to identify a session and verify it later.
#[derive(Debug, Clone)]
pub struct SessionFile {
    pub pid: u32,
    pub session_id: String,
    /// Epoch millis. `/1000` is directly comparable with a process start time.
    pub started_at_ms: i64,
    pub cwd: Option<String>,
    pub version: Option<String>,
    pub entrypoint: Option<String>,
    pub pid_domain: Option<String>,
    pub name: Option<String>,
    pub messaging_socket_path: Option<String>,
}

#[derive(Deserialize)]
struct RawSession {
    pid: Option<u32>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "startedAt")]
    started_at: Option<i64>,
    cwd: Option<String>,
    version: Option<String>,
    entrypoint: Option<String>,
    #[serde(rename = "pidDomain")]
    pid_domain: Option<String>,
    name: Option<String>,
    #[serde(rename = "messagingSocketPath")]
    messaging_socket_path: Option<String>,
}

/// Read every `*.json` in the sessions directory. Unreadable or incomplete
/// entries are skipped rather than failing the whole listing — the directory
/// is written by another process and can be caught mid-write.
pub fn read_registry(sessions_dir: &Path) -> Vec<SessionFile> {
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        // Only .json. The sibling .key files are 0600 secret material and must
        // never be read.
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(raw) = serde_json::from_str::<RawSession>(&text) else {
            continue;
        };
        let (Some(pid), Some(session_id), Some(started_at_ms)) =
            (raw.pid, raw.session_id, raw.started_at)
        else {
            continue;
        };
        out.push(SessionFile {
            pid,
            session_id,
            started_at_ms,
            cwd: raw.cwd,
            version: raw.version,
            entrypoint: raw.entrypoint,
            pid_domain: raw.pid_domain,
            name: raw.name,
            messaging_socket_path: raw.messaging_socket_path,
        });
    }

    out.sort_by_key(|s| s.pid);
    out
}
```

(The `mod` line was added in Step 1.)

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd src-tauri && cargo test sessions::registry 2>&1 | tail -12
```

Expected: `test result: ok. 6 passed`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/sessions/registry.rs src-tauri/src/sessions/mod.rs
git commit -m "feat(sessions): read the session registry tolerantly"
```

---

### Task 3: Reconcile registry against live processes

**Files:**
- Create: `src-tauri/src/sessions/reconcile.rs`
- Modify: `src-tauri/src/sessions/mod.rs`
- Test: inline in `src-tauri/src/sessions/reconcile.rs`

**Interfaces:**
- Consumes: `SessionFile` (Task 2), `ProcessProbe`/`FakeProbe` (Task 1).
- Produces: `SessionState { Live, Stale }`, `Reconciled { file: SessionFile, state: SessionState }`, `START_TIME_TOLERANCE_SECS`, `reconcile(&[SessionFile], &dyn ProcessProbe) -> Vec<Reconciled>`.

The matched process start time is deliberately NOT carried on `Reconciled`: nothing downstream reads it (uptime comes from `startedAt`, the session's own record), and an unread field trips clippy's dead-code lint.

**The identity rule:** a session is `Live` only when its pid is alive AND `|start_time_secs - started_at_ms/1000| <= 120`. Measured skew on six real sessions was 0–8 s, so 120 s is generous while still rejecting a recycled pid — a different process that inherited the number will almost never have started within two minutes of the recorded session.

- [ ] **Step 1: Write the failing test**

Add `pub mod reconcile;` to `src-tauri/src/sessions/mod.rs` FIRST. Then create
`src-tauri/src/sessions/reconcile.rs` with ONLY this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::probe::FakeProbe;
    use crate::sessions::registry::SessionFile;

    fn file(pid: u32, started_at_ms: i64) -> SessionFile {
        SessionFile {
            pid,
            session_id: format!("s-{pid}"),
            started_at_ms,
            cwd: Some("/p".to_string()),
            version: Some("2.1.267".to_string()),
            kind: Some("interactive".to_string()),
            entrypoint: Some("claude-vscode".to_string()),
            pid_domain: Some("darwin".to_string()),
            name: Some(format!("proj-{pid}")),
            messaging_socket_path: Some(format!("/tmp/cc-socks/{pid}.sock")),
        }
    }

    #[test]
    fn a_live_pid_with_matching_start_time_is_live() {
        let files = vec![file(100, 1_789_000_000_000)];
        let probe = FakeProbe::new().alive(100, 1_789_000_000, "claude");
        let got = reconcile(&files, &probe);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].state, SessionState::Live);
    }

    #[test]
    fn a_small_skew_is_still_live() {
        // Registration happens 1-8s after spawn in real data.
        let files = vec![file(100, 1_789_000_008_000)];
        let probe = FakeProbe::new().alive(100, 1_789_000_000, "claude");
        assert_eq!(reconcile(&files, &probe)[0].state, SessionState::Live);
    }

    #[test]
    fn a_dead_pid_is_stale() {
        let files = vec![file(100, 1_789_000_000_000)];
        let probe = FakeProbe::new(); // nothing alive
        let got = reconcile(&files, &probe);
        assert_eq!(got[0].state, SessionState::Stale);
    }

    #[test]
    fn a_recycled_pid_is_stale_not_live() {
        // Same pid, but the process started an hour after the session did:
        // the number was reused by something else.
        let files = vec![file(100, 1_789_000_000_000)];
        let probe = FakeProbe::new().alive(100, 1_789_003_600, "claude");
        let got = reconcile(&files, &probe);
        assert_eq!(
            got[0].state,
            SessionState::Stale,
            "a pid whose start time disagrees is not this session"
        );
    }

    #[test]
    fn tolerance_boundary_is_inclusive() {
        let base = 1_789_000_000i64;
        let inside = vec![file(100, (base + START_TIME_TOLERANCE_SECS) * 1000)];
        let outside = vec![file(100, (base + START_TIME_TOLERANCE_SECS + 1) * 1000)];
        let probe = FakeProbe::new().alive(100, base, "claude");
        assert_eq!(reconcile(&inside, &probe)[0].state, SessionState::Live);
        assert_eq!(reconcile(&outside, &probe)[0].state, SessionState::Stale);
    }

    #[test]
    fn a_non_darwin_pid_domain_is_skipped_entirely() {
        // That pid belongs to another machine's namespace; probing it locally
        // would be meaningless and could match an unrelated local process.
        let mut f = file(100, 1_789_000_000_000);
        f.pid_domain = Some("linux".to_string());
        let probe = FakeProbe::new().alive(100, 1_789_000_000, "claude");
        assert!(reconcile(&[f], &probe).is_empty());
    }

    #[test]
    fn a_missing_pid_domain_is_treated_as_local() {
        let mut f = file(100, 1_789_000_000_000);
        f.pid_domain = None;
        let probe = FakeProbe::new().alive(100, 1_789_000_000, "claude");
        assert_eq!(reconcile(&[f], &probe).len(), 1);
    }

    #[test]
    fn the_exe_basename_is_not_required_to_classify_live() {
        // Phase 2 does not gate on the binary: an npm-global install runs under
        // node, and start-time identity is already the strong check. Phase 3
        // uses cmd only to REFUSE a clearly-wrong process.
        let files = vec![file(100, 1_789_000_000_000)];
        let probe = FakeProbe::new().alive(100, 1_789_000_000, "node");
        assert_eq!(reconcile(&files, &probe)[0].state, SessionState::Live);
    }

    #[test]
    fn probes_every_pid_in_one_call_and_preserves_order() {
        let files = vec![
            file(100, 1_789_000_000_000),
            file(200, 1_789_000_000_000),
            file(300, 1_789_000_000_000),
        ];
        let probe = FakeProbe::new()
            .alive(100, 1_789_000_000, "claude")
            .alive(300, 1_789_000_000, "claude");
        let got = reconcile(&files, &probe);
        let pairs: Vec<(u32, SessionState)> =
            got.iter().map(|r| (r.file.pid, r.state)).collect();
        assert_eq!(
            pairs,
            vec![
                (100, SessionState::Live),
                (200, SessionState::Stale),
                (300, SessionState::Live)
            ]
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd src-tauri && cargo test sessions::reconcile 2>&1 | tail -20
```

Expected: compile error, `cannot find function reconcile`.

- [ ] **Step 3: Write the minimal implementation**

Prepend to `src-tauri/src/sessions/reconcile.rs`:

```rust
use crate::sessions::probe::ProcessProbe;
use crate::sessions::registry::SessionFile;

/// How far a process's start time may differ from the session's recorded
/// `startedAt` and still be considered the same session. Registration happens
/// 1-8s after spawn in real data, so this is generous; its job is to reject a
/// recycled pid, which would have to have started within two minutes of the
/// original to slip through.
pub const START_TIME_TOLERANCE_SECS: i64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Registered, alive, and the process start time matches.
    Live,
    /// Registered, but no live process matches it. Its files are debris.
    Stale,
}

#[derive(Debug, Clone)]
pub struct Reconciled {
    pub file: SessionFile,
    pub state: SessionState,
}

/// Classify every registered session against live process state. Input order
/// is preserved. Sessions whose pid belongs to another machine's namespace are
/// dropped entirely rather than misclassified.
pub fn reconcile(files: &[SessionFile], probe: &dyn ProcessProbe) -> Vec<Reconciled> {
    let local: Vec<&SessionFile> = files
        .iter()
        .filter(|f| match f.pid_domain.as_deref() {
            None | Some("darwin") => true,
            Some(_) => false,
        })
        .collect();

    let pids: Vec<u32> = local.iter().map(|f| f.pid).collect();
    let live = probe.probe(&pids);

    local
        .into_iter()
        .map(|f| {
            let info = live.get(&f.pid);
            let matches = info.is_some_and(|i| {
                (i.start_time_secs - f.started_at_ms / 1000).abs() <= START_TIME_TOLERANCE_SECS
            });
            Reconciled {
                file: f.clone(),
                state: if matches {
                    SessionState::Live
                } else {
                    SessionState::Stale
                },
            }
        })
        .collect()
}
```

(The `mod` line was added in Step 1.) `is_some_and` has been stable since rustc 1.70.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd src-tauri && cargo test sessions::reconcile 2>&1 | tail -12
```

Expected: `test result: ok. 9 passed`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/sessions/reconcile.rs src-tauri/src/sessions/mod.rs
git commit -m "feat(sessions): classify registered sessions as live or stale"
```

---

### Task 4: Expose per-session usage from the cache

**Files:**
- Modify: `src-tauri/src/usage/worker.rs`
- Test: inline in `src-tauri/src/usage/worker.rs`

**Interfaces:**
- Consumes: `UsageCache`, `FileEntry`, `SessionRollup`, `AgentRollup`, `TokenCounts::billable_total` (Phase 1).
- Produces: `SessionUsage { session_id, cwd, git_branch, first_ts, last_ts, message_count, tokens, agents }`, `AgentUsage { agent_id, agent_type, tokens, last_ts }`, `UsageWorker::session_usage() -> HashMap<String, SessionUsage>`.

**Why this is needed:** the Phase 1 adapter collapses everything into `StatsCache`, which has no per-session view. The sessions panel needs tokens and last-activity for one specific `sessionId`. Critically, **a session spans multiple files** — its main transcript plus one file per subagent, all sharing the parent's `sessionId` — so rollups must be merged across files by `session_id`, and agents merged by `agent_id`.

- [ ] **Step 1: Write the failing test**

Add to the existing `mod tests` in `src-tauri/src/usage/worker.rs`:

```rust
    #[test]
    fn session_usage_merges_a_session_split_across_parent_and_subagent_files() {
        use crate::usage::types::{AgentRollup, FileEntry, TokenCounts, UsageCache};

        let dir = tempfile::tempdir().expect("tempdir");
        let roots = resolve_with(Some(dir.path().join("claude")), None, None);
        let worker = UsageWorker::new(roots, dir.path().join("cache/c.json"));

        // Hand-build a cache: one session across two files, plus an unrelated one.
        {
            let mut guard = worker.cache.lock().expect("lock");
            *guard = UsageCache::new(0);

            let mut parent = FileEntry::default();
            parent.session.session_id = "s-1".to_string();
            parent.session.cwd = Some("/p/proj".to_string());
            parent.session.git_branch = Some("main".to_string());
            parent.session.first_ts = 1_000;
            parent.session.last_ts = 5_000;
            parent.session.message_count = 4;
            parent.session.by_model.insert(
                "claude-opus-5".to_string(),
                TokenCounts { input: 1, output: 2, cache_read: 3, cache_creation: 4, ..Default::default() },
            );

            let mut sub = FileEntry::default();
            sub.session.session_id = "s-1".to_string(); // same session!
            sub.session.first_ts = 2_000;
            sub.session.last_ts = 9_000;               // later than the parent
            sub.session.message_count = 3;
            sub.session.by_model.insert(
                "claude-sonnet-5".to_string(),
                TokenCounts { input: 10, output: 20, ..Default::default() },
            );
            sub.agents.insert(
                "a1".to_string(),
                AgentRollup {
                    agent_id: "a1".to_string(),
                    agent_type: Some("Explore".to_string()),
                    last_ts: 8_000,
                    tokens: TokenCounts { output: 50, ..Default::default() },
                },
            );

            let mut other = FileEntry::default();
            other.session.session_id = "s-2".to_string();
            other.session.last_ts = 7_000;

            guard.files.insert("/parent.jsonl".into(), parent);
            guard.files.insert("/parent/subagents/a.jsonl".into(), sub);
            guard.files.insert("/other.jsonl".into(), other);
        }

        let usage = worker.session_usage();
        assert_eq!(usage.len(), 2, "two distinct sessions");

        let s1 = usage.get("s-1").expect("s-1");
        assert_eq!(s1.first_ts, 1_000, "earliest across both files");
        assert_eq!(s1.last_ts, 9_000, "latest across both files");
        assert_eq!(s1.message_count, 7, "4 + 3");
        // billable totals: (1+2+3+4) + (10+20) = 10 + 30
        assert_eq!(s1.tokens, 40);
        assert_eq!(s1.cwd.as_deref(), Some("/p/proj"));
        assert_eq!(s1.git_branch.as_deref(), Some("main"));
        assert_eq!(s1.agents.len(), 1);
        assert_eq!(s1.agents[0].agent_id, "a1");
        assert_eq!(s1.agents[0].agent_type.as_deref(), Some("Explore"));
        assert_eq!(s1.agents[0].tokens, 50);
        assert_eq!(s1.agents[0].last_ts, 8_000);
    }

    #[test]
    fn session_usage_merges_one_agent_appearing_in_two_files() {
        use crate::usage::types::{AgentRollup, FileEntry, TokenCounts, UsageCache};

        let dir = tempfile::tempdir().expect("tempdir");
        let roots = resolve_with(Some(dir.path().join("claude")), None, None);
        let worker = UsageWorker::new(roots, dir.path().join("cache/c.json"));

        {
            let mut guard = worker.cache.lock().expect("lock");
            *guard = UsageCache::new(0);
            for (path, tokens, last) in [("/a.jsonl", 10u64, 100i64), ("/b.jsonl", 5, 300)] {
                let mut e = FileEntry::default();
                e.session.session_id = "s-1".to_string();
                e.agents.insert(
                    "a1".to_string(),
                    AgentRollup {
                        agent_id: "a1".to_string(),
                        agent_type: None,
                        last_ts: last,
                        tokens: TokenCounts { output: tokens, ..Default::default() },
                    },
                );
                guard.files.insert(path.into(), e);
            }
        }

        let usage = worker.session_usage();
        let agents = &usage.get("s-1").expect("s-1").agents;
        assert_eq!(agents.len(), 1, "same agent_id must merge, not duplicate");
        assert_eq!(agents[0].tokens, 15);
        assert_eq!(agents[0].last_ts, 300, "latest wins");
    }

    #[test]
    fn session_usage_skips_entries_with_no_session_id() {
        use crate::usage::types::{FileEntry, UsageCache};

        let dir = tempfile::tempdir().expect("tempdir");
        let roots = resolve_with(Some(dir.path().join("claude")), None, None);
        let worker = UsageWorker::new(roots, dir.path().join("cache/c.json"));
        {
            let mut guard = worker.cache.lock().expect("lock");
            *guard = UsageCache::new(0);
            guard.files.insert("/empty.jsonl".into(), FileEntry::default());
        }
        assert!(worker.session_usage().is_empty());
    }

    #[test]
    fn session_usage_sorts_agents_by_last_activity_descending() {
        use crate::usage::types::{AgentRollup, FileEntry, TokenCounts, UsageCache};

        let dir = tempfile::tempdir().expect("tempdir");
        let roots = resolve_with(Some(dir.path().join("claude")), None, None);
        let worker = UsageWorker::new(roots, dir.path().join("cache/c.json"));
        {
            let mut guard = worker.cache.lock().expect("lock");
            *guard = UsageCache::new(0);
            let mut e = FileEntry::default();
            e.session.session_id = "s-1".to_string();
            for (id, last) in [("old", 100i64), ("newest", 900), ("mid", 500)] {
                e.agents.insert(
                    id.to_string(),
                    AgentRollup {
                        agent_id: id.to_string(),
                        agent_type: None,
                        last_ts: last,
                        tokens: TokenCounts::default(),
                    },
                );
            }
            guard.files.insert("/a.jsonl".into(), e);
        }
        let ids: Vec<String> = worker
            .session_usage()
            .get("s-1")
            .expect("s-1")
            .agents
            .iter()
            .map(|a| a.agent_id.clone())
            .collect();
        assert_eq!(ids, vec!["newest", "mid", "old"], "most recent first");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cd src-tauri && cargo test usage::worker::tests::session_usage 2>&1 | tail -20
```

Expected: compile error, `no method named session_usage`.

- [ ] **Step 3: Write the minimal implementation**

Add to `src-tauri/src/usage/worker.rs`, alongside `Diagnostics`:

```rust
/// One subagent's contribution to a session. Read-only: subagents have no pid
/// of their own and cannot be acted on individually.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsage {
    pub agent_id: String,
    /// Absent on some subagent records, so genuinely optional.
    pub agent_type: Option<String>,
    pub tokens: u64,
    pub last_ts: i64,
}

/// Per-session view of the usage cache, merged across every file that belongs
/// to the session. A session's records live in its own transcript PLUS one file
/// per subagent, all sharing the parent's sessionId, so a per-file view would
/// undercount and split the activity timeline.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsage {
    pub session_id: String,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub first_ts: i64,
    /// Last record of ANY type: this is what idle age is measured from.
    pub last_ts: i64,
    pub message_count: u64,
    /// Billable total (input + output + cache read + cache creation).
    pub tokens: u64,
    pub agents: Vec<AgentUsage>,
}
```

and this method to `impl UsageWorker`:

```rust
    /// Per-session usage, keyed by session id, merged across files.
    pub fn session_usage(&self) -> HashMap<String, SessionUsage> {
        use std::collections::hash_map::Entry;

        let guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: HashMap<String, SessionUsage> = HashMap::new();
        // agent_id -> (tokens, last_ts, agent_type) per session.
        let mut agents: HashMap<String, HashMap<String, AgentUsage>> = HashMap::new();

        for entry in guard.files.values() {
            let sid = &entry.session.session_id;
            if sid.is_empty() {
                continue;
            }

            let tokens: u64 = entry
                .session
                .by_model
                .values()
                .map(|c| c.billable_total())
                .sum();

            match out.entry(sid.clone()) {
                Entry::Vacant(v) => {
                    v.insert(SessionUsage {
                        session_id: sid.clone(),
                        cwd: entry.session.cwd.clone(),
                        git_branch: entry.session.git_branch.clone(),
                        first_ts: entry.session.first_ts,
                        last_ts: entry.session.last_ts,
                        message_count: entry.session.message_count,
                        tokens,
                        agents: Vec::new(),
                    });
                }
                Entry::Occupied(mut o) => {
                    let s = o.get_mut();
                    if s.cwd.is_none() {
                        s.cwd = entry.session.cwd.clone();
                    }
                    if s.git_branch.is_none() {
                        s.git_branch = entry.session.git_branch.clone();
                    }
                    if entry.session.first_ts != 0
                        && (s.first_ts == 0 || entry.session.first_ts < s.first_ts)
                    {
                        s.first_ts = entry.session.first_ts;
                    }
                    if entry.session.last_ts > s.last_ts {
                        s.last_ts = entry.session.last_ts;
                    }
                    s.message_count += entry.session.message_count;
                    s.tokens += tokens;
                }
            }

            let per_session = agents.entry(sid.clone()).or_default();
            for (agent_id, a) in &entry.agents {
                let slot = per_session.entry(agent_id.clone()).or_insert(AgentUsage {
                    agent_id: agent_id.clone(),
                    agent_type: a.agent_type.clone(),
                    tokens: 0,
                    last_ts: 0,
                });
                if slot.agent_type.is_none() {
                    slot.agent_type = a.agent_type.clone();
                }
                slot.tokens += a.tokens.billable_total();
                if a.last_ts > slot.last_ts {
                    slot.last_ts = a.last_ts;
                }
            }
        }

        for (sid, per_session) in agents {
            if let Some(s) = out.get_mut(&sid) {
                let mut list: Vec<AgentUsage> = per_session.into_values().collect();
                // Most recently active first — that is the useful ordering when
                // asking "what is this session doing right now".
                list.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
                s.agents = list;
            }
        }

        out
    }
```

`HashMap` is already imported in this file via the `Diagnostics`/cache code; add `use std::collections::HashMap;` if it is not.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd src-tauri && cargo test usage::worker 2>&1 | tail -14
```

Expected: all worker tests pass, including the 4 new ones.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/usage/worker.rs
git commit -m "feat(usage): expose per-session usage merged across files"
```

---

### Task 5: Assemble the session rows

**Files:**
- Create: `src-tauri/src/sessions/rows.rs`
- Modify: `src-tauri/src/sessions/mod.rs`
- Test: inline in `src-tauri/src/sessions/rows.rs`

**Interfaces:**
- Consumes: `Reconciled`, `SessionState` (Task 3); `SessionUsage`, `AgentUsage` (Task 4).
- Produces: `SessionRow` and `AgentRow` (both camelCase for the frontend), `ACTIVE_THRESHOLD_SECS`, `build_rows(Vec<Reconciled>, &HashMap<String, SessionUsage>, i64) -> Vec<SessionRow>`.

`build_rows` takes `now_ms` explicitly so every test is deterministic.

- [ ] **Step 1: Write the failing test**

Add `pub mod rows;` to `src-tauri/src/sessions/mod.rs` FIRST. Then create
`src-tauri/src/sessions/rows.rs` with ONLY this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::registry::SessionFile;
    use crate::usage::worker::{AgentUsage, SessionUsage};
    use std::collections::HashMap;

    const NOW: i64 = 1_789_100_000_000;

    fn rec(pid: u32, sid: &str, started_ms: i64, state: SessionState) -> Reconciled {
        Reconciled {
            file: SessionFile {
                pid,
                session_id: sid.to_string(),
                started_at_ms: started_ms,
                cwd: Some("/Users/me/Projects/my-app".to_string()),
                version: Some("2.1.267".to_string()),
                kind: Some("interactive".to_string()),
                entrypoint: Some("claude-vscode".to_string()),
                pid_domain: Some("darwin".to_string()),
                name: Some("my-app-42".to_string()),
                messaging_socket_path: Some(format!("/tmp/cc-socks/{pid}.sock")),
            },
            state,
        }
    }

    fn usage(sid: &str, last_ts: i64, tokens: u64) -> SessionUsage {
        SessionUsage {
            session_id: sid.to_string(),
            cwd: Some("/Users/me/Projects/my-app".to_string()),
            git_branch: Some("main".to_string()),
            first_ts: last_ts - 60_000,
            last_ts,
            message_count: 12,
            tokens,
            agents: Vec::new(),
        }
    }

    #[test]
    fn a_live_session_reports_uptime_idle_and_tokens() {
        let started = NOW - 3_600_000; // one hour ago
        let recs = vec![rec(100, "s-1", started, SessionState::Live)];
        let mut u = HashMap::new();
        u.insert("s-1".to_string(), usage("s-1", NOW - 120_000, 1_234_567));

        let rows = build_rows(recs, &u, NOW);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.state, "live");
        assert_eq!(r.pid, 100);
        assert_eq!(r.name, "my-app-42");
        assert_eq!(r.project, "my-app", "basename of cwd");
        assert_eq!(r.git_branch.as_deref(), Some("main"));
        assert_eq!(r.uptime_secs, Some(3_600));
        assert_eq!(r.idle_secs, Some(120));
        assert_eq!(r.tokens, 1_234_567);
        assert_eq!(r.message_count, 12);
        assert!(r.is_active, "120s idle is inside the 5min active window");
    }

    #[test]
    fn active_is_true_only_within_the_threshold() {
        let started = NOW - 3_600_000;
        let mut u = HashMap::new();
        u.insert("s-1".to_string(), usage("s-1", NOW - 10_000, 1)); // 10s idle
        let rows = build_rows(vec![rec(100, "s-1", started, SessionState::Live)], &u, NOW);
        assert!(rows[0].is_active, "10s idle is active");

        let mut u2 = HashMap::new();
        let past = ACTIVE_THRESHOLD_SECS * 1000 + 1_000;
        u2.insert("s-1".to_string(), usage("s-1", NOW - past, 1));
        let rows2 = build_rows(vec![rec(100, "s-1", started, SessionState::Live)], &u2, NOW);
        assert!(!rows2[0].is_active, "past the threshold is idle");
    }

    #[test]
    fn a_live_session_with_no_transcript_reports_no_activity_rather_than_a_bogus_age() {
        // Observed in real data: a registered session can have no transcript
        // file at all.
        let started = NOW - 30_000;
        let rows = build_rows(
            vec![rec(100, "s-nothing", started, SessionState::Live)],
            &HashMap::new(),
            NOW,
        );
        let r = &rows[0];
        assert_eq!(r.last_activity_ms, None);
        assert_eq!(r.idle_secs, None);
        assert_eq!(r.tokens, 0);
        assert_eq!(r.message_count, 0);
        assert_eq!(r.uptime_secs, Some(30), "uptime still known from startedAt");
        assert!(!r.is_active, "unknown activity is not active");
    }

    #[test]
    fn a_stale_session_has_no_uptime_and_is_removable() {
        let rows = build_rows(
            vec![rec(100, "s-1", NOW - 3_600_000, SessionState::Stale)],
            &HashMap::new(),
            NOW,
        );
        let r = &rows[0];
        assert_eq!(r.state, "stale");
        assert_eq!(r.uptime_secs, None, "a dead process has no uptime");
        assert!(r.removable, "stale registrations are the one thing this phase can clear");
        assert!(!r.is_active);
    }

    #[test]
    fn a_live_session_is_not_removable() {
        let rows = build_rows(
            vec![rec(100, "s-1", NOW - 1_000, SessionState::Live)],
            &HashMap::new(),
            NOW,
        );
        assert!(!rows[0].removable, "never offer to delete a live session's files");
    }

    #[test]
    fn subagents_become_nested_rows_marked_not_killable() {
        let mut s = usage("s-1", NOW - 1_000, 100);
        s.agents = vec![
            AgentUsage { agent_id: "a1".to_string(), agent_type: Some("Explore".to_string()), tokens: 210_000, last_ts: NOW - 5_000 },
            AgentUsage { agent_id: "a2".to_string(), agent_type: None, tokens: 84_000, last_ts: NOW - 9_000 },
        ];
        let mut u = HashMap::new();
        u.insert("s-1".to_string(), s);

        let rows = build_rows(vec![rec(100, "s-1", NOW - 60_000, SessionState::Live)], &u, NOW);
        let agents = &rows[0].agents;
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].agent_type.as_deref(), Some("Explore"));
        assert_eq!(agents[0].tokens, 210_000);
        assert_eq!(agents[0].idle_secs, 5);
        assert_eq!(agents[1].agent_type, None, "unlabelled subagents stay unlabelled");
    }

    #[test]
    fn live_rows_sort_before_stale_and_within_a_group_by_activity() {
        let recs = vec![
            rec(100, "s-quiet", NOW - 10_000, SessionState::Live),
            rec(200, "s-dead", NOW - 10_000, SessionState::Stale),
            rec(300, "s-busy", NOW - 10_000, SessionState::Live),
        ];
        let mut u = HashMap::new();
        u.insert("s-quiet".to_string(), usage("s-quiet", NOW - 600_000, 1));
        u.insert("s-busy".to_string(), usage("s-busy", NOW - 1_000, 1));

        let rows = build_rows(recs, &u, NOW);
        let order: Vec<u32> = rows.iter().map(|r| r.pid).collect();
        assert_eq!(order, vec![300, 100, 200], "live by recency, then stale");
    }

    #[test]
    fn a_cwd_that_is_root_or_missing_does_not_panic() {
        let mut r1 = rec(100, "s-1", NOW - 1_000, SessionState::Live);
        r1.file.cwd = Some("/".to_string());
        let mut r2 = rec(200, "s-2", NOW - 1_000, SessionState::Live);
        r2.file.cwd = None;

        let rows = build_rows(vec![r1, r2], &HashMap::new(), NOW);
        assert_eq!(rows.len(), 2);
        // Whatever the fallback is, it must be non-empty and not a crash.
        assert!(!rows[0].project.is_empty());
        assert!(!rows[1].project.is_empty());
    }

    #[test]
    fn a_last_activity_in_the_future_clamps_to_zero_idle() {
        // Clock skew between the transcript writer and us must not produce a
        // negative duration.
        let mut u = HashMap::new();
        u.insert("s-1".to_string(), usage("s-1", NOW + 30_000, 1));
        let rows = build_rows(vec![rec(100, "s-1", NOW - 1_000, SessionState::Live)], &u, NOW);
        assert_eq!(rows[0].idle_secs, Some(0));
        assert!(rows[0].is_active);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd src-tauri && cargo test sessions::rows 2>&1 | tail -20
```

Expected: compile error, `cannot find function build_rows`.

- [ ] **Step 3: Write the minimal implementation**

Prepend to `src-tauri/src/sessions/rows.rs`:

```rust
use crate::sessions::reconcile::{Reconciled, SessionState};
use crate::usage::worker::SessionUsage;
use serde::Serialize;
use std::collections::HashMap;

/// A session with activity newer than this is shown as active rather than idle.
pub const ACTIVE_THRESHOLD_SECS: i64 = 300;

/// A subagent belonging to a session. Read-only by nature: subagents run
/// inside their parent process and have no pid of their own.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRow {
    pub agent_id: String,
    pub agent_type: Option<String>,
    pub tokens: u64,
    pub idle_secs: i64,
    /// Always false. Present so the UI never has to special-case the type.
    pub killable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    /// "live" or "stale". A string rather than an enum so the frontend can
    /// switch on it without a generated binding.
    pub state: String,
    pub pid: u32,
    pub session_id: String,
    pub name: String,
    pub cwd: Option<String>,
    /// Last path component of cwd, for the compact row label.
    pub project: String,
    pub git_branch: Option<String>,
    pub entrypoint: Option<String>,
    pub version: Option<String>,
    pub started_at_ms: i64,
    /// None for a stale row: the process is gone, so it has no uptime.
    pub uptime_secs: Option<i64>,
    /// None when the session has no transcript yet.
    pub last_activity_ms: Option<i64>,
    /// None when the session has no transcript yet. Clamped at 0.
    pub idle_secs: Option<i64>,
    pub tokens: u64,
    pub message_count: u64,
    pub is_active: bool,
    /// True only for stale rows: the one action this phase offers.
    pub removable: bool,
    pub agents: Vec<AgentRow>,
}

fn project_label(cwd: Option<&str>) -> String {
    cwd.and_then(|c| {
        std::path::Path::new(c)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
    })
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| "unknown".to_string())
}

/// Join reconciled registry entries with cached per-session usage.
/// `now_ms` is a parameter so every test is deterministic.
pub fn build_rows(
    reconciled: Vec<Reconciled>,
    usage: &HashMap<String, SessionUsage>,
    now_ms: i64,
) -> Vec<SessionRow> {
    let mut rows: Vec<SessionRow> = reconciled
        .into_iter()
        .map(|r| {
            let u = usage.get(&r.file.session_id);
            let live = r.state == SessionState::Live;

            let last_activity_ms = u.map(|u| u.last_ts).filter(|t| *t != 0);
            // Clamp: the transcript writer's clock may be marginally ahead.
            let idle_secs = last_activity_ms.map(|t| ((now_ms - t) / 1000).max(0));

            SessionRow {
                state: if live { "live" } else { "stale" }.to_string(),
                pid: r.file.pid,
                name: r
                    .file
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("pid {}", r.file.pid)),
                project: project_label(
                    u.and_then(|u| u.cwd.as_deref()).or(r.file.cwd.as_deref()),
                ),
                cwd: r.file.cwd.clone().or_else(|| u.and_then(|u| u.cwd.clone())),
                git_branch: u.and_then(|u| u.git_branch.clone()),
                entrypoint: r.file.entrypoint.clone(),
                version: r.file.version.clone(),
                started_at_ms: r.file.started_at_ms,
                uptime_secs: if live {
                    Some(((now_ms - r.file.started_at_ms) / 1000).max(0))
                } else {
                    None
                },
                last_activity_ms,
                idle_secs,
                tokens: u.map(|u| u.tokens).unwrap_or(0),
                message_count: u.map(|u| u.message_count).unwrap_or(0),
                is_active: live
                    && idle_secs.is_some_and(|s| s <= ACTIVE_THRESHOLD_SECS),
                removable: !live,
                agents: u
                    .map(|u| {
                        u.agents
                            .iter()
                            .map(|a| AgentRow {
                                agent_id: a.agent_id.clone(),
                                agent_type: a.agent_type.clone(),
                                tokens: a.tokens,
                                idle_secs: ((now_ms - a.last_ts) / 1000).max(0),
                                killable: false,
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                session_id: r.file.session_id,
            }
        })
        .collect();

    // Live first, then most recently active within each group. A row with no
    // known activity sorts last inside its group.
    rows.sort_by(|a, b| {
        let live_rank = |r: &SessionRow| if r.state == "live" { 0 } else { 1 };
        live_rank(a).cmp(&live_rank(b)).then_with(|| {
            let key = |r: &SessionRow| r.idle_secs.unwrap_or(i64::MAX);
            key(a).cmp(&key(b))
        })
    });

    rows
}
```

(The `mod` line was added in Step 1.)

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd src-tauri && cargo test sessions::rows 2>&1 | tail -14
```

Expected: `test result: ok. 9 passed`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/sessions/rows.rs src-tauri/src/sessions/mod.rs
git commit -m "feat(sessions): assemble session rows from registry, processes and usage"
```

---

### Task 6: Remove a stale registration

This is the only code in Phase 2 that writes anything under `~/.claude`. It deletes files, so it is written defensively and tested against a fake filesystem boundary.

**Files:**
- Create: `src-tauri/src/sessions/registration.rs`
- Modify: `src-tauri/src/sessions/mod.rs`
- Test: inline in `src-tauri/src/sessions/registration.rs`

**Interfaces:**
- Consumes: `SessionFile` (Task 2), `ProcessProbe` (Task 1).
- Produces: `RemovalOutcome { removed: Vec<PathBuf>, skipped: Vec<String> }`, `remove_registration(&Path, u32, &dyn ProcessProbe) -> Result<RemovalOutcome, String>`, and the seam `remove_registration_in(&Path, u32, &dyn ProcessProbe, &Path) -> Result<RemovalOutcome, String>`.

**Safety rules, all tested:**
1. Re-read the session file and **re-verify the pid is dead** at call time. A session that came back to life between the UI render and the click must be refused.
2. **Refuse a non-darwin `pidDomain`.** `reconcile` filters those out of the UI, but this command is callable directly; a foreign pid number probed locally would look dead and we would delete another machine's registration.
3. Delete only: `<sessions_dir>/<pid>.json`, any `<sessions_dir>/<pid>.<hex>.key`, and a socket that is **declared by that session's own JSON**, named exactly `<pid>.sock`, and sitting directly in the socket directory.
4. Never touch a directory, never glob beyond the `<pid>.` first dot-component, never follow a symlink.

**The socket is a socket, not a file.** Real entries in `/tmp/cc-socks` are Unix domain sockets — `srw-------`, so `S_ISSOCK` is true and `S_ISREG` is FALSE. A removal guard written as `metadata.is_file()` therefore refuses every real socket while passing any test that creates the fixture with `fs::write`. The `.json`/`.key` paths keep the strict regular-file rule; the declared socket accepts a socket type as well, and the test binds a real `UnixListener` so the fixture cannot hide the bug.

**The socket directory is injectable** so tests never touch the real `/tmp/cc-socks`, where they could collide with a live session's socket or with a parallel test run.

- [ ] **Step 1: Write the failing test**

Add `pub mod registration;` to `src-tauri/src/sessions/mod.rs` FIRST. Then
create `src-tauri/src/sessions/registration.rs` with ONLY this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::probe::FakeProbe;
    use std::path::{Path, PathBuf};

    /// Writes a registration, its .key sibling, and returns (sessions_dir, socket_dir).
    fn setup(root: &Path, pid: u32, sock: Option<&str>) -> (PathBuf, PathBuf) {
        let sessions = root.join("sessions");
        let socks = root.join("socks");
        std::fs::create_dir_all(&sessions).expect("mkdir sessions");
        std::fs::create_dir_all(&socks).expect("mkdir socks");
        let sock_line = sock
            .map(|s| format!(r#","messagingSocketPath":"{s}""#))
            .unwrap_or_default();
        std::fs::write(
            sessions.join(format!("{pid}.json")),
            format!(
                r#"{{"pid":{pid},"sessionId":"s-{pid}","startedAt":1789000000000,"pidDomain":"darwin"{sock_line}}}"#
            ),
        )
        .expect("write json");
        std::fs::write(sessions.join(format!("{pid}.abc123def456.key")), "secret")
            .expect("write key");
        (sessions, socks)
    }

    /// Bind a REAL Unix domain socket. The path survives the listener being
    /// dropped, which is exactly the leftover state we are cleaning up.
    fn bind_socket(path: &Path) {
        let l = std::os::unix::net::UnixListener::bind(path).expect("bind socket");
        drop(l);
        let md = std::fs::symlink_metadata(path).expect("stat");
        use std::os::unix::fs::FileTypeExt;
        assert!(md.file_type().is_socket(), "fixture must be a real socket");
        assert!(!md.is_file(), "a socket is not a regular file - that is the point");
    }

    #[test]
    fn removes_the_json_and_key_of_a_dead_session() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (sessions, socks) = setup(tmp.path(), 4242, None);

        let out = remove_registration_in(&sessions, 4242, &FakeProbe::new(), &socks).expect("ok");

        assert!(!sessions.join("4242.json").exists());
        assert!(!sessions.join("4242.abc123def456.key").exists());
        assert_eq!(out.removed.len(), 2);
    }

    #[test]
    fn removes_a_real_unix_socket_declared_by_the_session() {
        // The regression test for the socket type. A guard that only accepts
        // regular files refuses this and the feature silently does nothing.
        let tmp = tempfile::tempdir().expect("tempdir");
        let socks = tmp.path().join("socks");
        std::fs::create_dir_all(&socks).expect("mkdir");
        let sock = socks.join("4242.sock");
        let (sessions, _) = setup(tmp.path(), 4242, Some(sock.to_str().expect("utf8")));
        bind_socket(&sock);

        let out = remove_registration_in(&sessions, 4242, &FakeProbe::new(), &socks).expect("ok");

        assert!(!sock.exists(), "the session's own socket must be removed");
        assert_eq!(out.removed.len(), 3, "json + key + socket");
        assert!(out.skipped.is_empty(), "unexpected skips: {:?}", out.skipped);
    }

    #[test]
    fn refuses_when_the_pid_is_actually_alive() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (sessions, socks) = setup(tmp.path(), 4242, None);

        // startedAt in the fixture is 1789000000000 -> 1789000000s.
        let probe = FakeProbe::new().alive(4242, 1_789_000_000, "claude");
        let err = remove_registration_in(&sessions, 4242, &probe, &socks).expect_err("must refuse");

        assert!(err.to_lowercase().contains("alive"), "unhelpful error: {err}");
        assert!(sessions.join("4242.json").exists(), "nothing may be deleted on refusal");
        assert!(sessions.join("4242.abc123def456.key").exists());
    }

    #[test]
    fn refuses_a_non_darwin_pid_domain() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sessions = tmp.path().join("sessions");
        let socks = tmp.path().join("socks");
        std::fs::create_dir_all(&sessions).expect("mkdir");
        std::fs::create_dir_all(&socks).expect("mkdir");
        std::fs::write(
            sessions.join("4242.json"),
            r#"{"pid":4242,"sessionId":"s","startedAt":1789000000000,"pidDomain":"linux"}"#,
        )
        .expect("write");

        let err = remove_registration_in(&sessions, 4242, &FakeProbe::new(), &socks)
            .expect_err("must refuse a foreign pid namespace");
        assert!(err.to_lowercase().contains("domain"), "unhelpful error: {err}");
        assert!(sessions.join("4242.json").exists());
    }

    #[test]
    fn refuses_a_socket_path_outside_the_socket_dir_and_reports_it() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let decoy = tmp.path().join("important.txt");
        std::fs::write(&decoy, b"do not delete me").expect("write");
        let (sessions, socks) = setup(tmp.path(), 4242, Some(decoy.to_str().expect("utf8")));

        let out = remove_registration_in(&sessions, 4242, &FakeProbe::new(), &socks).expect("ok");

        assert!(decoy.exists(), "a path outside the socket dir must be refused");
        assert_eq!(out.skipped.len(), 1);
        assert!(out.skipped[0].contains("important.txt"));
        assert!(!sessions.join("4242.json").exists(), "the registration still goes");
    }

    #[test]
    fn refuses_a_traversal_attempt_in_the_declared_socket_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let victim = tmp.path().join("victim.txt");
        std::fs::write(&victim, b"keep me").expect("write");
        let socks = tmp.path().join("socks");
        std::fs::create_dir_all(&socks).expect("mkdir");
        // A path that lexically sits under socks but climbs back out.
        let evil = format!("{}/../victim.txt", socks.display());
        let (sessions, _) = setup(tmp.path(), 4242, Some(&evil));

        let out = remove_registration_in(&sessions, 4242, &FakeProbe::new(), &socks).expect("ok");

        assert!(victim.exists(), "traversal must not escape the socket dir");
        assert_eq!(out.skipped.len(), 1);
    }

    #[test]
    fn refuses_a_socket_whose_name_is_not_the_pid() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let socks = tmp.path().join("socks");
        std::fs::create_dir_all(&socks).expect("mkdir");
        let other = socks.join("9999.sock");
        bind_socket(&other);
        let (sessions, _) = setup(tmp.path(), 4242, Some(other.to_str().expect("utf8")));

        let out = remove_registration_in(&sessions, 4242, &FakeProbe::new(), &socks).expect("ok");

        assert!(other.exists(), "only <pid>.sock may be removed");
        assert_eq!(out.skipped.len(), 1);
    }

    #[test]
    fn does_not_touch_another_sessions_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (sessions, socks) = setup(tmp.path(), 4242, None);
        setup(tmp.path(), 5555, None);

        remove_registration_in(&sessions, 4242, &FakeProbe::new(), &socks).expect("ok");

        assert!(sessions.join("5555.json").exists(), "a neighbour must survive");
        assert!(sessions.join("5555.abc123def456.key").exists());
    }

    #[test]
    fn a_pid_prefix_must_not_match_a_longer_pid() {
        // 424 must not sweep up 4242's files.
        let tmp = tempfile::tempdir().expect("tempdir");
        let (sessions, socks) = setup(tmp.path(), 4242, None);
        setup(tmp.path(), 424, None);

        remove_registration_in(&sessions, 424, &FakeProbe::new(), &socks).expect("ok");

        assert!(!sessions.join("424.json").exists());
        assert!(sessions.join("4242.json").exists(), "4242 is a different session");
        assert!(sessions.join("4242.abc123def456.key").exists());
    }

    #[test]
    fn refuses_an_unknown_pid_rather_than_deleting_nothing_silently() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sessions = tmp.path().join("sessions");
        let socks = tmp.path().join("socks");
        std::fs::create_dir_all(&sessions).expect("mkdir");
        std::fs::create_dir_all(&socks).expect("mkdir");
        let err = remove_registration_in(&sessions, 9999, &FakeProbe::new(), &socks)
            .expect_err("must refuse");
        assert!(err.to_lowercase().contains("no registration"), "unhelpful error: {err}");
    }

    #[test]
    fn never_removes_a_directory_even_if_one_is_named_like_a_key() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (sessions, socks) = setup(tmp.path(), 4242, None);
        let trap = sessions.join("4242.trap.key");
        std::fs::create_dir_all(&trap).expect("mkdir trap");
        std::fs::write(trap.join("inside"), b"x").expect("write");

        let out = remove_registration_in(&sessions, 4242, &FakeProbe::new(), &socks).expect("ok");

        assert!(trap.exists(), "a directory must never be removed");
        assert!(trap.join("inside").exists());
        assert!(out.skipped.iter().any(|s| s.contains("4242.trap.key")));
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd src-tauri && cargo test sessions::registration 2>&1 | tail -20
```

Expected: compile error, `cannot find function remove_registration`.

- [ ] **Step 3: Write the minimal implementation**

Prepend to `src-tauri/src/sessions/registration.rs`:

```rust
use crate::sessions::probe::ProcessProbe;
use crate::sessions::reconcile::START_TIME_TOLERANCE_SECS;
use crate::sessions::registry::read_registry;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

/// Sockets may only be removed from this directory in production. Tests pass
/// their own via `remove_registration_in` so they never touch the real one,
/// where they could collide with a live session or a parallel test run.
const SOCKET_DIR: &str = "/tmp/cc-socks";

#[derive(Debug, Default)]
pub struct RemovalOutcome {
    pub removed: Vec<PathBuf>,
    /// Paths we declined to touch, with the reason, so the UI can say why.
    pub skipped: Vec<String>,
}

/// Remove the registration files of a session whose process is gone.
pub fn remove_registration(
    sessions_dir: &Path,
    pid: u32,
    probe: &dyn ProcessProbe,
) -> Result<RemovalOutcome, String> {
    remove_registration_in(sessions_dir, pid, probe, Path::new(SOCKET_DIR))
}

/// Testable form: the socket directory is a parameter.
///
/// This is the only write under `~/.claude` in Phase 2, so it re-verifies
/// liveness at call time rather than trusting the row the user clicked: a
/// session that came back between render and click must not have its files
/// deleted out from under it.
pub fn remove_registration_in(
    sessions_dir: &Path,
    pid: u32,
    probe: &dyn ProcessProbe,
    socket_dir: &Path,
) -> Result<RemovalOutcome, String> {
    let entry = read_registry(sessions_dir)
        .into_iter()
        .find(|s| s.pid == pid)
        .ok_or_else(|| format!("no registration found for pid {pid}"))?;

    // A pid from another machine's namespace would look dead locally. Refuse
    // rather than delete a registration we cannot reason about.
    if entry
        .pid_domain
        .as_deref()
        .is_some_and(|d| d != "darwin")
    {
        return Err(format!(
            "pid {pid} belongs to another pid domain ({}); refusing",
            entry.pid_domain.as_deref().unwrap_or("unknown")
        ));
    }

    // Re-verify: alive AND the same session (start times agree).
    if let Some(info) = probe.probe(&[pid]).get(&pid) {
        let same = (info.start_time_secs - entry.started_at_ms / 1000).abs()
            <= START_TIME_TOLERANCE_SECS;
        if same {
            return Err(format!(
                "pid {pid} is alive and is still this session; refusing to remove its files"
            ));
        }
    }

    let mut out = RemovalOutcome::default();

    // 1. The registration JSON. Regular files only.
    remove_regular_file(&sessions_dir.join(format!("{pid}.json")), &mut out);

    // 2. Key siblings: exactly `<pid>.<something>.key`. Comparing the whole
    //    first dot-component means 424 cannot match 4242.
    if let Ok(entries) = std::fs::read_dir(sessions_dir) {
        for e in entries.flatten() {
            let path = e.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.ends_with(".key") {
                continue;
            }
            if name.split('.').next().unwrap_or("") != pid.to_string() {
                continue;
            }
            remove_regular_file(&path, &mut out);
        }
    }

    // 3. The socket this session declared. Three independent conditions, all
    //    required: it sits DIRECTLY in socket_dir (a lexical parent check,
    //    which also defeats `socket_dir/../x` since that parent is
    //    `socket_dir/..`), it is named exactly `<pid>.sock`, and it really is
    //    a socket or a regular file.
    if let Some(declared) = entry.messaging_socket_path.as_deref() {
        let p = Path::new(declared);
        let in_dir = p.parent() == Some(socket_dir);
        let right_name = p.file_name().and_then(|n| n.to_str()) == Some(&format!("{pid}.sock"));
        if in_dir && right_name {
            remove_socket_or_file(p, &mut out);
        } else {
            let why = if in_dir {
                "not named <pid>.sock"
            } else {
                "outside the socket directory"
            };
            out.skipped.push(format!("{declared} ({why}; refused)"));
        }
    }

    Ok(out)
}

/// Remove a path only if it is a regular file. Anything else — a directory, a
/// symlink, a socket — is reported and left alone.
fn remove_regular_file(path: &Path, out: &mut RemovalOutcome) {
    match std::fs::symlink_metadata(path) {
        Ok(md) if md.is_file() => do_remove(path, out),
        Ok(_) => out
            .skipped
            .push(format!("{} (not a regular file; refused)", path.display())),
        Err(_) => {} // already gone; nothing to report
    }
}

/// Remove a path that may legitimately be a Unix domain socket.
///
/// A socket is NOT a regular file: real entries under the socket directory are
/// `srw-------`, so `is_file()` is false for every one of them. Using the
/// regular-file guard here would refuse every real socket while still passing
/// any test whose fixture was created with `fs::write`. Symlinks are still
/// refused, because `symlink_metadata` does not follow them.
fn remove_socket_or_file(path: &Path, out: &mut RemovalOutcome) {
    match std::fs::symlink_metadata(path) {
        Ok(md) if md.file_type().is_socket() || md.is_file() => do_remove(path, out),
        Ok(_) => out
            .skipped
            .push(format!("{} (not a socket or regular file; refused)", path.display())),
        Err(_) => {}
    }
}

fn do_remove(path: &Path, out: &mut RemovalOutcome) {
    match std::fs::remove_file(path) {
        Ok(()) => out.removed.push(path.to_path_buf()),
        Err(e) => out.skipped.push(format!("{} ({e})", path.display())),
    }
}
```

(The `mod` line was added in Step 1.)

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd src-tauri && cargo test sessions::registration 2>&1 | tail -16
```

Expected: `test result: ok. 11 passed`. Every test uses its own tempdir for both the sessions and the socket directory, so nothing touches the real `/tmp/cc-socks`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/sessions/registration.rs src-tauri/src/sessions/mod.rs
git commit -m "feat(sessions): allowlisted removal of dead session registrations"
```

---

### Task 7: Settings persistence, Tauri commands, tray session count

**Files:**
- Create: `src-tauri/src/settings.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: inline in `src-tauri/src/settings.rs`, plus one in `lib.rs`

**Interfaces:**
- Consumes: everything from Tasks 1-6.
- Produces: `AppSettings { tray_show_sessions }`, `settings::load(&Path)`, `settings::save(&Path, &AppSettings)`, `settings::SettingsPath(PathBuf)`; commands `list_sessions`, `remove_stale_registration`, `get_app_settings`, `set_tray_show_sessions`; `live_session_count(&AppHandle) -> usize`.

**Why a newtype for the path:** Tauri's managed state is keyed by `TypeId`, and `manage` silently keeps the EXISTING value if that type is already managed. Nothing manages a bare `PathBuf` today, but any future plugin that did would silently redirect our settings file with no error. `SettingsPath` costs three lines and removes the whole class of problem.

- [ ] **Step 1: Write the failing settings test**

Create `src-tauri/src/settings.rs` with ONLY this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_showing_the_session_count() {
        assert!(AppSettings::default().tray_show_sessions);
    }

    #[test]
    fn missing_file_yields_defaults_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let s = load(&dir.path().join("absent.json"));
        assert!(s.tray_show_sessions);
    }

    #[test]
    fn corrupt_file_yields_defaults_rather_than_failing_startup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("settings.json");
        std::fs::write(&p, b"{ not json").expect("write");
        assert!(load(&p).tray_show_sessions);
    }

    #[test]
    fn saves_and_reloads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("nested/settings.json");
        save(&p, &AppSettings { tray_show_sessions: false }).expect("save");
        assert!(!load(&p).tray_show_sessions);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cd src-tauri && cargo test settings:: 2>&1 | tail -15
```

Expected: compile error, `cannot find type AppSettings`.

- [ ] **Step 3: Implement settings**

Prepend to `src-tauri/src/settings.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    /// Append the live session count to the menu bar title.
    #[serde(default = "default_true")]
    pub tray_show_sessions: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            tray_show_sessions: true,
        }
    }
}

/// Never fails: a missing or unreadable settings file means defaults, because
/// a broken preference must not stop the app from starting.
pub fn load(path: &Path) -> AppSettings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Wrapper so Tauri's `TypeId`-keyed state cannot collide with anything else
/// that manages a bare `PathBuf`.
pub struct SettingsPath(pub std::path::PathBuf);

pub fn save(path: &Path, settings: &AppSettings) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {parent:?}: {e}"))?;
    }
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| format!("write {path:?}: {e}"))
}
```

Add `mod settings;` to `src-tauri/src/lib.rs`.

- [ ] **Step 4: Run the settings tests**

```bash
cd src-tauri && cargo test settings:: 2>&1 | tail -10
```

Expected: `test result: ok. 4 passed`.

- [ ] **Step 5: Write the failing tray test**

Add to the existing `mod tests` in `src-tauri/src/lib.rs`:

```rust
    #[test]
    fn tray_title_appends_session_count_only_when_enabled_and_nonzero() {
        assert_eq!(tray_title(2_400_000, 5, true), "2.4M · 5");
        assert_eq!(tray_title(2_400_000, 0, true), "2.4M", "zero sessions adds nothing");
        assert_eq!(tray_title(2_400_000, 5, false), "2.4M", "toggle off suppresses it");
        assert_eq!(tray_title(0, 3, true), "0 · 3");
    }
```

- [ ] **Step 6: Run it to verify it fails**

```bash
cd src-tauri && cargo test tray_title 2>&1 | tail -10
```

Expected: compile error, `cannot find function tray_title`.

- [ ] **Step 7: Implement the commands and tray wiring**

In `src-tauri/src/lib.rs`, add this pure helper next to `format_tokens`:

```rust
/// The menu bar title. Kept pure so it is testable without a tray.
pub fn tray_title(month_tokens: u64, live_sessions: usize, show_sessions: bool) -> String {
    let base = format_tokens(month_tokens);
    if show_sessions && live_sessions > 0 {
        format!("{base} · {live_sessions}")
    } else {
        base
    }
}
```

Add a shared probe and settings path to managed state. In the `setup()` closure, after the worker is managed:

```rust
            let settings_path = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("app data dir: {e}"))?
                .join("settings.json");
            app.manage(settings::SettingsPath(settings_path.clone()));
            app.manage(std::sync::Arc::new(sessions::probe::SysinfoProbe::new())
                as std::sync::Arc<dyn sessions::probe::ProcessProbe>);
```

Add the commands:

```rust
#[tauri::command]
async fn list_sessions(
    app: AppHandle,
    worker: tauri::State<'_, std::sync::Arc<usage::worker::UsageWorker>>,
) -> Result<Vec<sessions::rows::SessionRow>, String> {
    let probe = app
        .try_state::<std::sync::Arc<dyn sessions::probe::ProcessProbe>>()
        .ok_or("process probe unavailable")?
        .inner()
        .clone();
    let worker = worker.inner().clone();

    // Off the async runtime: this reads the filesystem, probes processes, and
    // takes the cache mutex — which the first full ingest holds for seconds.
    // The UI polls every 2s, so blocking a runtime thread here would stack up.
    tauri::async_runtime::spawn_blocking(move || {
        let roots = config::resolve(None);
        let files = sessions::registry::read_registry(&roots.sessions);
        let reconciled = sessions::reconcile::reconcile(&files, probe.as_ref());
        let usage = worker.session_usage();
        sessions::rows::build_rows(reconciled, &usage, usage::dates::now_ms())
    })
    .await
    .map_err(|e| format!("list_sessions failed: {e}"))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RemovalReport {
    removed: Vec<String>,
    skipped: Vec<String>,
}

#[tauri::command]
async fn remove_stale_registration(app: AppHandle, pid: u32) -> Result<RemovalReport, String> {
    let probe = app
        .try_state::<std::sync::Arc<dyn sessions::probe::ProcessProbe>>()
        .ok_or("process probe unavailable")?
        .inner()
        .clone();

    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let roots = config::resolve(None);
        sessions::registration::remove_registration(&roots.sessions, pid, probe.as_ref())
    })
    .await
    .map_err(|e| format!("remove_stale_registration failed: {e}"))??;

    Ok(RemovalReport {
        removed: outcome
            .removed
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        skipped: outcome.skipped,
    })
}

#[tauri::command]
async fn get_app_settings(
    path: tauri::State<'_, settings::SettingsPath>,
) -> Result<settings::AppSettings, String> {
    Ok(settings::load(&path.inner().0))
}

#[tauri::command]
async fn set_tray_show_sessions(
    app: AppHandle,
    path: tauri::State<'_, settings::SettingsPath>,
    enabled: bool,
) -> Result<(), String> {
    settings::save(
        &path.inner().0,
        &settings::AppSettings {
            tray_show_sessions: enabled,
        },
    )?;
    update_tray_from_worker(&app);
    Ok(())
}
```

Extend the `invoke_handler` list with `list_sessions, remove_stale_registration, get_app_settings, set_tray_show_sessions`.

Finally, make the tray use the new title. Replace the body of `update_tray_from_worker` so it reads the live session count and the toggle:

```rust
pub fn live_session_count(app: &AppHandle) -> usize {
    let Some(probe) = app.try_state::<std::sync::Arc<dyn sessions::probe::ProcessProbe>>() else {
        return 0;
    };
    let roots = config::resolve(None);
    let files = sessions::registry::read_registry(&roots.sessions);
    sessions::reconcile::reconcile(&files, probe.inner().as_ref())
        .iter()
        .filter(|r| r.state == sessions::reconcile::SessionState::Live)
        .count()
}
```

and inside `update_tray_from_worker`, replace `let title = format_tokens(month_tokens);` with:

```rust
    let show = app
        .try_state::<settings::SettingsPath>()
        .map(|p| settings::load(&p.inner().0).tray_show_sessions)
        .unwrap_or(true);
    let title = tray_title(month_tokens, live_session_count(app), show);
```

- [ ] **Step 8: Make the tray count able to go down**

As written in Phase 1, `polling.rs` only refreshes the tray when a TRANSCRIPT
changed (`files_read > 0 || files_retired > 0`), and `should_react` accepts only
`.jsonl` under `projects` — so the `sessions/` watch it registers has its events
discarded. The consequence: when you quit your last session and nothing writes a
transcript afterwards, the title keeps showing `2.4M · 5` indefinitely, and the
60 s fallback does not help because it is gated the same way.

In `src-tauri/src/polling.rs`, widen the filter:

```rust
/// True when a batch of changed paths contains a transcript OR a session
/// registration. Session events matter even when no transcript changed: the
/// live session count in the tray has to be able to go down.
pub fn should_react(paths: &[PathBuf]) -> bool {
    paths.iter().any(|p| {
        match p.extension().and_then(|e| e.to_str()) {
            Some("jsonl") => p.components().any(|c| c.as_os_str() == "projects"),
            Some("json") => p.components().any(|c| c.as_os_str() == "sessions"),
            _ => false,
        }
    })
}
```

and make `refresh` always repaint the tray, since the session count can change
with no transcript activity at all:

```rust
    let report = worker.refresh_now();
    if let Err(e) = worker.maybe_persist() {
        eprintln!("[polling] could not persist cache: {}", e);
    }
    let _ = report; // persistence is gated; the tray is not
    crate::update_tray_from_worker(app);
```

Add these tests beside the existing `should_react` ones:

```rust
    #[test]
    fn reacts_to_session_registration_changes() {
        // A session exiting removes its <pid>.json. Without this the tray's
        // live count can only ever go up.
        let paths = vec![PathBuf::from("/Users/me/.claude/sessions/12158.json")];
        assert!(should_react(&paths));
    }

    #[test]
    fn still_ignores_json_outside_the_sessions_directory() {
        let paths = vec![
            PathBuf::from("/Users/me/.claude/mcp-needs-auth-cache.json"),
            PathBuf::from("/Users/me/.claude/file-history/x.json"),
        ];
        assert!(!should_react(&paths));
    }
```

- [ ] **Step 9: Run the suite and build**

```bash
cd src-tauri && cargo test 2>&1 | grep -E "^test result: ok" | head -1
cd .. && npx vite build 2>&1 | grep "built in"
```

Expected: all Rust tests pass; frontend still builds.

- [ ] **Step 10: Commit**

```bash
git add src-tauri/src/settings.rs src-tauri/src/lib.rs src-tauri/src/polling.rs
git commit -m "feat(sessions): session commands, tray session count, session-aware watcher

The watcher previously discarded sessions/ events and only repainted the
tray when a transcript changed, so the live session count could never go
down once every session had exited."
```

---

### Task 8: Frontend types and API

**Files:**
- Modify: `src/lib/types.ts`, `src/lib/api.ts`
- Test: `src/lib/__tests__/api.test.ts`

**Interfaces:**
- Consumes: the four commands from Task 7.
- Produces: TS `AgentRow`, `SessionRow`, `RemovalReport`, `AppSettings`; `listSessions()`, `removeStaleRegistration(pid)`, `getAppSettings()`, `setTrayShowSessions(enabled)`.

- [ ] **Step 1: Write the failing test**

Add a new `describe` block at the end of `src/lib/__tests__/api.test.ts` (the file already mocks `@tauri-apps/api/core` from Phase 1):

```ts
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
```

Extend the file's `../api` import to include `listSessions`, `removeStaleRegistration`, `getAppSettings`, `setTrayShowSessions`.

- [ ] **Step 2: Run it to verify it fails**

```bash
npm test 2>&1 | tail -15
```

Expected: FAIL — `listSessions is not a function`.

- [ ] **Step 3: Add the types**

Append to `src/lib/types.ts`:

```ts
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
```

- [ ] **Step 4: Add the API functions**

Append to `src/lib/api.ts`:

```ts
export async function listSessions(): Promise<SessionRow[]> {
  return invoke<SessionRow[]>("list_sessions");
}

export async function removeStaleRegistration(pid: number): Promise<RemovalReport> {
  return invoke<RemovalReport>("remove_stale_registration", { pid });
}

export async function getAppSettings(): Promise<AppSettings> {
  return invoke<AppSettings>("get_app_settings");
}

export async function setTrayShowSessions(enabled: boolean): Promise<void> {
  return invoke("set_tray_show_sessions", { enabled });
}
```

and extend its type import:

```ts
import type {
  StatsCache,
  Diagnostics,
  SessionRow,
  RemovalReport,
  AppSettings,
} from "./types";
```

- [ ] **Step 5: Run the tests**

```bash
npm test 2>&1 | tail -8
```

Expected: 44 tests pass (41 prior + 3 new).

- [ ] **Step 6: Commit**

```bash
git add src/lib/types.ts src/lib/api.ts src/lib/__tests__/api.test.ts
git commit -m "feat(ui): session types and API bindings"
```

---

### Task 9: Sessions tab UI

**Files:**
- Create: `src/components/SessionsList.svelte`, `src/components/SessionRow.svelte`
- Modify: `src/App.svelte`, `src/components/Dashboard.svelte`, `src/components/Settings.svelte`
- Create: `src/lib/duration.ts`
- Test: `src/lib/__tests__/duration.test.ts`

**Interfaces:**
- Consumes: `listSessions`, `removeStaleRegistration`, `getAppSettings`, `setTrayShowSessions`, `SessionRow`, `AgentRow` (Task 8).
- Produces: `formatDuration(secs)`, `formatIdle(secs | null)`.

The formatting helpers are extracted so they can be unit-tested without mounting a component — this project has no component-test harness, and adding one is out of scope.

- [ ] **Step 1: Write the failing test**

Create `src/lib/__tests__/duration.test.ts`:

```ts
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
```

- [ ] **Step 2: Run it to verify it fails**

```bash
npm test 2>&1 | tail -12
```

Expected: FAIL — cannot resolve `../duration`.

- [ ] **Step 3: Implement the helpers**

Create `src/lib/duration.ts`:

```ts
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
```

- [ ] **Step 4: Run the tests**

```bash
npm test 2>&1 | tail -8
```

Expected: 51 tests pass (44 prior + 7 new).

- [ ] **Step 5: Build the row component**

Create `src/components/SessionRow.svelte`:

```svelte
<script lang="ts">
  import type { SessionRow } from "../lib/types";
  import { formatDuration, formatIdle } from "../lib/duration";
  import { formatTokens } from "../lib/format";

  interface Props {
    row: SessionRow;
    onRemove: (pid: number) => void;
    busy: boolean;
  }
  let { row, onRemove, busy }: Props = $props();

  let expanded = $state(false);

  // Two granularities on purpose: the dot follows the spec's 5-minute
  // "active" window, while the text gives the precise age (and says
  // "active now" only under a minute). A green dot beside "idle 2m" means
  // recently active, not a contradiction.
  const dotClass = $derived(
    row.state === "stale"
      ? "bg-red-500"
      : row.isActive
        ? "bg-green-500"
        : "bg-amber-500",
  );
</script>

<div class="px-4 py-2.5 border-b border-gray-100 dark:border-gray-800">
  <div class="flex items-start gap-2">
    <span class="mt-1.5 h-2 w-2 shrink-0 rounded-full {dotClass}"></span>

    <div class="min-w-0 flex-1">
      <div class="flex items-baseline justify-between gap-2">
        <span class="truncate text-sm font-medium text-gray-900 dark:text-gray-100">{row.name}</span>
        <span class="shrink-0 text-xs font-mono text-gray-500 dark:text-gray-400">
          {formatTokens(row.tokens)}
        </span>
      </div>

      <div class="truncate text-xs text-gray-500 dark:text-gray-400">
        {row.project}{#if row.gitBranch} · {row.gitBranch}{/if}
      </div>

      <div class="text-xs text-gray-400 dark:text-gray-500">
        {#if row.state === "stale"}
          pid {row.pid} · no running process
        {:else}
          up {formatDuration(row.uptimeSecs ?? 0)} · {formatIdle(row.idleSecs)}
        {/if}
      </div>

      {#if row.agents.length > 0}
        <button
          class="mt-1 text-xs text-blue-600 dark:text-blue-400"
          onclick={() => (expanded = !expanded)}
        >
          {expanded ? "▾" : "▸"} {row.agents.length} agent{row.agents.length === 1 ? "" : "s"}
        </button>
        {#if expanded}
          <div class="mt-1 space-y-1 border-l border-gray-200 pl-2 dark:border-gray-700">
            {#each row.agents as agent (agent.agentId)}
              <div class="flex items-baseline justify-between gap-2 text-xs">
                <span class="truncate text-gray-600 dark:text-gray-300">
                  {agent.agentType ?? "agent"}
                </span>
                <span class="shrink-0 font-mono text-gray-400 dark:text-gray-500">
                  {formatTokens(agent.tokens)} · {formatIdle(agent.idleSecs)}
                </span>
              </div>
            {/each}
            <p class="pt-0.5 text-xs text-gray-400 dark:text-gray-500">
              Agents run inside this session and can't be stopped separately.
            </p>
          </div>
        {/if}
      {/if}
    </div>

    {#if row.removable}
      <button
        class="shrink-0 text-xs text-red-600 disabled:opacity-50 dark:text-red-400"
        disabled={busy}
        onclick={() => onRemove(row.pid)}
        title="Remove this dead session's leftover registration files"
      >
        Clear
      </button>
    {/if}
  </div>
</div>
```

- [ ] **Step 6: Build the list component**

Create `src/components/SessionsList.svelte`. The polling lifecycle is the part to get right: poll every 2 s while this tab is mounted AND the window is visible/focused, and always clear on unmount.

```svelte
<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { listSessions, removeStaleRegistration } from "../lib/api";
  import type { SessionRow as Row } from "../lib/types";
  import SessionRow from "./SessionRow.svelte";

  interface Props {
    onSettings: () => void;
  }
  let { onSettings }: Props = $props();

  const POLL_MS = 2000;

  let rows = $state<Row[]>([]);
  // Two error slots on purpose: a successful poll every 2s would otherwise
  // erase the "pid X is alive, refusing" message before it could be read.
  let pollError = $state<string | null>(null);
  let actionError = $state<string | null>(null);
  let loading = $state(true);
  let busyPid = $state<number | null>(null);
  let timer: ReturnType<typeof setInterval> | null = null;

  const live = $derived(rows.filter((r) => r.state === "live"));
  const stale = $derived(rows.filter((r) => r.state === "stale"));

  async function refresh() {
    try {
      rows = await listSessions();
      pollError = null;
    } catch (e) {
      pollError = String(e);
    } finally {
      loading = false;
    }
  }

  function start() {
    if (timer !== null) return;
    timer = setInterval(refresh, POLL_MS);
  }

  function stop() {
    if (timer !== null) {
      clearInterval(timer);
      timer = null;
    }
  }

  function onVisibility() {
    // The popover hides on focus loss; don't probe processes while unseen.
    if (document.visibilityState === "visible") {
      refresh();
      start();
    } else {
      stop();
    }
  }

  onMount(() => {
    refresh();
    start();
    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("focus", onVisibility);
    window.addEventListener("blur", stop);
  });

  onDestroy(() => {
    stop();
    document.removeEventListener("visibilitychange", onVisibility);
    window.removeEventListener("focus", onVisibility);
    window.removeEventListener("blur", stop);
  });

  async function remove(pid: number) {
    busyPid = pid;
    actionError = null;
    try {
      await removeStaleRegistration(pid);
      await refresh();
    } catch (e) {
      // Survives subsequent polls; cleared on the next attempt.
      actionError = String(e);
    } finally {
      busyPid = null;
    }
  }
</script>

<!--
  Sessions gets its own header rather than putting a gear in the tab bar:
  Dashboard already owns a header with its refresh and settings buttons, and a
  second gear in the tab strip would sit right beside Dashboard's own. This
  keeps Dashboard untouched.
-->
<div class="flex items-center justify-between border-b border-gray-200 px-4 py-3 dark:border-gray-700">
  <div>
    <h1 class="text-sm font-semibold text-gray-900 dark:text-white">Sessions</h1>
    <p class="text-[10px] text-gray-400 dark:text-gray-500">
      {live.length} live{#if stale.length > 0} · {stale.length} stale{/if}
    </p>
  </div>
  <button
    onclick={onSettings}
    class="rounded-md p-1.5 transition-colors hover:bg-gray-100 dark:hover:bg-gray-800"
    title="Settings"
    aria-label="Settings"
  >
    <svg class="h-4 w-4 text-gray-500 dark:text-gray-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
      <path stroke-linecap="round" stroke-linejoin="round" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
      <path stroke-linecap="round" stroke-linejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
    </svg>
  </button>
</div>

<div class="flex-1 overflow-y-auto">
  {#if actionError}
    <p class="px-4 py-2 text-xs text-red-600 dark:text-red-400">{actionError}</p>
  {/if}
  {#if pollError}
    <p class="px-4 py-2 text-xs text-amber-600 dark:text-amber-400">{pollError}</p>
  {/if}

  {#if loading}
    <p class="px-4 py-3 text-xs text-gray-500 dark:text-gray-400">Looking for sessions…</p>
  {:else if rows.length === 0}
    <p class="px-4 py-3 text-xs text-gray-500 dark:text-gray-400">
      No Claude Code sessions registered.
    </p>
  {:else}
    {#if live.length > 0}
      <h3 class="px-4 pt-3 pb-1 text-xs font-medium uppercase tracking-wide text-gray-500 dark:text-gray-400">
        Live ({live.length})
      </h3>
      {#each live as row (row.pid)}
        <SessionRow {row} onRemove={remove} busy={busyPid === row.pid} />
      {/each}
    {/if}

    {#if stale.length > 0}
      <h3 class="px-4 pt-3 pb-1 text-xs font-medium uppercase tracking-wide text-gray-500 dark:text-gray-400">
        Stale registrations ({stale.length})
      </h3>
      <p class="px-4 pb-1 text-xs text-gray-400 dark:text-gray-500">
        These sessions are gone but left files behind.
      </p>
      {#each stale as row (row.pid)}
        <SessionRow {row} onRemove={remove} busy={busyPid === row.pid} />
      {/each}
    {/if}
  {/if}
</div>
```

- [ ] **Step 7: Make the shell three-tab**

Replace `src/App.svelte` with:

```svelte
<script lang="ts">
  import Dashboard from "./components/Dashboard.svelte";
  import Settings from "./components/Settings.svelte";
  import SessionsList from "./components/SessionsList.svelte";

  type Tab = "usage" | "sessions";

  let tab = $state<Tab>("usage");
  let showSettings = $state(false);

  const tabClass = (active: boolean) =>
    active
      ? "text-gray-900 dark:text-gray-100 border-b-2 border-gray-900 dark:border-gray-100"
      : "text-gray-500 dark:text-gray-400 border-b-2 border-transparent";
</script>

<div
  class="w-[400px] h-[600px] bg-white dark:bg-gray-900 rounded-xl shadow-2xl overflow-hidden border border-gray-200 dark:border-gray-800 flex flex-col"
>
  {#if showSettings}
    <!-- Back returns to whichever tab opened Settings, not always Usage. -->
    <Settings onBack={() => (showSettings = false)} />
  {:else}
    <div class="flex items-center gap-4 border-b border-gray-200 px-4 dark:border-gray-800">
      <button class="py-2 text-sm {tabClass(tab === 'usage')}" onclick={() => (tab = "usage")}>
        Usage
      </button>
      <button class="py-2 text-sm {tabClass(tab === 'sessions')}" onclick={() => (tab = "sessions")}>
        Sessions
      </button>
    </div>

    {#if tab === "usage"}
      <Dashboard onSettings={() => (showSettings = true)} />
    {:else}
      <SessionsList onSettings={() => (showSettings = true)} />
    {/if}
  {/if}
</div>
```

**`Dashboard.svelte` is not modified at all.** It keeps its `onSettings` prop and its own header (title, refresh, gear), which is exactly why the tab strip has no gear of its own — two would sit side by side. `Settings` is a full-screen overlay rather than a third tab, so returning from it lands back on the tab that opened it.

- [ ] **Step 8: Add the tray toggle to Settings**

In `src/components/Settings.svelte`, extend the script:

```ts
  import { getAppSettings, setTrayShowSessions } from "../lib/api";
  import type { AppSettings } from "../lib/types";

  let appSettings = $state<AppSettings | null>(null);

  onMount(async () => {
    try {
      appSettings = await getAppSettings();
    } catch {
      appSettings = null;
    }
  });

  async function toggleTraySessions(enabled: boolean) {
    await setTrayShowSessions(enabled);
    appSettings = { trayShowSessions: enabled };
  }
```

and add this card, matching the file's existing pattern:

```svelte
    <!-- Menu bar -->
    <div>
      <h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wide mb-3">Menu bar</h3>
      <div class="bg-gray-50 dark:bg-gray-800 rounded-lg p-3 space-y-2">
        <label class="flex items-center justify-between">
          <span class="text-sm text-gray-600 dark:text-gray-400">Show live session count</span>
          <input
            type="checkbox"
            checked={appSettings?.trayShowSessions ?? true}
            onchange={(e) => toggleTraySessions(e.currentTarget.checked)}
          />
        </label>
        <p class="text-xs text-gray-500 dark:text-gray-500">
          Appends the number of running sessions after the token total.
        </p>
      </div>
    </div>
```

- [ ] **Step 9: Verify everything**

```bash
npm test 2>&1 | tail -6
npx vite build 2>&1 | grep "built in"
cd src-tauri && cargo test 2>&1 | grep -E "^test result: ok" | head -1
```

Expected: 51 frontend tests pass, build succeeds, Rust suite green.

- [ ] **Step 10: Commit**

```bash
git add src/App.svelte src/components/SessionsList.svelte src/components/SessionRow.svelte src/components/Settings.svelte src/lib/duration.ts src/lib/__tests__/duration.test.ts
git commit -m "feat(ui): read-only sessions tab with nested subagents"
```

---

### Task 10: Manual verification against real sessions

No code. This is the gate before Phase 2 is called done, and it covers what tests cannot: that we read the real Claude Code correctly and that the UI is legible.

- [ ] **Step 1: Capture ground truth**

```bash
ls ~/.claude/sessions/*.json | wc -l
for f in ~/.claude/sessions/*.json; do
  python3 -c "
import json;d=json.load(open('$f'))
print(f\"{d['pid']:<8} {d.get('name','?'):<32} {d['cwd']}\")"
done
ps ax -o pid=,comm= | grep -c "native-binary/claude"
```

Record the pid list. The Sessions tab must show exactly these.

- [ ] **Step 2: Run the app and compare**

```bash
npx tauri dev
```

Open the popover, switch to Sessions. Confirm:
- every live pid from Step 1 appears, and nothing extra;
- each row's project matches the `cwd` basename, and the git branch is right for that checkout;
- uptimes are plausible against Step 1's process list;
- the session you are driving right now shows **active now**; others show an idle age;
- a session with subagents expands and its agent tokens are non-zero;
- the tray title gained `· N` matching the live count.

- [ ] **Step 3: Verify the idle-age source**

In one live session, start something slow (`sleep 120` via a Bash tool call) and watch its row. It must keep reporting recent activity rather than climbing to "idle 2m" — idle age comes from the last record of ANY type, not just usage-bearing ones. If it goes idle during the sleep, `last_ts` is being taken from the wrong field.

- [ ] **Step 4: Manufacture a stale registration and clear it**

```bash
# A dead pid that is certainly not running, with a plausible startedAt.
python3 - <<'EOF'
import json, os, time
p = os.path.expanduser('~/.claude/sessions/999001.json')
json.dump({"pid": 999001, "sessionId": "fake-stale-session",
           "cwd": "/tmp/gone", "startedAt": int(time.time()*1000) - 3600_000,
           "pidDomain": "darwin", "name": "gone-session-99",
           "messagingSocketPath": "/tmp/cc-socks/999001.sock"}, open(p, 'w'))
print("wrote", p)
EOF
python3 - <<'EOF'
import os, socket
os.makedirs('/tmp/cc-socks', exist_ok=True)
path = '/tmp/cc-socks/999001.sock'
if os.path.exists(path):
    os.unlink(path)
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.bind(path)   # a REAL socket: srw-------, not a regular file
s.close()
import stat
print('is socket:', stat.S_ISSOCK(os.lstat(path).st_mode))
EOF
```

**Bind it, do not `touch` it.** A `touch`ed path is a regular file, and a
regular file would be removed by a guard that refuses real sockets — so
touching here would let a broken implementation pass this gate. The script
prints `is socket: True`; if it does not, stop and fix the fixture.

The Sessions tab must show it under **Stale registrations** within ~2 s, with a Clear button and no uptime. Click Clear, then confirm both files are gone:

```bash
ls ~/.claude/sessions/999001.json /tmp/cc-socks/999001.sock 2>&1
```

Expected: both "No such file or directory".

- [ ] **Step 5: Verify the tray count goes DOWN**

The session count must fall when a session exits, including when nothing writes
a transcript afterwards — which is the ordinary "closed everything" case.

Note the current tray title, then quit one Claude Code session and leave the
machine idle (do not type into any other session). Within about a minute the
title's `· N` must decrease by one. If it stays put, the watcher is discarding
`sessions/` events or the tray repaint is still gated on transcript activity.

- [ ] **Step 6: Verify the live-session guard**

Pick a REAL live pid from Step 1 and hand-edit nothing — instead confirm the guard by temporarily writing a registration whose pid is your own shell (`echo $$`) and whose `startedAt` is now; that row must appear as **Live** with no Clear button. Remove the file yourself afterwards:

```bash
rm -f ~/.claude/sessions/$$.json
```

- [ ] **Step 7: Verify the toggle**

Turn off "Show live session count" in Settings; the tray must drop the `· N` suffix immediately. Quit and relaunch; it must stay off. Turn it back on.

- [ ] **Step 8: Record the outcome**

```bash
git commit --allow-empty -m "test: verify Phase 2 sessions tab against real sessions

Live rows matched the registry and process list; idle age tracked
non-usage activity; a manufactured stale registration with a REAL bound
socket was listed and fully cleared; a live session offered no Clear
action; the tray count fell when a session exited; toggle persisted."
```

---

## Definition of done

- [ ] `cargo test` green, `npm test` green (51 tests), `npx vite build` succeeds.
- [ ] `cargo clippy --all-targets` introduces no new warnings. Record the count
      before starting and after finishing; they must match. (Phase 1 left a
      non-zero baseline, so "zero warnings" is not the bar — "no NEW warnings"
      is. `FakeProbe` is `#[cfg(test)]` and the unread `kind` /
      `start_time_secs` fields were dropped precisely to hold this line.)
- [ ] The Sessions tab lists exactly the live sessions the registry and process table agree on.
- [ ] A session with no transcript renders "no activity yet" rather than an error or a bogus age.
- [ ] Idle age reflects non-usage activity (a long Bash call does not read as idle).
- [ ] A live session offers no Clear action; a stale one does, and clearing removes only its own files — including a REAL Unix domain socket, which is not a regular file.
- [ ] The tray's session count decreases when a session exits with no transcript activity following it.
- [ ] A Clear failure message survives the 2-second poll instead of flashing.
- [ ] Removal refuses a pid that is alive and still that session.
- [ ] Subagents appear nested, labelled, and marked not individually stoppable.
- [ ] Tray shows `· N` when enabled and non-zero; the toggle persists across a restart.
- [ ] No termination path exists anywhere in the diff — that is Phase 3.
- [ ] Nothing under `~/.claude` is written except a verified-dead session's own registration files.

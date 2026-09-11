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

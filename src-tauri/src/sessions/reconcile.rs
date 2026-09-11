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

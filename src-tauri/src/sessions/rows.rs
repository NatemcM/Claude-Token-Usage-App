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

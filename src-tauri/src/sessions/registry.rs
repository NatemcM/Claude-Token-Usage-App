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
#[allow(dead_code)]
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

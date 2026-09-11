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

//! A minimal, blocking JSON-RPC-over-Unix-socket client for npackd, exactly
//! matching the newline-delimited-JSON protocol documented in
//! docs/using-npack.md: `{"id", "method", "params"}` requests, `{"id",
//! "result"|"error"}` responses. Deliberately has no dependency on the
//! `npack` crate's internals -- npackd's frontend-independence (Phase 2 of
//! the roadmap) means a GUI should only ever need to know this wire
//! protocol, not any Rust types from the daemon's implementation.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::{net::UnixStream, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use serde_json::Value;

/// An error reported by npackd itself: its full message plus the stable
/// `error_code` (absent from older daemons). Carried inside `anyhow::Error`
/// so callers can `downcast_ref` it for friendly wording.
#[derive(Debug)]
pub struct DaemonError {
    pub code: Option<String>,
    pub message: String,
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DaemonError {}

/// Where to find the `npack` program: next to this executable (how the
/// packages install the two), otherwise whatever `PATH` offers.
pub fn npack_program() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("npack")))
        .filter(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from("npack"))
}

fn unreachable(message: String) -> anyhow::Error {
    DaemonError {
        code: Some("daemon_unreachable".to_owned()),
        message,
    }
    .into()
}

/// If nothing is listening on `socket`, starts `program daemon --socket
/// <socket>` and waits up to `wait` for it to accept connections.
///
/// The daemon is put in its own process group so closing the window (or a
/// terminal it was started from) does not take it down, and it is only ever
/// started when nothing is listening: `npack daemon` replaces an existing
/// socket, so starting a second one would orphan the first.
pub fn ensure_daemon_at(socket: &Path, program: &Path, wait: Duration) -> anyhow::Result<bool> {
    if UnixStream::connect(socket).is_ok() {
        return Ok(false);
    }
    let mut child = Command::new(program)
        .arg("daemon")
        .arg("--socket")
        .arg(socket)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|error| {
            unreachable(format!(
                "couldn't start the npack background service ({}): {error}",
                program.display()
            ))
        })?;
    let started = Instant::now();
    while started.elapsed() < wait {
        if UnixStream::connect(socket).is_ok() {
            return Ok(true);
        }
        // If it has already exited it will never listen: say so now rather
        // than after the full wait.
        if let Ok(Some(status)) = child.try_wait() {
            return Err(unreachable(format!(
                "the npack background service stopped as soon as it started ({status})"
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(unreachable(format!(
        "the npack background service didn't start listening within {} seconds",
        wait.as_secs().max(1)
    )))
}

/// Where the system-wide service (`npack daemon --system`) listens.
pub const SYSTEM_SOCKET: &str = "/run/npackd.sock";

pub fn system_socket_path() -> PathBuf {
    PathBuf::from(SYSTEM_SOCKET)
}

pub fn is_system_socket(path: &Path) -> bool {
    path == Path::new(SYSTEM_SOCKET)
}

/// Whether something is accepting connections on `socket`.
pub fn listening_at(socket: &Path) -> bool {
    UnixStream::connect(socket).is_ok()
}

pub struct DaemonClient {
    socket_path: PathBuf,
    next_id: AtomicU64,
}

impl DaemonClient {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            next_id: AtomicU64::new(1),
        }
    }

    /// Makes sure npackd is answering on this client's socket, starting
    /// `npack daemon` for this person if nothing is listening. Returns whether
    /// it had to start one. Blocks for up to `wait`, so call it off the UI thread.
    pub fn ensure_daemon(&self, wait: Duration) -> anyhow::Result<bool> {
        // The system-wide service belongs to the administrator (systemd starts
        // it as root). Starting a copy of it as this person would just fail
        // or, worse, answer on the wrong socket, so it is never started here.
        if is_system_socket(&self.socket_path) {
            return if listening_at(&self.socket_path) {
                Ok(false)
            } else {
                Err(unreachable(
                    "the system-wide npack service isn't running on this computer".to_owned(),
                ))
            };
        }
        ensure_daemon_at(&self.socket_path, &npack_program(), wait)
    }

    pub fn default_socket_path() -> PathBuf {
        dirs::runtime_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("npackd.sock")
    }

    /// Opens a fresh connection and makes one request. npackd handles many
    /// concurrent connections (Phase 2), so a short-lived connection per
    /// call keeps this client trivially simple rather than managing a
    /// persistent connection's lifecycle across GUI redraws.
    pub fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = serde_json::json!({ "id": id, "method": method, "params": params });
        let mut line = serde_json::to_string(&request)?;
        line.push('\n');

        let mut stream = UnixStream::connect(&self.socket_path).map_err(|error| {
            anyhow::anyhow!(
                "connecting to npackd at {}: {error} (is `npack daemon` running?)",
                self.socket_path.display()
            )
        })?;
        stream.write_all(line.as_bytes())?;

        let mut reader = BufReader::new(stream);
        let mut response_line = String::new();
        reader.read_line(&mut response_line)?;
        if response_line.is_empty() {
            anyhow::bail!("npackd closed the connection without a response");
        }
        let response: Value = serde_json::from_str(&response_line)?;
        if let Some(error) = response.get("error").and_then(Value::as_str) {
            return Err(DaemonError {
                code: response
                    .get("error_code")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                message: error.to_owned(),
            }
            .into());
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixListener,
    };

    /// A minimal stand-in for npackd that echoes back a canned response per
    /// request, so `DaemonClient`'s request/response parsing can be tested
    /// without a real daemon (verified separately, by hand, against the
    /// genuine `npack daemon` over a real socket).
    fn fake_daemon(socket_path: PathBuf, responses: Vec<Value>) {
        let listener = UnixListener::bind(&socket_path).unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            for response in responses {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                let mut out = serde_json::to_string(&response).unwrap();
                out.push('\n');
                writer.write_all(out.as_bytes()).unwrap();
            }
        });
    }

    #[test]
    fn call_returns_the_result_field_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("npackd.sock");
        fake_daemon(
            socket_path.clone(),
            vec![serde_json::json!({"id": 1, "result": [{"name": "hello"}]})],
        );
        let client = DaemonClient::new(socket_path);
        let result = client.call("ListInstalled", serde_json::json!({})).unwrap();
        assert_eq!(result, serde_json::json!([{"name": "hello"}]));
    }

    #[test]
    fn call_turns_an_error_field_into_an_err() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("npackd.sock");
        fake_daemon(
            socket_path.clone(),
            vec![serde_json::json!({"id": 1, "error": "unknown method Bogus"})],
        );
        let client = DaemonClient::new(socket_path);
        let error = client.call("Bogus", serde_json::json!({})).unwrap_err();
        assert!(error.to_string().contains("unknown method Bogus"));
    }

    #[test]
    fn call_keeps_the_error_code_for_friendly_wording() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("npackd.sock");
        fake_daemon(
            socket_path.clone(),
            vec![serde_json::json!({
                "id": 1, "error": "no relays configured", "error_code": "no_relays"
            })],
        );
        let client = DaemonClient::new(socket_path);
        let error = client.call("Search", serde_json::json!({})).unwrap_err();
        let daemon_error = error.downcast_ref::<DaemonError>().unwrap();
        assert_eq!(daemon_error.code.as_deref(), Some("no_relays"));
    }

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn nothing_is_started_when_a_daemon_is_already_listening() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("npackd.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        // A program that would leave a marker if it were ever run.
        let marker = dir.path().join("ran");
        let program = script(dir.path(), "npack", &format!("touch {}", marker.display()));
        assert!(!ensure_daemon_at(&socket, &program, Duration::from_secs(1)).unwrap());
        assert!(
            !marker.exists(),
            "starting a second daemon would steal the socket"
        );
    }

    #[test]
    fn the_daemon_is_started_with_the_right_arguments_and_waited_for() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("run/npackd.sock");
        let args = dir.path().join("args");
        // The stand-in daemon records how it was started, then stays alive.
        let program = script(
            dir.path(),
            "npack",
            &format!("echo \"$@\" > {}\nsleep 5", args.display()),
        );
        // Something starts listening shortly after, as a real daemon would.
        let listen_at = socket.clone();
        let binder = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            std::fs::create_dir_all(listen_at.parent().unwrap()).unwrap();
            let listener = std::os::unix::net::UnixListener::bind(&listen_at).unwrap();
            std::thread::sleep(Duration::from_secs(2));
            drop(listener);
        });
        assert!(ensure_daemon_at(&socket, &program, Duration::from_secs(4)).unwrap());
        assert_eq!(
            std::fs::read_to_string(&args).unwrap().trim(),
            format!("daemon --socket {}", socket.display())
        );
        binder.join().unwrap();
    }

    #[test]
    fn a_daemon_that_exits_at_once_is_reported_without_waiting_out_the_clock() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "npack", "exit 3");
        let started = Instant::now();
        let error = ensure_daemon_at(
            &dir.path().join("s.sock"),
            &program,
            Duration::from_secs(10),
        )
        .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        let daemon_error = error.downcast_ref::<DaemonError>().unwrap();
        assert_eq!(daemon_error.code.as_deref(), Some("daemon_unreachable"));
        assert!(
            daemon_error
                .message
                .contains("stopped as soon as it started")
        );
    }

    #[test]
    fn only_the_system_socket_is_treated_as_the_system_service() {
        assert!(is_system_socket(Path::new("/run/npackd.sock")));
        assert!(!is_system_socket(Path::new("/run/user/1000/npackd.sock")));
        assert!(!is_system_socket(Path::new("/tmp/npackd.sock")));
    }

    #[test]
    fn listening_is_detected_only_while_something_accepts_connections() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s.sock");
        assert!(!listening_at(&socket));
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert!(listening_at(&socket));
        drop(listener);
        assert!(
            !listening_at(&socket),
            "a stale socket file is not a service"
        );
    }

    #[test]
    fn a_missing_program_gives_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let error = ensure_daemon_at(
            &dir.path().join("s.sock"),
            &dir.path().join("no-such-npack"),
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("couldn't start the npack background service")
        );
    }

    #[test]
    fn a_daemon_that_never_listens_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "npack", "sleep 5");
        let error = ensure_daemon_at(
            &dir.path().join("s.sock"),
            &program,
            Duration::from_millis(600),
        )
        .unwrap_err();
        assert!(error.to_string().contains("didn't start listening"));
    }

    #[test]
    fn call_reports_a_clear_error_when_nothing_is_listening() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("npackd.sock");
        let client = DaemonClient::new(socket_path);
        let error = client
            .call("ListInstalled", serde_json::json!({}))
            .unwrap_err();
        assert!(error.to_string().contains("is `npack daemon` running?"));
    }
}

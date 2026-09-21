//! A minimal, blocking JSON-RPC-over-Unix-socket client for npackd, exactly
//! matching the newline-delimited-JSON protocol documented in
//! docs/using-npack.md: `{"id", "method", "params"}` requests, `{"id",
//! "result"|"error"}` responses. Deliberately has no dependency on the
//! `npack` crate's internals -- npackd's frontend-independence (Phase 2 of
//! the roadmap) means a GUI should only ever need to know this wire
//! protocol, not any Rust types from the daemon's implementation.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::Value;

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
            anyhow::bail!("{error}");
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

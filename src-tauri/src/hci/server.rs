//! HCI TCP server: bind, accept, per-connection auth, and the shutdown
//! channel that lets callers stop the server cleanly.
//!
//! The server is a thin wrapper around `tokio::net::TcpListener` with a
//! shared `AuthState`. Per-connection handling (JSON-RPC parsing, method
//! dispatch, rate limiting) lives in `router.rs` (WP3).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::hci::auth::{fresh_token, AuthState};
use crate::hci::conn_info::{build_conn_info, remove_conn_info, write_conn_info, write_conn_info_to, ConnInfo};

#[derive(Debug, Clone)]
pub struct HciServerConfig {
    pub bind_addr: IpAddr,
    pub channel: String,
    /// How long a client may take to complete the auth handshake before
    /// the connection is dropped.
    pub auth_timeout: Duration,
    /// If set, the connection-info file is written into this directory
    /// instead of being derived from the exe's location. Used by tests.
    pub conn_info_dir_override: Option<std::path::PathBuf>,
}

impl HciServerConfig {
    /// Default config: loopback only, channel reflects the build profile.
    pub fn from_env() -> Self {
        let channel = if cfg!(debug_assertions) {
            "debug".to_string()
        } else {
            "release".to_string()
        };
        Self {
            bind_addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
            channel,
            auth_timeout: Duration::from_secs(5),
            conn_info_dir_override: None,
        }
    }
}

/// Handle to a running HCI server. Drop the handle or call `shutdown()`
/// to stop accepting new connections and close existing ones.
pub struct HciServer {
    pub config: HciServerConfig,
    pub conn_info: ConnInfo,
    pub auth: Arc<AuthState>,
    shutdown: Arc<Notify>,
    listener_task: JoinHandle<()>,
}

impl HciServer {
    /// Bind the listener, write the connection-info file, and start the
    /// accept loop. Returns the server handle plus the join handle of
    /// the rotation timer (so callers can decide whether to await it).
    pub async fn start(config: HciServerConfig) -> Result<Self, StartError> {
        let listener = TcpListener::bind(SocketAddr::new(config.bind_addr, 0))
            .await
            .map_err(StartError::Bind)?;
        let local_addr = listener.local_addr().map_err(StartError::Bind)?;

        // Build a fresh boot token and connection-info record.
        let boot_token = fresh_token();
        let conn_info = build_conn_info(local_addr.ip(), local_addr.port(), &config.channel);

        // Replace any stale file with our new one. Atomic write.
        if let Some(dir) = &config.conn_info_dir_override {
            write_conn_info_to(&conn_info, dir).map_err(StartError::ConnInfo)?;
        } else {
            write_conn_info(&conn_info).map_err(StartError::ConnInfo)?;
        }

        let auth = AuthState::new(boot_token);
        let shutdown = Arc::new(Notify::new());

        let listener_task = {
            let shutdown = shutdown.clone();
            let auth = auth.clone();
            let auth_timeout = config.auth_timeout;
            tokio::spawn(async move {
                run_accept_loop(listener, auth, auth_timeout, shutdown).await;
            })
        };

        Ok(Self {
            config,
            conn_info,
            auth,
            shutdown,
            listener_task,
        })
    }

    /// Trigger a graceful shutdown. Idempotent.
    pub fn shutdown(&self) {
        self.shutdown.notify_waiters();
    }

    /// Block until the listener task exits. Mostly useful in tests.
    pub async fn join(mut self) -> std::io::Result<()> {
        // Notify the accept loop; then await.
        self.shutdown.notify_waiters();
        // Take the task out so Drop doesn't double-cancel it.
        let task = std::mem::replace(&mut self.listener_task, tokio::spawn(async {}));
        task.await.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    }
}

impl Drop for HciServer {
    fn drop(&mut self) {
        // Best-effort cleanup. If we're being dropped as part of normal
        // shutdown, the file is removed by the caller via `shutdown()`
        // + the run() teardown; this catches the panic / early-drop case.
        self.shutdown.notify_waiters();
        let _ = remove_conn_info();
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("could not bind TCP listener: {0}")]
    Bind(std::io::Error),
    #[error("could not write connection-info file: {0}")]
    ConnInfo(crate::hci::conn_info::ConnInfoError),
}

async fn run_accept_loop(
    listener: TcpListener,
    auth: Arc<AuthState>,
    auth_timeout: Duration,
    shutdown: Arc<Notify>,
) {
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            accept = listener.accept() => {
                match accept {
                    Ok((stream, peer)) => {
                        let auth = auth.clone();
                        tokio::spawn(async move {
                            handle_connection(stream, peer, auth, auth_timeout).await;
                        });
                    }
                    Err(e) => {
                        // A transient accept error shouldn't kill the server.
                        // Log and continue. In production this would go
                        // through the Tauri logging shim.
                        eprintln!("[hci] accept error: {e}");
                    }
                }
            }
        }
    }
}

/// Per-connection handshake. The first line the client sends must be a
/// JSON-RPC `auth` method with the `boot_token`. On success we
/// acknowledge; on failure we close.
///
/// After auth, this function hands the stream to the router (WP3) — for
/// now it just closes, so the auth round-trip is independently testable.
async fn handle_connection(
    mut stream: TcpStream,
    _peer: SocketAddr,
    auth: Arc<AuthState>,
    auth_timeout: Duration,
) {
    let _ = stream.set_nodelay(true);
    let (read_half, mut write_half) = stream.split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    let read_result = tokio::time::timeout(
        auth_timeout,
        reader.read_line(&mut line),
    )
    .await;

    let line = match read_result {
        Ok(Ok(n)) if n > 0 => line,
        Ok(Ok(_)) => return, // EOF
        Ok(Err(e)) => {
            eprintln!("[hci] read error during auth: {e}");
            return;
        }
        Err(_) => {
            // Timeout: send a structured error and close.
            let err = r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32002,"message":"auth required"}}"#;
            let _ = write_half.write_all(err.as_bytes()).await;
            let _ = write_half.write_all(b"\n").await;
            return;
        }
    };

    // Parse just enough to find the boot_token. We don't go through the
    // full JSON-RPC machinery because the auth payload is the first
    // thing every client sends; we want it to be cheap.
    let presented_token = match parse_auth_token(&line) {
        Some(t) => t,
        None => {
            let err = r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"expected auth method"}}"#;
            let _ = write_half.write_all(err.as_bytes()).await;
            let _ = write_half.write_all(b"\n").await;
            return;
        }
    };

    let Some(session) = auth.consume_boot_token(&presented_token) else {
        let err = r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32002,"message":"auth failed"}}"#;
        let _ = write_half.write_all(err.as_bytes()).await;
        let _ = write_half.write_all(b"\n").await;
        return;
    };

    let ok = format!(
        r#"{{"jsonrpc":"2.0","id":null,"result":{{"session_token":"{session}","server_version":"0.1.0"}}}}"#
    );
    let _ = write_half.write_all(ok.as_bytes()).await;
    let _ = write_half.write_all(b"\n").await;

    // WP3 will replace this with a call into the router.
    // For now, close: the auth round-trip is what WP2 verifies.
    let _ = write_half.shutdown().await;
}

/// Extract a `boot_token` string from a JSON-RPC auth payload. Tolerant
/// of field ordering and a missing `jsonrpc` field. Returns `None` if
/// the line isn't an `auth` call.
fn parse_auth_token(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let method = v.get("method")?.as_str()?;
    if method != "auth" {
        return None;
    }
    let token = v
        .get("params")?
        .get("token")?
        .as_str()?;
    Some(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_auth_token_happy_path() {
        let line = r#"{"method":"auth","params":{"token":"abc"}}"#;
        assert_eq!(parse_auth_token(line).as_deref(), Some("abc"));
    }

    #[test]
    fn parse_auth_token_wrong_method() {
        let line = r#"{"method":"system.ping","params":{}}"#;
        assert!(parse_auth_token(line).is_none());
    }

    #[test]
    fn parse_auth_token_missing_params() {
        let line = r#"{"method":"auth"}"#;
        assert!(parse_auth_token(line).is_none());
    }

    #[test]
    fn parse_auth_token_garbage() {
        assert!(parse_auth_token("not json").is_none());
        assert!(parse_auth_token("").is_none());
    }

    async fn with_temp_conn_info<F, Fut>(f: F)
    where
        F: FnOnce(std::path::PathBuf) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let dir = tempfile::tempdir().expect("tempdir");
        f(dir.path().to_path_buf()).await;
        let _ = dir.close();
    }

    fn cfg_with_dir(dir: std::path::PathBuf) -> HciServerConfig {
        HciServerConfig {
            bind_addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
            channel: "test".into(),
            auth_timeout: Duration::from_millis(500),
            conn_info_dir_override: Some(dir),
        }
    }

    #[tokio::test]
    async fn server_binds_and_writes_conn_info() {
        with_temp_conn_info(|dir| async move {
            let cfg = cfg_with_dir(dir);
            let server = HciServer::start(cfg).await.expect("server starts");
            assert!(server.conn_info.port > 0);
            assert_eq!(server.conn_info.addr, "127.0.0.1");
            assert_eq!(server.conn_info.build.channel, "test");
            assert_eq!(server.conn_info.boot_token.len(), 43);
            server.shutdown();
            let _ = server.join().await;
        }).await;
    }

    #[tokio::test]
    async fn auth_round_trip_succeeds() {
        with_temp_conn_info(|dir| async move {
            let cfg = cfg_with_dir(dir);
            let server = HciServer::start(cfg).await.expect("server starts");
            let addr = SocketAddr::new(server.config.bind_addr, server.conn_info.port);
            let token = server.conn_info.boot_token.clone();

            let mut stream = TcpStream::connect(addr).await.expect("connect");
            let payload = format!(r#"{{"method":"auth","params":{{"token":"{token}"}}}}"#);
            stream.write_all(payload.as_bytes()).await.expect("write auth");
            stream.write_all(b"\n").await.expect("newline");

            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("read response");
            let v: serde_json::Value = serde_json::from_str(&line).expect("json response");
            assert!(v.get("result").is_some(), "expected result, got: {line}");
            let session = v["result"]["session_token"].as_str().expect("session_token string");
            assert_eq!(session.len(), 43);
        }).await;
    }

    #[tokio::test]
    async fn wrong_token_rejected() {
        with_temp_conn_info(|dir| async move {
            let cfg = cfg_with_dir(dir);
            let server = HciServer::start(cfg).await.expect("server starts");
            let addr = SocketAddr::new(server.config.bind_addr, server.conn_info.port);

            let mut stream = TcpStream::connect(addr).await.expect("connect");
            stream
                .write_all(br#"{"method":"auth","params":{"token":"nope"}}"#)
                .await
                .expect("write");
            stream.write_all(b"\n").await.expect("newline");

            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("read response");
            let v: serde_json::Value = serde_json::from_str(&line).expect("json");
            assert_eq!(v["error"]["code"], -32002);
        }).await;
    }
}

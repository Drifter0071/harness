//! HCI TCP server: bind, accept, per-connection auth, and the post-auth
//! frame loop that dispatches JSON-RPC requests to the router.
//!
//! The server is a thin wrapper around `tokio::net::TcpListener` with a
//! shared `AuthState`, a shared `Router` (one instance for the process),
//! and a shared `RpcContext` (cloned per connection).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::hci::auth::{fresh_token, AuthState};
use crate::hci::conn_info::{build_conn_info, remove_conn_info, write_conn_info, write_conn_info_to, ConnInfo};
use crate::hci::router::{ConnectionCtx, RpcContext, RpcResponse, Router};

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
    pub router: Arc<Router>,
    pub rpc_ctx: RpcContext,
    shutdown: Arc<Notify>,
    listener_task: JoinHandle<()>,
}

impl HciServer {
    /// Bind the listener, write the connection-info file, and start the
    /// accept loop. Returns the server handle plus the join handle of
    /// the rotation timer (so callers can decide whether to await it).
    pub async fn start(
        config: HciServerConfig,
        router: Arc<Router>,
        rpc_ctx: RpcContext,
    ) -> Result<Self, StartError> {
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
            let router = router.clone();
            let rpc_ctx = rpc_ctx.clone();
            let auth_timeout = config.auth_timeout;
            tokio::spawn(async move {
                run_accept_loop(listener, auth, router, rpc_ctx, auth_timeout, shutdown).await;
            })
        };

        Ok(Self {
            config,
            conn_info,
            auth,
            router,
            rpc_ctx,
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
    router: Arc<Router>,
    rpc_ctx: RpcContext,
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
                        let router = router.clone();
                        let rpc_ctx = rpc_ctx.clone();
                        tokio::spawn(async move {
                            handle_connection(stream, peer, auth, router, rpc_ctx, auth_timeout).await;
                        });
                    }
                    Err(e) => {
                        eprintln!("[hci] accept error: {e}");
                    }
                }
            }
        }
    }
}

/// Per-connection handshake + frame loop. The first line the client
/// sends must be a JSON-RPC `auth` method with the `boot_token`. On
/// success the server acknowledges with a session token and then
/// enters the frame loop, parsing newline-delimited JSON-RPC frames
/// and dispatching them to the shared router.
async fn handle_connection(
    mut stream: TcpStream,
    _peer: SocketAddr,
    auth: Arc<AuthState>,
    router: Arc<Router>,
    rpc_ctx: RpcContext,
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

    let mut line = match read_result {
        Ok(Ok(n)) if n > 0 => line,
        Ok(Ok(_)) => return, // EOF
        // (line is bound mutable below in the frame loop, but here we
        // only need an immutable borrow into the Ok arm)
        Ok(Err(e)) => {
            eprintln!("[hci] read error during auth: {e}");
            return;
        }
        Err(_) => {
            let err = r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32002,"message":"auth required"}}"#;
            let _ = write_half.write_all(err.as_bytes()).await;
            let _ = write_half.write_all(b"\n").await;
            return;
        }
    };

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

    // Frame loop. Read newline-delimited JSON-RPC frames until the
    // peer closes or we error out. Each frame is dispatched through
    // the shared router. Responses are written back one per line.
    let conn = ConnectionCtx { session_token: session };
    loop {
        line.clear();
        let read_result = reader.read_line(&mut line).await;
        let n = match read_result {
            Ok(n) => n,
            Err(e) => {
                eprintln!("[hci] read error after auth: {e}");
                break;
            }
        };
        if n == 0 {
            // Peer closed.
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: crate::hci::router::RpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                // -32700 parse error
                let resp = RpcResponse::err(Value::Null, -32700, &format!("parse: {e}"));
                if let Ok(s) = serde_json::to_string(&resp) {
                    let _ = write_half.write_all(s.as_bytes()).await;
                    let _ = write_half.write_all(b"\n").await;
                }
                continue;
            }
        };
        if let Some(resp) = router.handle_request(req, &rpc_ctx, &conn).await {
            if let Ok(s) = serde_json::to_string(&resp) {
                let _ = write_half.write_all(s.as_bytes()).await;
                let _ = write_half.write_all(b"\n").await;
            }
        }
        // Notifications produce None and we don't write anything — correct.
    }
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

    fn empty_app() -> Arc<crate::commands::AppState> {
        Arc::new(crate::commands::AppState::new(Arc::new(
            crate::secrets::InMemoryStore::new(),
        )))
    }

    fn empty_rpc_ctx() -> (Arc<crate::hci::router::Router>, crate::hci::router::RpcContext) {
        let router = Arc::new(crate::hci::router::Router::new());
        let ctx = crate::hci::router::RpcContext {
            writes_enabled: false,
            auth: None,
            started_at: chrono::Utc::now(),
            app: None,
            canvas_bus: None,
        };
        (router, ctx)
    }

    #[tokio::test]
    async fn server_binds_and_writes_conn_info() {
        with_temp_conn_info(|dir| async move {
            let cfg = cfg_with_dir(dir);
            let (router, ctx) = empty_rpc_ctx();
            let _app = empty_app();
            let server = HciServer::start(cfg, router, ctx).await.expect("server starts");
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
            let (router, ctx) = empty_rpc_ctx();
            let server = HciServer::start(cfg, router, ctx).await.expect("server starts");
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
            let (router, ctx) = empty_rpc_ctx();
            let server = HciServer::start(cfg, router, ctx).await.expect("server starts");
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

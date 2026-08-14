//! HCI TCP server: bind, accept, and the per-connection frame loop that
//! dispatches JSON-RPC requests to the router.
//!
//! The server is a thin wrapper around `tokio::net::TcpListener` with a
//! shared `Router` (one instance for the process) and a shared
//! `RpcContext` (cloned per connection). It binds to loopback only, so
//! external network peers cannot reach it without first tunneling
//! through the user's machine.
//!
//! Auth: the loopback bind is the only access control. There is no
//! token handshake on the wire — clients can connect and call methods
//! directly. The legacy `auth` method is still accepted (it returns a
//! session token) so older clients and the in-process Health Agent
//! don't break, but the token is not enforced.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::hci::conn_info::{
    build_conn_info, remove_conn_info, write_conn_info, write_conn_info_to, ConnInfo,
};
use crate::hci::router::{ConnectionCtx, Router, RpcContext, RpcResponse};

#[derive(Debug, Clone)]
pub struct HciServerConfig {
    pub bind_addr: IpAddr,
    pub channel: String,
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
            conn_info_dir_override: None,
        }
    }
}

/// Handle to a running HCI server. Drop the handle or call `shutdown()`
/// to stop accepting new connections and close existing ones.
pub struct HciServer {
    pub config: HciServerConfig,
    pub conn_info: ConnInfo,
    pub router: Arc<Router>,
    pub rpc_ctx: RpcContext,
    shutdown: Arc<Notify>,
    listener_task: JoinHandle<()>,
}

impl HciServer {
    /// Bind the listener, write the connection-info file, and start the
    /// accept loop.
    pub async fn start(
        config: HciServerConfig,
        router: Arc<Router>,
        rpc_ctx: RpcContext,
    ) -> Result<Self, StartError> {
        let listener = TcpListener::bind(SocketAddr::new(config.bind_addr, 0))
            .await
            .map_err(StartError::Bind)?;
        let local_addr = listener.local_addr().map_err(StartError::Bind)?;

        // Build a connection-info record. No boot token anymore — the
        // loopback bind is the only access control.
        let conn_info = build_conn_info(local_addr.ip(), local_addr.port(), &config.channel);

        // Replace any stale file with our new one. Atomic write.
        if let Some(dir) = &config.conn_info_dir_override {
            write_conn_info_to(&conn_info, dir).map_err(StartError::ConnInfo)?;
        } else {
            write_conn_info(&conn_info).map_err(StartError::ConnInfo)?;
        }

        let shutdown = Arc::new(Notify::new());

        let listener_task = {
            let shutdown = shutdown.clone();
            let router = router.clone();
            let rpc_ctx = rpc_ctx.clone();
            tokio::spawn(async move {
                run_accept_loop(listener, router, rpc_ctx, shutdown).await;
            })
        };

        Ok(Self {
            config,
            conn_info,
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
        task.await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
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
    router: Arc<Router>,
    rpc_ctx: RpcContext,
    shutdown: Arc<Notify>,
) {
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            accept = listener.accept() => {
                match accept {
                    Ok((stream, peer)) => {
                        let router = router.clone();
                        let rpc_ctx = rpc_ctx.clone();
                        tokio::spawn(async move {
                            handle_connection(stream, peer, router, rpc_ctx).await;
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

/// Per-connection frame loop. Reads newline-delimited JSON-RPC frames
/// from the client and dispatches them through the shared router.
/// There is no auth gate — the loopback bind is the only access
/// control. A stub session token is created for each connection so
/// the router can still surface per-connection metadata; it is not
/// validated.
async fn handle_connection(
    mut stream: TcpStream,
    _peer: SocketAddr,
    router: Arc<Router>,
    rpc_ctx: RpcContext,
) {
    let _ = stream.set_nodelay(true);
    let (read_half, mut write_half) = stream.split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    let conn = ConnectionCtx {
        session_token: "loopback".to_string(),
    };

    loop {
        line.clear();
        let read_result = reader.read_line(&mut line).await;
        let n = match read_result {
            Ok(n) => n,
            Err(e) => {
                eprintln!("[hci] read error: {e}");
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
        // Legacy `auth` method: answer with a stub session token so
        // older clients and the in-process Health Agent keep working,
        // but do NOT enforce the token on subsequent calls.
        if req.method == "auth" {
            let resp = RpcResponse::ok(
                req.id.clone().unwrap_or(Value::Null),
                serde_json::json!({
                    "session_token": "loopback",
                    "server_version": "0.1.0"
                }),
            );
            if let Ok(s) = serde_json::to_string(&resp) {
                let _ = write_half.write_all(s.as_bytes()).await;
                let _ = write_half.write_all(b"\n").await;
            }
            continue;
        }
        if let Some(resp) = router.handle_request(req, &rpc_ctx, &conn).await {
            if let Ok(s) = serde_json::to_string(&resp) {
                let _ = write_half.write_all(s.as_bytes()).await;
                let _ = write_half.write_all(b"\n").await;
            }
        }
        // Notifications produce None and we don't write anything — correct.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            conn_info_dir_override: Some(dir),
        }
    }

    fn empty_app() -> Arc<crate::commands::AppState> {
        Arc::new(crate::commands::AppState::new(Arc::new(
            crate::secrets::InMemoryStore::new(),
        )))
    }

    fn empty_rpc_ctx() -> (
        Arc<crate::hci::router::Router>,
        crate::hci::router::RpcContext,
    ) {
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
            let server = HciServer::start(cfg, router, ctx)
                .await
                .expect("server starts");
            assert!(server.conn_info.port > 0);
            assert_eq!(server.conn_info.addr, "127.0.0.1");
            assert_eq!(server.conn_info.build.channel, "test");
            // boot_token is still in the wire format for compat, but its
            // value is no longer security-relevant.
            assert_eq!(server.conn_info.boot_token.len(), 43);
            server.shutdown();
            let _ = server.join().await;
        })
        .await;
    }

    #[tokio::test]
    async fn no_auth_required_to_call_methods() {
        with_temp_conn_info(|dir| async move {
            let cfg = cfg_with_dir(dir);
            let (router, ctx) = empty_rpc_ctx();
            let server = HciServer::start(cfg, router, ctx)
                .await
                .expect("server starts");
            let addr = SocketAddr::new(server.config.bind_addr, server.conn_info.port);

            let mut stream = TcpStream::connect(addr).await.expect("connect");
            // Send a method directly without any auth handshake.
            let payload = r#"{"id":1,"method":"system.ping","params":{}}"#;
            stream.write_all(payload.as_bytes()).await.expect("write");
            stream.write_all(b"\n").await.expect("newline");

            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("read response");
            let v: serde_json::Value = serde_json::from_str(&line).expect("json");
            // The server has no `system.ping` registered in the empty
            // test context, so we expect either a result (if a future
            // test wants it) or a -32601 method-not-found. Crucially:
            // NOT an auth-required -32002.
            let code = v["error"]["code"].as_i64().unwrap_or(0);
            assert_ne!(code, -32002, "got auth-required: {line}");
        })
        .await;
    }

    #[tokio::test]
    async fn legacy_auth_method_returns_stub_token() {
        with_temp_conn_info(|dir| async move {
            let cfg = cfg_with_dir(dir);
            let (router, ctx) = empty_rpc_ctx();
            let server = HciServer::start(cfg, router, ctx)
                .await
                .expect("server starts");
            let addr = SocketAddr::new(server.config.bind_addr, server.conn_info.port);

            let mut stream = TcpStream::connect(addr).await.expect("connect");
            stream
                .write_all(br#"{"method":"auth","params":{"token":"anything"}}"#)
                .await
                .expect("write");
            stream.write_all(b"\n").await.expect("newline");

            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("read response");
            let v: serde_json::Value = serde_json::from_str(&line).expect("json");
            let session = v["result"]["session_token"]
                .as_str()
                .expect("session_token string");
            assert_eq!(session, "loopback");
        })
        .await;
    }
}

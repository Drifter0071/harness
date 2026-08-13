//! HCI ↔ Tauri integration. Owns the lifecycle of the HCI server
//! relative to the Tauri app: it starts the server inside the
//! `setup` callback, threads the `AppState` into the `RpcContext`,
//! and stops the server on app exit.

use std::sync::Arc;

use crate::commands::AppState;
use crate::hci::methods_canvas::CanvasEventBus;
use crate::hci::router::{Router, RpcContext};
use crate::hci::server::HciServer;
use crate::hci::{methods_canvas, methods_sessions_agents, methods_system, methods_workspace};

/// Build the HCI router with every read-tier method registered.
pub fn build_router() -> Router {
    Router::new()
        .register(methods_system::PingHandler)
        .register(methods_system::InfoHandler)
        .register(methods_workspace::ListHandler)
        .register(methods_workspace::ReadHandler)
        .register(methods_workspace::StatHandler)
        .register(methods_workspace::SearchHandler)
        .register(methods_sessions_agents::SessionListHandler)
        .register(methods_sessions_agents::SessionGetHandler)
        .register(methods_sessions_agents::AgentListHandler)
        .register(methods_sessions_agents::AgentGetHandler)
        .register(methods_canvas::SnapshotHandler)
        .register(methods_canvas::SubscribeHandler)
}

/// Decide whether HCI should be enabled.
/// - `HARNESS_CONTROL=0` forces off.
/// - `HARNESS_CONTROL=1` forces on (explicit).
/// - Otherwise on (the server binds to loopback only, so there is no
///   remote attack surface).
pub fn hci_enabled_by_default() -> bool {
    match std::env::var("HARNESS_CONTROL").ok().as_deref() {
        Some("0") => false,
        Some("1") => true,
        _ => true,
    }
}

/// Returns true if the write tier should be exposed. The write tier
/// remains opt-in for now so a remote takeover that escapes loopback
/// can't immediately mutate the workspace; once we have user-visible
/// write paths we can flip the default.
pub fn hci_write_enabled() -> bool {
    std::env::var("HARNESS_CONTROL_WRITE").ok().as_deref() == Some("1")
}

/// Construct the `RpcContext` the server will hand to handlers.
pub fn build_rpc_context(app: Arc<AppState>) -> RpcContext {
    RpcContext {
        writes_enabled: hci_write_enabled(),
        auth: None,
        started_at: chrono::Utc::now(),
        app: Some(app),
        canvas_bus: Some(CanvasEventBus::new()),
    }
}

/// Start the HCI server. Returns `None` if disabled. The `app` is
/// threaded into the `RpcContext` so handlers can read the same
/// `AppState` the Tauri IPC commands use.
pub async fn start(
    app: Arc<AppState>,
) -> Result<Option<HciServer>, crate::hci::server::StartError> {
    if !hci_enabled_by_default() {
        return Ok(None);
    }
    let cfg = crate::hci::server::HciServerConfig {
        bind_addr: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        channel: if cfg!(debug_assertions) {
            "debug".into()
        } else {
            "release".into()
        },
        conn_info_dir_override: std::env::var("HARNESS_CONN_INFO_DIR")
            .ok()
            .map(std::path::PathBuf::from),
    };
    let router = Arc::new(build_router());
    let rpc_ctx = build_rpc_context(app);
    let server = HciServer::start(cfg, router, rpc_ctx).await?;
    Ok(Some(server))
}

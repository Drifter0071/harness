//! HCI ↔ Tauri integration. Owns the lifecycle of the HCI server
//! relative to the Tauri app: it starts the server inside the
//! `setup` callback, threads the `AppState` and write-tier flag
//! into the `RpcContext`, and stops the server on app exit.

use std::sync::Arc;
use std::time::Duration;

use crate::commands::AppState;
use crate::hci::auth::AuthState;
use crate::hci::methods_canvas::CanvasEventBus;
use crate::hci::router::{RpcContext, Router};
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
/// - In dev builds, enabled by default.
/// - In release builds, disabled by default.
/// - `HARNESS_CONTROL=1` forces on.
pub fn hci_enabled_by_default() -> bool {
    match std::env::var("HARNESS_CONTROL").ok().as_deref() {
        Some("0") => false,
        Some("1") => true,
        _ => cfg!(debug_assertions),
    }
}

/// Returns true if the write tier should be exposed.
pub fn hci_write_enabled() -> bool {
    std::env::var("HARNESS_CONTROL_WRITE").ok().as_deref() == Some("1")
}

/// Construct the `RpcContext` the server will hand to handlers.
pub fn build_rpc_context(
    auth: Arc<AuthState>,
    app: Arc<AppState>,
) -> RpcContext {
    RpcContext {
        writes_enabled: hci_write_enabled(),
        auth: Some(auth),
        started_at: chrono::Utc::now(),
        app: Some(app),
        canvas_bus: Some(CanvasEventBus::new()),
    }
}

/// Start the HCI server. Returns `None` if disabled.
///
/// v0 takes no app-state argument: the server only needs to bind a
/// port and write the connection-info file. The `RpcContext.app` is
/// populated lazily when handlers are invoked via the Tauri IPC
/// bridge (a follow-up that wires WebView→Rust state plumbing).
pub async fn start() -> Result<Option<HciServer>, crate::hci::server::StartError> {
    if !hci_enabled_by_default() {
        return Ok(None);
    }
    let cfg = crate::hci::server::HciServerConfig {
        bind_addr: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        channel: if cfg!(debug_assertions) { "debug".into() } else { "release".into() },
        auth_timeout: Duration::from_secs(5),
        conn_info_dir_override: std::env::var("HARNESS_CONN_INFO_DIR")
            .ok()
            .map(std::path::PathBuf::from),
    };
    let server = HciServer::start(cfg).await?;
    Ok(Some(server))
}

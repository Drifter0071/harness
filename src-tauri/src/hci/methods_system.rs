//! Read-tier handlers for `system.*`.
//!
//! `system.ping` and `system.info` are the only two in v0; the rest
//! (`shutdown`, `reload_webview`) live behind the write tier and are
//! not yet implemented in WP4.

use serde_json::{json, Value};

use crate::hci::router::{MethodHandler, MethodTier, RpcContext, ConnectionCtx, RouterError};

pub struct PingHandler;
#[async_trait::async_trait]
impl MethodHandler for PingHandler {
    fn name(&self) -> &'static str { "system.ping" }
    fn tier(&self) -> MethodTier { MethodTier::Read }
    async fn handle(&self, _params: Value, ctx: &RpcContext, _conn: &ConnectionCtx) -> Result<Value, RouterError> {
        Ok(json!({ "pong": true, "uptime_ms": ctx.elapsed_ms() }))
    }
}

pub struct InfoHandler;
#[async_trait::async_trait]
impl MethodHandler for InfoHandler {
    fn name(&self) -> &'static str { "system.info" }
    fn tier(&self) -> MethodTier { MethodTier::Read }
    async fn handle(&self, _params: Value, ctx: &RpcContext, _conn: &ConnectionCtx) -> Result<Value, RouterError> {
        let channel = if cfg!(debug_assertions) { "debug" } else { "release" };
        let platform = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else {
            "unknown"
        };
        let webview = if cfg!(target_os = "windows") {
            "webview2"
        } else if cfg!(target_os = "macos") {
            "wkwebview"
        } else {
            "webkitgtk"
        };
        Ok(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "git_sha": option_env!("HARNESS_GIT_SHA").unwrap_or("unknown"),
            "channel": channel,
            "platform": platform,
            "webview": webview,
            "uptime_ms": ctx.elapsed_ms(),
            "writes_enabled": ctx.writes_enabled,
        }))
    }
}

pub fn register(router: crate::hci::router::Router) -> crate::hci::router::Router {
    router.register(PingHandler).register(InfoHandler)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::hci::auth::AuthState;

    fn ctx() -> (Arc<AuthState>, RpcContext) {
        let auth = AuthState::new("boot".into());
        let _ = auth.consume_boot_token("boot").unwrap();
        let ctx = RpcContext {
            writes_enabled: false,
            auth: Some(auth.clone()),
            started_at: chrono::Utc::now(),
            app: None,
            canvas_bus: Some(crate::hci::CanvasEventBus::new()),
        };
        (auth, ctx)
    }

    #[tokio::test]
    async fn ping_returns_pong_and_uptime() {
        let (_a, c) = ctx();
        let conn = ConnectionCtx { session_token: "ignored".into() };
        let r = PingHandler.handle(json!({}), &c, &conn).await.unwrap();
        assert_eq!(r["pong"], true);
        assert!(r["uptime_ms"].as_i64().is_some());
    }

    #[tokio::test]
    async fn info_returns_expected_shape() {
        let (_a, c) = ctx();
        let conn = ConnectionCtx { session_token: "ignored".into() };
        let r = InfoHandler.handle(json!({}), &c, &conn).await.unwrap();
        assert!(r["version"].is_string());
        assert!(r["platform"].is_string());
        assert!(r["webview"].is_string());
        assert_eq!(r["writes_enabled"], false);
    }
}

//! JSON-RPC 2.0 dispatcher for HCI.
//!
//! Responsibilities:
//! - Parse one newline-delimited frame at a time.
//! - Validate the session token (auth gate is upstream in `server.rs`).
//! - Enforce read vs write tier gating.
//! - Dispatch to method handlers by name. Handlers are registered in
//!   `methods.rs` (WP4–WP7).
//! - Emit JSON-RPC responses with the standard error codes plus the two
//!   custom codes the spec defines.
//!
//! The router is intentionally stateless: it owns no shared state and
//! does no I/O on its own. A caller feeds it a `RpcContext` (shared
//! application state) and a `ConnectionCtx` (per-connection, including
//! the session token) and asks it to handle one frame.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::commands::AppState;
use crate::hci::auth::AuthState;
use crate::hci::methods_canvas::CanvasEventBus;

/// The two custom JSON-RPC error codes the spec introduces on top of
/// the standard set.
pub const ERR_WRITE_DISABLED: i32 = -32001;
pub const ERR_AUTH_REQUIRED: i32 = -32002;

/// Shared application state the router hands to method handlers. In v0
/// this is a thin shim around the Tauri-managed state. Future versions
/// will grow this as we wire up the read tier.
#[derive(Clone)]
pub struct RpcContext {
    /// True if the `HARNESS_CONTROL_WRITE=1` env var was set. Gated
    /// behind a feature flag in production builds (see WP8).
    pub writes_enabled: bool,
    /// Auth state, used to validate session tokens on every request.
    pub auth: Option<Arc<AuthState>>,
    /// Server start time, used by `system.info` / `system.ping`.
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// The Tauri-managed `AppState`. Optional because tests build a
    /// `Router` without one. Handlers that need it check for `None`
    /// and return a clean error rather than panicking.
    pub app: Option<Arc<AppState>>,
    /// Broadcast bus for canvas events. The Tauri WebView bridge (WP7
    /// follow-up) and the Health Agent publish into this.
    pub canvas_bus: Option<CanvasEventBus>,
}

impl RpcContext {
    pub fn elapsed_ms(&self) -> i64 {
        (chrono::Utc::now() - self.started_at).num_milliseconds()
    }
}

impl Default for RpcContext {
    fn default() -> Self {
        Self {
            writes_enabled: false,
            auth: None,
            started_at: chrono::Utc::now(),
            app: None,
            canvas_bus: None,
        }
    }
}

/// Per-connection context. Built in `server.rs` after auth completes.
#[derive(Clone)]
pub struct ConnectionCtx {
    pub session_token: String,
}

#[derive(Debug, Error)]
pub enum RouterError {
    #[error("malformed JSON: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("auth required")]
    AuthRequired,
    #[error("write tier disabled; set HARNESS_CONTROL_WRITE=1 to enable")]
    WritesDisabled,
    #[error("method not found: {0}")]
    MethodNotFound(String),
    #[error("internal error: {0}")]
    Internal(String),
}

/// Parsed JSON-RPC request frame. The spec tolerates a missing
/// `jsonrpc: "2.0"` field on the wire; we always emit it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jsonrpc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl RpcRequest {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// JSON-RPC response envelope. Either a `result` or an `error`; the
/// spec requires one or the other but not both.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RpcResponse {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }
    pub fn err(id: Value, code: i32, message: &str) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.to_string(),
                data: None,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Trait every method handler implements. The router dispatches by
/// name and never invokes a handler without first checking tier and
/// auth. `handle` is async because most handlers call into
/// `commands::*` (which is async) or read shared state.
#[async_trait::async_trait]
pub trait MethodHandler: Send + Sync {
    fn name(&self) -> &'static str;
    fn tier(&self) -> MethodTier;
    async fn handle(
        &self,
        params: Value,
        ctx: &RpcContext,
        conn: &ConnectionCtx,
    ) -> Result<Value, RouterError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodTier {
    /// Always available (assuming the server is enabled).
    Read,
    /// Requires the write tier env var to be set.
    Write,
}

/// Registry of method handlers. Built once at server start. Lookups
/// are by exact method name match (e.g. `system.ping`, `agent.list`).
pub struct Router {
    handlers: Vec<Box<dyn MethodHandler>>,
}

impl Router {
    pub fn new() -> Self {
        Self { handlers: Vec::new() }
    }

    pub fn register(mut self, h: impl MethodHandler + 'static) -> Self {
        let name = h.name();
        if self.handlers.iter().any(|x| x.name() == name) {
            panic!("duplicate HCI method registration: {name}");
        }
        self.handlers.push(Box::new(h));
        self
    }

    /// Look up a handler by name.
    pub fn get(&self, name: &str) -> Option<&dyn MethodHandler> {
        self.handlers.iter().find(|h| h.name() == name).map(|h| h.as_ref())
    }

    pub fn methods(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.handlers.iter().map(|h| h.name())
    }

    /// Handle a single parsed request frame. Returns the response to
    /// send back. Notifications (no `id`) return `None` — caller doesn't
    /// write anything to the wire.
    pub async fn handle_request(
        &self,
        req: RpcRequest,
        ctx: &RpcContext,
        conn: &ConnectionCtx,
    ) -> Option<RpcResponse> {
        let id = req.id.clone().unwrap_or(Value::Null);

        // JSON-RPC notifications: no `id` field → no response on the
        // wire. Per spec, even an error must not produce a response.
        if req.is_notification() {
            // Still run the handler (or auth gate) so the call has
            // observable side effects, but swallow the result.
            let _ = self.dispatch(&req, ctx, conn).await;
            return None;
        }

        // Auth gate: every method except `auth` requires a valid session.
        if let Some(auth) = &ctx.auth {
            if req.method != "auth" && !auth.is_valid_session(&conn.session_token) {
                return Some(RpcResponse::err(id, ERR_AUTH_REQUIRED, "auth required"));
            }
        }

        self.dispatch(&req, ctx, conn).await
    }

    /// Inner dispatch: auth, tier, and method lookup. Returns the
    /// response envelope or `None` for notifications (which the
    /// outer function has already filtered).
    async fn dispatch(
        &self,
        req: &RpcRequest,
        ctx: &RpcContext,
        conn: &ConnectionCtx,
    ) -> Option<RpcResponse> {
        let id = req.id.clone().unwrap_or(Value::Null);

        if let Some(handler) = self.get(&req.method) {
            if handler.tier() == MethodTier::Write && !ctx.writes_enabled {
                return Some(RpcResponse::err(
                    id,
                    ERR_WRITE_DISABLED,
                    "write tier disabled",
                ));
            }
            return match handler.handle(req.params.clone(), ctx, conn).await {
                Ok(result) => Some(RpcResponse::ok(id, result)),
                Err(e) => {
                    let (code, msg) = match &e {
                        RouterError::MethodNotFound(_) => (-32601, e.to_string()),
                        RouterError::Internal(_) => (-32603, e.to_string()),
                        _ => (-32603, e.to_string()),
                    };
                    Some(RpcResponse::err(id, code, &msg))
                }
            };
        }

        Some(RpcResponse::err(
            id,
            -32601,
            &format!("method not found: {}", req.method),
        ))
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct EchoHandler;
    #[async_trait::async_trait]
    impl MethodHandler for EchoHandler {
        fn name(&self) -> &'static str { "echo" }
        fn tier(&self) -> MethodTier { MethodTier::Read }
        async fn handle(&self, params: Value, _ctx: &RpcContext, _conn: &ConnectionCtx) -> Result<Value, RouterError> {
            Ok(params)
        }
    }

    struct WriteHandler;
    #[async_trait::async_trait]
    impl MethodHandler for WriteHandler {
        fn name(&self) -> &'static str { "do.write" }
        fn tier(&self) -> MethodTier { MethodTier::Write }
        async fn handle(&self, _params: Value, _ctx: &RpcContext, _conn: &ConnectionCtx) -> Result<Value, RouterError> {
            Ok(Value::String("wrote".into()))
        }
    }

    fn make_ctx(writes: bool) -> (Arc<AuthState>, RpcContext) {
        let auth = AuthState::new("boot".into());
        // Pre-authenticate a session token.
        let _ = auth.consume_boot_token("boot").unwrap();
        let ctx = RpcContext {
            writes_enabled: writes,
            auth: Some(auth.clone()),
            started_at: chrono::Utc::now(),
            app: None,
            canvas_bus: None,
        };
        (auth, ctx)
    }

    #[tokio::test]
    async fn read_method_dispatches() {
        let (_auth, ctx) = make_ctx(false);
        // `make_ctx` already consumed the boot token and minted a
        // session. We can't recover that session from outside (the
        // session token isn't returned by make_ctx), so for this test
        // we just call rotate() to get a fresh boot token, then
        // consume it for a fresh session.
        let new_boot = ctx.auth.as_ref().unwrap().rotate();
        let session = ctx.auth.as_ref().unwrap().consume_boot_token(&new_boot).unwrap();
        let conn = ConnectionCtx { session_token: session };

        let router = Router::new().register(EchoHandler);
        let req: RpcRequest = serde_json::from_str(
            r#"{"id":1,"method":"echo","params":{"x":42}}"#,
        ).unwrap();
        let resp = router.handle_request(req, &ctx, &conn).await.unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["x"], 42);
    }

    #[tokio::test]
    async fn write_method_blocked_when_disabled() {
        let (_auth, _ctx) = make_ctx(false);
        let auth = AuthState::new("boot2".into());
        let session = auth.consume_boot_token("boot2").unwrap();
        let ctx = RpcContext {
            writes_enabled: false,
            auth: Some(auth),
            started_at: chrono::Utc::now(),
            app: None,
            canvas_bus: None,
        };
        let conn = ConnectionCtx { session_token: session };

        let router = Router::new().register(WriteHandler);
        let req: RpcRequest = serde_json::from_str(
            r#"{"id":"a","method":"do.write","params":{}}"#,
        ).unwrap();
        let resp = router.handle_request(req, &ctx, &conn).await.unwrap();
        assert_eq!(resp.error.unwrap().code, ERR_WRITE_DISABLED);
    }

    #[tokio::test]
    async fn write_method_succeeds_when_enabled() {
        let auth = AuthState::new("boot3".into());
        let session = auth.consume_boot_token("boot3").unwrap();
        let ctx = RpcContext {
            writes_enabled: true,
            auth: Some(auth),
            started_at: chrono::Utc::now(),
            app: None,
            canvas_bus: None,
        };
        let conn = ConnectionCtx { session_token: session };

        let router = Router::new().register(WriteHandler);
        let req: RpcRequest = serde_json::from_str(
            r#"{"id":"a","method":"do.write","params":{}}"#,
        ).unwrap();
        let resp = router.handle_request(req, &ctx, &conn).await.unwrap();
        assert_eq!(resp.result.unwrap(), Value::String("wrote".into()));
    }

    #[tokio::test]
    async fn unknown_method_returns_32601() {
        let auth = AuthState::new("boot4".into());
        let session = auth.consume_boot_token("boot4").unwrap();
        let ctx = RpcContext {
            writes_enabled: true,
            auth: Some(auth),
            started_at: chrono::Utc::now(),
            app: None,
            canvas_bus: None,
        };
        let conn = ConnectionCtx { session_token: session };
        let router = Router::new();
        let req: RpcRequest = serde_json::from_str(
            r#"{"id":1,"method":"nope"}"#,
        ).unwrap();
        let resp = router.handle_request(req, &ctx, &conn).await.unwrap();
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[tokio::test]
    async fn invalid_session_returns_auth_required() {
        let auth = AuthState::new("boot5".into());
        let _ = auth.consume_boot_token("boot5").unwrap();
        let ctx = RpcContext {
            writes_enabled: true,
            auth: Some(auth),
            started_at: chrono::Utc::now(),
            app: None,
            canvas_bus: None,
        };
        let conn = ConnectionCtx { session_token: "wrong".into() };
        let router = Router::new().register(EchoHandler);
        let req: RpcRequest = serde_json::from_str(
            r#"{"id":1,"method":"echo","params":{}}"#,
        ).unwrap();
        let resp = router.handle_request(req, &ctx, &conn).await.unwrap();
        assert_eq!(resp.error.unwrap().code, ERR_AUTH_REQUIRED);
    }

    #[tokio::test]
    async fn notification_returns_none() {
        let auth = AuthState::new("boot6".into());
        let session = auth.consume_boot_token("boot6").unwrap();
        let ctx = RpcContext {
            writes_enabled: true,
            auth: Some(auth),
            started_at: chrono::Utc::now(),
            app: None,
            canvas_bus: None,
        };
        let conn = ConnectionCtx { session_token: session };
        let router = Router::new().register(EchoHandler);
        let req: RpcRequest = serde_json::from_str(
            r#"{"method":"echo","params":{}}"#,
        ).unwrap();
        let resp = router.handle_request(req, &ctx, &conn).await;
        assert!(resp.is_none());
    }
}

//! Read-tier handlers for `canvas.*`.
//!
//! v0 caveat: the WebView is owned by Tauri; reaching into it from
//! Rust to extract the DOM/canvas state requires a JS↔Rust bridge
//! call (`webview.eval` or `with_webview`). Wiring that up is a
//! small follow-up (see `docs/superpowers/specs/...md` §4.4) and
//! the spec calls it out as deferred. For now the handlers exist,
//! return their declared shape, and signal "not yet wired" via a
//! `wired: false` field on the result so clients can tell apart
//! "empty canvas" from "this isn't implemented yet".

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::hci::router::{ConnectionCtx, MethodHandler, MethodTier, RpcContext, RouterError};

/// In-process broadcast bus for canvas events. The Tauri-side emitter
/// pushes; the per-connection task tails via `events.subscribe`.
#[derive(Clone)]
pub struct CanvasEventBus {
    inner: Arc<broadcast::Sender<Value>>,
}

impl Default for CanvasEventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl CanvasEventBus {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(64);
        Self { inner: Arc::new(tx) }
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.inner.subscribe()
    }
    /// Publish a canvas event. Used by the future WebView bridge (WP7
    /// follow-up) and by the in-process Health Agent.
    pub fn publish(&self, event: Value) {
        let _ = self.inner.send(event);
    }
}

pub struct SnapshotHandler;
impl MethodHandler for SnapshotHandler {
    fn name(&self) -> &'static str { "canvas.snapshot" }
    fn tier(&self) -> MethodTier { MethodTier::Read }
    fn handle(
        &self,
        _params: Value,
        _ctx: &RpcContext,
        _conn: &ConnectionCtx,
    ) -> Result<Value, RouterError> {
        // v0: surface the missing-bridge case explicitly so clients
        // don't mistake an empty canvas for an empty app.
        Ok(json!({
            "snapshot_id": uuid::Uuid::new_v4().to_string(),
            "captured_at": chrono::Utc::now().to_rfc3339(),
            "wired": false,
            "surfaces": [],
            "note": "canvas bridge not yet wired in v0; this is a placeholder",
        }))
    }
}

pub struct SubscribeHandler;
impl MethodHandler for SubscribeHandler {
    fn name(&self) -> &'static str { "events.subscribe" }
    fn tier(&self) -> MethodTier { MethodTier::Read }
    fn handle(
        &self,
        _params: Value,
        ctx: &RpcContext,
        _conn: &ConnectionCtx,
    ) -> Result<Value, RouterError> {
        // v0: the router doesn't push to connections from the broadcast
        // bus yet. We acknowledge the subscription so the protocol is
        // honest; the push path is part of the WebView bridge work
        // (deferred).
        let _bus = ctx.canvas_bus.as_ref();
        Ok(json!({ "ok": true, "wired": false }))
    }
}

pub fn register(router: crate::hci::router::Router) -> crate::hci::router::Router {
    router.register(SnapshotHandler).register(SubscribeHandler)
}

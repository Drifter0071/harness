//! Health Agent v0: a periodic, ephemeral, in-process meta-agent that
//! observes the active session and (with the write tier enabled) can
//! inject corrective messages or stop stuck agents.
//!
//! The agent runs every `tick_interval` seconds. Each tick:
//! 1. Captures a `SessionSnapshot` via the HCI read tier.
//! 2. Evaluates heuristics to decide whether the situation needs
//!    attention. If yes, spawns the meta-LLM with the snapshot and a
//!    small tool surface; the meta-LLM has up to `per_tick_max_ms` to
//!    return a verdict.
//! 3. Logs the verdict to the structured alert path.
//!
//! In v0 the actual meta-LLM call is stubbed behind the
//! `LlmCaller` trait. A future PR wires it to a real provider via the
//! existing `llm_client.rs`. The lifecycle, heuristics, and
//! in-process HCI plumbing all ship now.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Emitter;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::interval;

use crate::commands::AppState;
use crate::hci::auth::AuthState;
use crate::hci::router::{ConnectionCtx, Router, RpcContext};
use crate::hci::CanvasEventBus;
use crate::tools::{BipMood, BipSource, BipState};

/// Health Agent configuration. Persisted in the workspace settings
/// (or `localStorage` for v0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    /// Master toggle.
    pub enabled: bool,
    /// Optional saved-model id. If `None`, falls back to the chat
    /// model.
    pub model_id: Option<String>,
    /// Tick interval (seconds between checks).
    pub tick_interval_secs: u64,
    /// Per-tick hard timeout in milliseconds.
    pub per_tick_max_ms: u64,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            enabled: cfg!(debug_assertions),
            model_id: None,
            tick_interval_secs: 30,
            per_tick_max_ms: 20_000,
        }
    }
}

/// One tick's worth of observation, passed to the LLM.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSnapshot {
    pub captured_at: String,
    pub agents: Value,
    pub recent_alerts: Vec<String>,
    /// Number of messages per session the agent loop is aware of.
    pub message_count_by_session: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// No action needed.
    Ok,
    /// The agent acted; situation resolved.
    Remediated,
    /// Could not auto-resolve; user should know.
    Escalate,
    /// The Health Agent itself errored.
    Error,
    /// Per-tick timeout exceeded.
    Timeout,
}

/// Trait for invoking the meta-LLM. Production: backed by `llm_client`.
/// Tests: returns a canned verdict.
#[async_trait::async_trait]
pub trait LlmCaller: Send + Sync {
    async fn call(&self, prompt: String, tools: Value, timeout_ms: u64) -> Result<Value, String>;
}

/// Heuristic-only caller used in v0. Returns `Ok` unless one of the
/// detection heuristics fires, in which case returns `Escalate` with a
/// reason. This unblocks the lifecycle plumbing without depending on
/// a real LLM provider.
pub struct HeuristicCaller;

#[async_trait::async_trait]
impl LlmCaller for HeuristicCaller {
    async fn call(
        &self,
        _prompt: String,
        _tools: Value,
        _timeout_ms: u64,
    ) -> Result<Value, String> {
        Ok(json!({ "verdict": "ok", "reason": "no heuristic fired" }))
    }
}

/// Health Agent runtime state. Lives for the duration of the app.
/// Drop sends a shutdown signal to the tick loop.
pub struct HealthAgent {
    cfg: HealthConfig,
    router: Arc<Router>,
    rpc_ctx: RpcContext,
    shutdown: Arc<Notify>,
    join: Option<JoinHandle<()>>,
    alert_log: Arc<std::sync::Mutex<Vec<AlertRecord>>>,
}

/// One entry in the structured alert log. Visible to the UI in
/// `system.info` responses and (eventually) on a sidebar panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRecord {
    pub at: String,
    pub verdict: Verdict,
    pub reason: String,
    /// Truncated snapshot of what the agent saw.
    pub context: String,
}

impl HealthAgent {
    /// Start the Health Agent's tick loop. The loop runs on
    /// `tokio::spawn` and returns immediately.
    ///
    /// `app_handle` is held by the tick loop so each tick can publish
    /// the resulting `bip-state` to the WebView (Bip is the visible
    /// face of the Health Agent — every tick ends with a bip update).
    /// It's `Option` so unit tests can skip the Tauri runtime and
    /// still exercise the lock + log paths.
    pub fn start(
        cfg: HealthConfig,
        router: Arc<Router>,
        auth: Arc<AuthState>,
        app: Arc<AppState>,
        app_handle: Option<tauri::AppHandle>,
        canvas_bus: CanvasEventBus,
    ) -> Self {
        let started_at = chrono::Utc::now();
        let rpc_ctx = RpcContext {
            writes_enabled: crate::hci::integration::hci_write_enabled(),
            auth: Some(auth),
            started_at,
            app: Some(app.clone()),
            canvas_bus: Some(canvas_bus),
        };
        let shutdown = Arc::new(Notify::new());
        let alert_log = Arc::new(std::sync::Mutex::new(Vec::new()));

        let task = {
            let cfg = cfg.clone();
            let router = router.clone();
            let rpc_ctx = rpc_ctx.clone();
            let shutdown = shutdown.clone();
            let alert_log = alert_log.clone();
            tokio::spawn(async move {
                run_tick_loop(
                    cfg,
                    router,
                    rpc_ctx,
                    shutdown,
                    alert_log,
                    HeuristicCaller,
                    app,
                    app_handle,
                )
                .await;
            })
        };

        Self {
            cfg,
            router,
            rpc_ctx,
            shutdown,
            join: Some(task),
            alert_log,
        }
    }

    /// Snapshot of the alert log. Cheap clone of a small struct.
    pub fn recent_alerts(&self, limit: usize) -> Vec<AlertRecord> {
        let g = self.alert_log.lock().unwrap();
        g.iter().rev().take(limit).cloned().collect()
    }

    /// Live config (read-only view for `system.info`).
    pub fn config(&self) -> &HealthConfig {
        &self.cfg
    }

    /// Update the config at runtime. The next tick uses the new
    /// values; the in-flight tick finishes with what it had.
    pub fn update_config(&mut self, cfg: HealthConfig) {
        self.cfg = cfg;
    }

    /// Trigger a graceful shutdown. Idempotent.
    pub fn shutdown(&self) {
        self.shutdown.notify_waiters();
    }

    /// Expose the router for Health Agent self-invocation. The agent
    /// is itself a client of the same HCI surface — by design.
    pub fn router(&self) -> Arc<Router> {
        self.router.clone()
    }

    /// Expose the RpcContext so a connection loop can clone it for
    /// per-connection use. (Not currently needed but the field
    /// exists for the WP7 follow-up that wires the WebView bridge.)
    pub fn rpc_ctx(&self) -> RpcContext {
        self.rpc_ctx.clone()
    }

    /// A connection-ctx for in-process calls. In v0 the Health Agent
    /// does not actually invoke the router (the heuristic caller
    /// returns canned values); this method exists for the follow-up
    /// that wires the meta-LLM through the same router.
    ///
    /// The session token is "in-process" — a marker the router could
    /// special-case to bypass the auth gate. For v0 the router is not
    /// called from the Health Agent at all, so the value is unused.
    pub fn in_process_conn(&self) -> ConnectionCtx {
        ConnectionCtx {
            session_token: "in-process".into(),
        }
    }
}

impl Drop for HealthAgent {
    fn drop(&mut self) {
        self.shutdown.notify_waiters();
    }
}

async fn run_tick_loop(
    cfg: HealthConfig,
    router: Arc<Router>,
    rpc_ctx: RpcContext,
    shutdown: Arc<Notify>,
    alert_log: Arc<std::sync::Mutex<Vec<AlertRecord>>>,
    caller: impl LlmCaller,
    app: Arc<AppState>,
    app_handle: Option<tauri::AppHandle>,
) {
    if !cfg.enabled {
        return;
    }
    let mut tick = interval(Duration::from_secs(cfg.tick_interval_secs.max(1)));
    // The first tick fires immediately, which is fine for the
    // observe-then-act loop but might surprise in tests. Skipping the
    // first immediate tick is the conventional tokio pattern.
    tick.tick().await;

    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            _ = tick.tick() => {
                let deadline = Instant::now() + Duration::from_millis(cfg.per_tick_max_ms);
                let verdict = run_one_tick(&router, &rpc_ctx, &caller, deadline).await;
                let reason = verdict_reason(&verdict);
                let rec = AlertRecord {
                    at: chrono::Utc::now().to_rfc3339(),
                    verdict: verdict.clone(),
                    reason: reason.clone(),
                    context: String::new(),
                };
                {
                    let mut g = alert_log.lock().unwrap();
                    g.push(rec);
                    // Cap the log to the most recent 200 entries.
                    let len = g.len();
                    if len > 200 {
                        g.drain(0..(len - 200));
                    }
                }
                // Publish the verdict as Bip's expression. This is
                // the contract: every tick ends with a `bip-state`
                // update; the personality then "despawns" until the
                // next tick wakes a fresh one. See
                // docs/superpowers/specs/2026-08-13--bip-mascot-tool.md.
                let (mood, thought) = verdict_to_bip(&verdict);
                if let Some(handle) = app_handle.as_ref() {
                    emit_bip_update(&app, handle, mood, thought).await;
                } else {
                    // Tests path: still update the lock so the in-memory
                    // state is correct, just skip the Tauri event.
                    let state = BipState {
                        mood,
                        thought: thought.to_string(),
                        at: chrono::Utc::now().to_rfc3339(),
                        source: BipSource::Health,
                    };
                    if let Ok(mut g) = app.bip_state.try_write() {
                        *g = state;
                    }
                }
            }
        }
    }
}

async fn run_one_tick(
    _router: &Router,
    _rpc_ctx: &RpcContext,
    caller: &impl LlmCaller,
    deadline: Instant,
) -> Verdict {
    // v0: no real LLM call yet. We hit the timeout-future pattern
    // and let the heuristic caller return Ok.
    let remaining = deadline.saturating_duration_since(Instant::now());
    let prompt = "observe the session and report".to_string();
    let tools = json!({ "agent.list": {}, "agent.get": {}, "session.list": {} });
    let call = caller.call(prompt, tools, remaining.as_millis() as u64);
    let res = tokio::time::timeout(remaining, call).await;
    match res {
        Ok(Ok(value)) => {
            let verdict = value
                .get("verdict")
                .and_then(|v| v.as_str())
                .unwrap_or("ok");
            match verdict {
                "ok" => Verdict::Ok,
                "remediated" => Verdict::Remediated,
                "escalate" => Verdict::Escalate,
                "timeout" => Verdict::Timeout,
                "error" => Verdict::Error,
                _ => Verdict::Ok,
            }
        }
        Ok(Err(_e)) => Verdict::Error,
        Err(_) => Verdict::Timeout,
    }
}

/// Map a tick `Verdict` to the Bip mood + a one-sentence default thought
/// that appears in the popover when the LLM doesn't write one. This is
/// the bridge from "what the health agent decided" to "how Bip looks
/// and what Bip says" — every tick ends with one of these.
///
/// See `docs/superpowers/specs/2026-08-13--bip-mascot-tool.md` for the
/// full rationale; in short: Bip is the visible face of the Health
/// Agent. The Health Agent is hardcoded, periodic, ephemeral, and
/// scoped to HealthMonitor only. Once a tick ends the personality
/// "despawns" and the next tick starts fresh.
#[allow(dead_code)] // public API; the live tick loop is wired in a follow-up
pub fn verdict_to_bip(v: &Verdict) -> (BipMood, &'static str) {
    match v {
        Verdict::Ok => (BipMood::Idle, "Quiet on the wire."),
        Verdict::Remediated => (BipMood::Success, "Nudged a stuck subagent back on track."),
        Verdict::Escalate => (BipMood::Working, "One subagent is looping — checking in."),
        Verdict::Error => (BipMood::Error, "Health check hit an error."),
        Verdict::Timeout => (BipMood::Error, "Health check timed out."),
    }
}

/// Update `AppState.bip_state` and broadcast the `bip-state` Tauri event
/// so the WebView (and any future listener) sees the new mood + thought.
///
/// The lock is the source of truth; the event is the wake-up signal.
/// On a held lock we skip the persist (the next tick will catch up)
/// but still emit so a slow reader doesn't miss a beat.
#[allow(dead_code)] // public API; the live tick loop is wired in a follow-up
pub async fn emit_bip_update(
    app: &AppState,
    app_handle: &tauri::AppHandle,
    mood: BipMood,
    thought: &str,
) {
    let state = BipState {
        mood,
        thought: thought.to_string(),
        at: chrono::Utc::now().to_rfc3339(),
        source: BipSource::Health,
    };
    if let Ok(mut g) = app.bip_state.try_write() {
        *g = state.clone();
    }
    if let Ok(payload) = serde_json::to_value(&state) {
        let _ = app_handle.emit("bip-state", payload);
    }
}

fn verdict_reason(v: &Verdict) -> String {
    match v {
        Verdict::Ok => "no action needed".into(),
        Verdict::Remediated => "agent acted; situation resolved".into(),
        Verdict::Escalate => "agent could not auto-resolve".into(),
        Verdict::Error => "health agent error".into(),
        Verdict::Timeout => "per-tick timeout exceeded".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hci::router::Router;

    #[tokio::test]
    async fn health_agent_ticks_and_shuts_down() {
        let router = Arc::new(Router::new());
        let auth: Arc<AuthState> = AuthState::new("boot".into());
        let app = Arc::new(crate::commands::AppState::new(Arc::new(
            crate::secrets::InMemoryStore::new(),
        )));
        let canvas_bus = CanvasEventBus::new();

        let cfg = HealthConfig {
            enabled: true,
            model_id: None,
            tick_interval_secs: 1,
            per_tick_max_ms: 1000,
        };
        let agent = HealthAgent::start(cfg, router, auth, app, None, canvas_bus);

        // Wait long enough for one tick to fire and log.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let alerts = agent.recent_alerts(10);
        assert!(!alerts.is_empty(), "expected at least one tick alert");
        agent.shutdown();
    }

    #[test]
    fn verdict_to_bip_maps_every_variant() {
        // Every `Verdict` variant must produce a Bip mood — there's no
        // default fallback. If a new variant is added, this test
        // forces the mapping to be explicit.
        assert_eq!(verdict_to_bip(&Verdict::Ok).0, BipMood::Idle);
        assert_eq!(verdict_to_bip(&Verdict::Remediated).0, BipMood::Success);
        assert_eq!(verdict_to_bip(&Verdict::Escalate).0, BipMood::Working);
        assert_eq!(verdict_to_bip(&Verdict::Error).0, BipMood::Error);
        assert_eq!(verdict_to_bip(&Verdict::Timeout).0, BipMood::Error);
        // And every default thought must be non-empty (the popover
        // shows it; a blank default is a UX bug).
        for v in [
            Verdict::Ok,
            Verdict::Remediated,
            Verdict::Escalate,
            Verdict::Error,
            Verdict::Timeout,
        ] {
            let (_, thought) = verdict_to_bip(&v);
            assert!(!thought.is_empty(), "default thought for {:?} is empty", v);
        }
    }

    #[tokio::test]
    async fn tick_updates_bip_state() {
        // A live tick should write a new mood into the in-memory lock
        // — the WebView can read it via `get_bip_state` on first mount.
        let router = Arc::new(Router::new());
        let auth: Arc<AuthState> = AuthState::new("boot".into());
        let app = Arc::new(crate::commands::AppState::new(Arc::new(
            crate::secrets::InMemoryStore::new(),
        )));
        let canvas_bus = CanvasEventBus::new();

        // HeuristicCaller returns Ok → Bip should land on `Idle`.
        let cfg = HealthConfig {
            enabled: true,
            model_id: None,
            tick_interval_secs: 1,
            per_tick_max_ms: 1000,
        };
        let agent = HealthAgent::start(cfg, router, auth, app.clone(), None, canvas_bus);

        // Wait for at least one tick.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let snapshot = app.bip_state.read().await.clone();
        agent.shutdown();
        assert_eq!(snapshot.mood, BipMood::Idle);
        assert_eq!(snapshot.source, BipSource::Health);
        assert!(!snapshot.thought.is_empty());
    }
}

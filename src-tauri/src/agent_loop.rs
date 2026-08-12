//! Rust-owned agent turn loop (spec §4, §5).
//!
//! One `run_agent_turn` call drives a complete agent turn: stream the
//! provider response, execute requested tool calls through the policy gate
//! (auto / ask-risky / ask-all with an in-app approval prompt), feed tool
//! results back, and keep looping until the model answers without tool
//! calls. Goal mode turns on a `[continue]` marker loop so the model can
//! chain work until it finishes or hits a cap.
//!
//! The loop is deliberately UI-free: all events flow through
//! [`AgentEventSink`] and all provider calls through [`TurnCompleter`], so
//! tests exercise the whole policy/approval/goal machinery with mocks and
//! never touch Tauri or the network.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use dashmap::DashMap;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::llm_client::{
    complete_with_tools, AgentTurnError, ChatUsage, ToolCall, ToolChatMessage, ToolChatRequest,
    ToolSpec, TurnOutcome,
};
use crate::secrets::SecretStore;
use crate::session::{
    save_session, ActionMode, Message, Session, TokenStats, ToolActivity, ToolActivityStatus,
};
use crate::tools::{execute_tool, ToolResult};

/// Hard cap on LLM rounds inside one turn (each round may execute tools).
const MAX_ROUNDS: usize = 24;
/// Consecutive identical tool calls with no successful mutation trip the
/// stall detector after this many rounds.
const STALL_WINDOW: usize = 3;
/// Tools that mutate the workspace; everything else is read-only for stall
/// detection and treated as "safe" under `AskRisky`.
const RISKY_TOOLS: [&str; 3] = ["write_file", "edit_file", "run_command"];
/// Goal-mode continuation marker the model appends to keep working.
const CONTINUE_MARKER: &str = "[continue]";
/// Cap on goal-mode continuation turns after the first.
const GOAL_MAX_TURNS: usize = 8;

// -- Event wire types (camelCase, routed by `requestId` on the frontend) --

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnStartEvent<'a> {
    request_id: &'a str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChunkEvent<'a> {
    request_id: &'a str,
    delta: &'a str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolStartEvent<'a> {
    request_id: &'a str,
    call_id: String,
    name: String,
    arguments: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolResultEvent<'a> {
    request_id: &'a str,
    call_id: String,
    name: String,
    ok: bool,
    summary: String,
    status: &'a str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalRequestEvent<'a> {
    request_id: &'a str,
    approval_id: String,
    name: String,
    summary: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalResultEvent<'a> {
    request_id: &'a str,
    approval_id: String,
    allowed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DoneEvent<'a> {
    request_id: &'a str,
    content: &'a str,
    usage: ChatUsage,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorEvent<'a> {
    request_id: &'a str,
    kind: &'a str,
    message: &'a str,
}

// -- Seams --

/// Where the loop pushes events. The production impl emits Tauri app
/// events; tests use an in-memory recorder.
pub trait AgentEventSink: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}

/// Completes one provider turn. The boxed-future pattern keeps the trait
/// object-safe without pulling in async-trait.
pub trait TurnCompleter: Send + Sync {
    #[allow(clippy::type_complexity)]
    fn complete(
        &self,
        request: ToolChatRequest,
        tools: Vec<ToolSpec>,
        text_cb: Box<dyn Fn(&str) + Send>,
        cancel_rx: oneshot::Receiver<()>,
    ) -> Pin<Box<dyn Future<Output = Result<TurnOutcome, AgentTurnError>> + Send>>;
}

/// Production completer backed by `complete_with_tools` (owns the secret
/// store so the loop itself never touches credentials).
pub struct LlmClientTurner {
    secrets: Arc<dyn SecretStore>,
}

impl LlmClientTurner {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self { secrets }
    }
}

impl TurnCompleter for LlmClientTurner {
    fn complete(
        &self,
        request: ToolChatRequest,
        tools: Vec<ToolSpec>,
        text_cb: Box<dyn Fn(&str) + Send>,
        cancel_rx: oneshot::Receiver<()>,
    ) -> Pin<Box<dyn Future<Output = Result<TurnOutcome, AgentTurnError>> + Send>> {
        let secrets = Arc::clone(&self.secrets);
        Box::pin(async move {
            complete_with_tools(&request, &tools, secrets.as_ref(), text_cb, cancel_rx).await
        })
    }
}

/// Production sink that broadcasts Tauri app events (payloads are already
/// serialized to JSON by the caller).
pub struct AppEventSink {
    app: tauri::AppHandle,
}

impl AppEventSink {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl AgentEventSink for AppEventSink {
    fn emit(&self, event: &str, payload: Value) {
        use tauri::Emitter;
        let _ = self.app.emit(event, payload);
    }
}

/// Per-turn cancellation: the sender is registered in the `in_flight` map
/// so `cancel_agent(request_id)` can flip it; the loop races `changed()` /
/// `wait_for()` at every await. Watch receivers are "sticky" — a cancel sent
/// between awaits is observed at the next await, so no cancellation is lost.
fn new_cancel_token() -> (watch::Sender<bool>, watch::Receiver<bool>) {
    watch::channel(false)
}

/// Drop guard that resolves any still-pending approval for this turn as
/// denied. Covers aborts mid-approval (turn dropped, cancelled, panic).
struct PendingApprovalGuard {
    pending: Arc<DashMap<String, oneshot::Sender<bool>>>,
    request_id: String,
}

impl Drop for PendingApprovalGuard {
    fn drop(&mut self) {
        if let Some((_, tx)) = self.pending.remove(&self.request_id) {
            let _ = tx.send(false);
        }
    }
}

// -- Policy gate --

fn requires_approval(action_mode: &ActionMode, name: &str) -> bool {
    match action_mode {
        ActionMode::Auto => false,
        ActionMode::AskRisky => RISKY_TOOLS.contains(&name),
        ActionMode::AskAll => true,
    }
}

/// True when a tool mutates the workspace; used for stall detection.
fn is_mutating(name: &str) -> bool {
    RISKY_TOOLS.contains(&name)
}

// -- History conversion --

/// Rebuild the provider history from persisted session messages. Tool-granular
/// `role:"tool"` records are not persisted (they live in `activities`), so a
/// fresh turn replays user/assistant text only — the assistant summary carries
/// the state across turns. Within a turn, the loop keeps the full
/// assistant-echo + tool-result sequence in `messages` so OpenAI accepts
/// multi-round tool use.
fn history_from_session(session: &Session) -> Vec<ToolChatMessage> {
    session
        .messages
        .iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .map(|m| ToolChatMessage {
            role: m.role.clone(),
            content: m.content.clone(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        })
        .collect()
}

fn build_system_prompt(session: &Session, workspace: &str, goal_mode: bool) -> String {
    let mut prompt = session.agent.system_prompt.clone();
    prompt.push_str("\n\nWorkspace root: ");
    prompt.push_str(workspace);
    prompt.push_str(
        "\n\nYou are running inside the user's workspace. Use the provided tools to inspect \
         and modify files. Prefer many small, verified steps over one large blind change. \
         After every mutation, re-read the affected file to confirm the result.",
    );
    if goal_mode {
        prompt.push_str(&format!(
            "\n\nGoal mode is active: when your work is not finished yet, end your reply \
             with the marker `{}` and the loop will continue feeding you. Never claim \
             completion while work remains.",
            CONTINUE_MARKER
        ));
    }
    prompt
}

fn build_request(
    session: &Session,
    model_id: &str,
    workspace: &str,
    goal_mode: bool,
    messages: &[ToolChatMessage],
) -> ToolChatRequest {
    ToolChatRequest {
        model_id: model_id.to_string(),
        system_prompt: build_system_prompt(session, workspace, goal_mode),
        messages: messages.to_vec(),
    }
}

// -- Token telemetry --

fn merge_usage(stats: &mut Option<TokenStats>, usage: &ChatUsage) {
    let s = stats.get_or_insert_with(|| TokenStats {
        last_in: 0,
        last_out: 0,
        cumulative_in: 0,
        cumulative_out: 0,
        session_window_used: None,
        session_window_resets_at: None,
    });
    s.last_in = usage.input;
    s.last_out = usage.output;
    // `cumulativeIn` is the peak input size, not a sum (spec §4.5).
    s.cumulative_in = s.cumulative_in.max(usage.input);
    s.cumulative_out = s.cumulative_out.saturating_add(usage.output);
}

// -- Persistence --

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn user_message(session: &mut Session, text: &str) {
    session.messages.push(Message {
        id: Uuid::new_v4().to_string(),
        role: "user".into(),
        content: text.to_string(),
        timestamp: now_iso(),
        mentions: Vec::new(),
        agent_id: None,
        agent_name: None,
    });
}

fn assistant_message(session: &mut Session, text: &str) {
    session.messages.push(Message {
        id: Uuid::new_v4().to_string(),
        role: "assistant".into(),
        content: text.to_string(),
        timestamp: now_iso(),
        mentions: Vec::new(),
        agent_id: Some(session.agent.id.clone()),
        agent_name: Some(session.agent.name.clone()),
    });
}

fn push_activity(session: &mut Session, tool: &str, summary: String, status: ToolActivityStatus) {
    session.activities.push(ToolActivity {
        id: Uuid::new_v4().to_string(),
        tool: tool.to_string(),
        summary,
        status,
        timestamp: now_iso(),
    });
}

/// Persist the session after any mutation. `save_session` forces version 2,
/// rewrites `updatedAt` and auto-titles; failures are logged, not fatal to
/// the in-memory turn.
async fn persist(session: &mut Session) {
    if let Err(e) = save_session(&session.project_path, session.clone()).await {
        eprintln!(
            "agent_loop: failed to persist session {}: {}",
            session.id, e
        );
    }
}

// -- Stall detection --

#[derive(Clone)]
struct StallProbe {
    name: String,
    args_json: String,
    changed: bool,
}

/// `STALL_WINDOW` identical tool calls in a row, none of which mutated the
/// workspace, means the model is spinning. Returns the offending call name.
fn detect_stall(ring: &VecDeque<StallProbe>) -> Option<String> {
    if ring.len() < STALL_WINDOW {
        return None;
    }
    let first = ring.front()?;
    let same = ring
        .iter()
        .all(|p| p.name == first.name && p.args_json == first.args_json && !p.changed);
    if same {
        Some(first.name.clone())
    } else {
        None
    }
}

fn has_continue_marker(text: &str) -> bool {
    text.trim_end().ends_with(CONTINUE_MARKER)
}

fn strip_continue_marker(text: &str) -> String {
    text.trim_end()
        .strip_suffix(CONTINUE_MARKER)
        .map(|s| s.trim_end().to_string())
        .unwrap_or_else(|| text.to_string())
}

// -- The loop --

#[allow(clippy::too_many_arguments)]
pub fn run_agent_turn(
    request_id: String,
    session: Session,
    model_id: String,
    message_text: String,
    action_mode: ActionMode,
    goal_mode: bool,
    workspace: String,
    sink: Arc<dyn AgentEventSink>,
    pending_approvals: Arc<DashMap<String, oneshot::Sender<bool>>>,
    in_flight: Arc<DashMap<String, watch::Sender<bool>>>,
    completer: Arc<dyn TurnCompleter>,
) -> JoinHandle<()> {
    tokio::spawn(run_agent_turn_inner(
        request_id,
        session,
        model_id,
        message_text,
        action_mode,
        goal_mode,
        workspace,
        sink,
        pending_approvals,
        in_flight,
        completer,
    ))
}

/// How a turn ended, after the final message is persisted.
enum TurnFinish {
    Done { content: String, usage: ChatUsage },
    Error { kind: String, message: String },
}

#[allow(clippy::too_many_arguments)]
async fn run_agent_turn_inner(
    request_id: String,
    mut session: Session,
    model_id: String,
    message_text: String,
    action_mode: ActionMode,
    goal_mode: bool,
    workspace: String,
    sink: Arc<dyn AgentEventSink>,
    pending_approvals: Arc<DashMap<String, oneshot::Sender<bool>>>,
    in_flight: Arc<DashMap<String, watch::Sender<bool>>>,
    completer: Arc<dyn TurnCompleter>,
) {
    let outcome = run_turn_core(
        &request_id,
        &mut session,
        &model_id,
        &message_text,
        &action_mode,
        goal_mode,
        &workspace,
        &sink,
        &pending_approvals,
        &in_flight,
        completer.as_ref(),
    )
    .await;

    // Turn finished (success, error or cancellation): deregister, then
    // emit the terminal event.
    let _ = in_flight.remove(&request_id);
    match outcome {
        Ok(TurnFinish::Done { content, usage }) => {
            sink.emit(
                "agent-done",
                json!(DoneEvent {
                    request_id: &request_id,
                    content: &content,
                    usage
                }),
            );
        }
        Ok(TurnFinish::Error { kind, message }) => {
            sink.emit(
                "agent-error",
                json!(ErrorEvent {
                    request_id: &request_id,
                    kind: &kind,
                    message: &message
                }),
            );
        }
        Err(()) => {
            sink.emit(
                "agent-error",
                json!(ErrorEvent {
                    request_id: &request_id,
                    kind: "cancelled",
                    message: "Agent turn cancelled by the user",
                }),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_turn_core(
    request_id: &str,
    session: &mut Session,
    model_id: &str,
    message_text: &str,
    action_mode: &ActionMode,
    goal_mode: bool,
    workspace: &str,
    sink: &Arc<dyn AgentEventSink>,
    pending_approvals: &Arc<DashMap<String, oneshot::Sender<bool>>>,
    in_flight: &Arc<DashMap<String, watch::Sender<bool>>>,
    completer: &dyn TurnCompleter,
) -> Result<TurnFinish, ()> {
    let (cancel_tx, mut cancel_rx) = new_cancel_token();
    in_flight.insert(request_id.to_string(), cancel_tx);

    sink.emit("agent-turn-start", json!(TurnStartEvent { request_id }));

    // Message history: prior session text + the new user message. The
    // assistant turn output (with tool-call echoes and tool results) is
    // appended as the rounds go.
    let mut messages = history_from_session(session);
    user_message(session, message_text);
    messages.push(ToolChatMessage {
        role: "user".into(),
        content: message_text.to_string(),
        tool_call_id: None,
        tool_calls: Vec::new(),
    });

    let mut llm_rounds = 0usize;
    let mut goal_turns = 0usize;
    let mut stall_ring: VecDeque<StallProbe> = VecDeque::with_capacity(STALL_WINDOW);
    let tools = ToolSpec::all();

    loop {
        if llm_rounds >= MAX_ROUNDS {
            return Ok(finish_error(
                session,
                "loop_cap",
                "Agent turn exceeded the maximum number of LLM rounds",
            )
            .await);
        }

        // -- Provider call, raced against cancellation --
        let request = build_request(session, model_id, workspace, goal_mode, &messages);
        let text_cb: Box<dyn Fn(&str) + Send> = {
            let sink = Arc::clone(sink);
            let request_id = request_id.to_string();
            Box::new(move |delta: &str| {
                sink.emit(
                    "agent-chunk",
                    json!(ChunkEvent {
                        request_id: &request_id,
                        delta,
                    }),
                );
            })
        };
        // The provider's internal cancel receiver never fires: the outer
        // select drops the whole call future on cancellation, which aborts
        // the stream. The sender must stay alive so the receiver is not
        // "closed" (that would abort the request immediately).
        let (cancel_keep, cancel_pass) = oneshot::channel::<()>();
        let _cancel_keep = cancel_keep;
        // Send-safe cancellation future: resolves only when the watch flips.
        // A dropped sender mid-turn cannot happen (the sender lives in
        // `in_flight` until the turn ends); if it ever does, park rather
        // than misreport.
        let cancel_fut = {
            let mut rx = cancel_rx.clone();
            async move {
                loop {
                    if *rx.borrow() {
                        return;
                    }
                    match rx.changed().await {
                        Ok(()) => {
                            if *rx.borrow() {
                                return;
                            }
                        }
                        Err(_) => std::future::pending::<()>().await,
                    }
                }
            }
        };
        let llm_fut = completer.complete(request, tools.clone(), text_cb, cancel_pass);
        let outcome = tokio::select! {
            biased;
            _ = cancel_fut => {
                return Err(());
            }
            r = llm_fut => match r {
                Ok(o) => o,
                Err(e) => {
                    let (kind, msg) = error_vocab(&e);
                    return Ok(finish_error(session, kind, msg).await);
                }
            }
        };
        llm_rounds += 1;

        // Persist the assistant text (continue marker stripped when the loop
        // keeps going, intact when this is the final answer).
        let has_marker = goal_mode && has_continue_marker(&outcome.text);
        let final_text = if has_marker && outcome.tool_calls.is_empty() {
            strip_continue_marker(&outcome.text)
        } else {
            outcome.text.clone()
        };
        assistant_message(session, &final_text);
        merge_usage(&mut session.token_stats, &outcome.usage);
        persist(session).await;

        // -- Execute requested tool calls --
        if !outcome.tool_calls.is_empty() {
            messages.push(ToolChatMessage {
                role: "assistant".into(),
                content: final_text.clone(),
                tool_call_id: None,
                tool_calls: outcome.tool_calls.clone(),
            });
            for call in &outcome.tool_calls {
                let allowed = gate_tool(
                    request_id,
                    session,
                    action_mode,
                    call,
                    &mut messages,
                    sink,
                    pending_approvals,
                    &mut cancel_rx,
                )
                .await?;
                if !allowed {
                    continue;
                }
                execute_one(
                    request_id,
                    session,
                    workspace,
                    call,
                    &mut messages,
                    sink,
                    &mut stall_ring,
                )
                .await;
            }
            if let Some(name) = detect_stall(&stall_ring) {
                return Ok(finish_error(
                    session,
                    "stalled",
                    format!("Agent repeated the same {} call without effect", name),
                )
                .await);
            }
            continue;
        }

        // -- No tool calls: goal continuation or done --
        if has_marker {
            goal_turns += 1;
            if goal_turns > GOAL_MAX_TURNS {
                return Ok(finish_error(
                    session,
                    "loop_cap",
                    "Goal loop exceeded the maximum number of continuation turns",
                )
                .await);
            }
            let marker_msg = CONTINUE_MARKER.to_string();
            user_message(session, &marker_msg);
            messages.push(ToolChatMessage {
                role: "user".into(),
                content: marker_msg,
                tool_call_id: None,
                tool_calls: Vec::new(),
            });
            continue;
        }

        return Ok(TurnFinish::Done {
            content: final_text,
            usage: outcome.usage,
        });
    }
}

fn error_vocab(e: &AgentTurnError) -> (&'static str, String) {
    match e {
        AgentTurnError::Auth(m) => ("auth", m.clone()),
        AgentTurnError::RateLimited(_) => (
            "rate_limited",
            "Provider rate limit hit — try again shortly".into(),
        ),
        AgentTurnError::Network(m) => ("network", m.clone()),
        AgentTurnError::Provider(m) => ("provider", m.clone()),
        AgentTurnError::BadRequest(m) => ("bad_request", m.clone()),
        AgentTurnError::Cancelled => ("cancelled", "Agent turn cancelled".into()),
        AgentTurnError::Internal(m) => ("internal", m.clone()),
    }
}

/// Record a turn-terminating error as an activity and persist, so the UI can
/// show why a turn stopped even after the terminal event.
async fn finish_error(session: &mut Session, kind: &str, message: impl Into<String>) -> TurnFinish {
    let message = message.into();
    push_activity(
        session,
        "agent",
        format!("turn error ({}): {}", kind, message),
        ToolActivityStatus::Error,
    );
    persist(session).await;
    TurnFinish::Error {
        kind: kind.to_string(),
        message,
    }
}

/// Run the action-policy gate for one tool call. Returns Ok(true) to run,
/// Ok(false) when denied. Err(()) means the turn was cancelled.
async fn gate_tool(
    request_id: &str,
    session: &mut Session,
    action_mode: &ActionMode,
    call: &ToolCall,
    messages: &mut Vec<ToolChatMessage>,
    sink: &Arc<dyn AgentEventSink>,
    pending_approvals: &Arc<DashMap<String, oneshot::Sender<bool>>>,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<bool, ()> {
    if !requires_approval(action_mode, &call.name) {
        return Ok(true);
    }

    let approval_id = Uuid::new_v4().to_string();
    let (approval_tx, approval_rx) = oneshot::channel();
    let _guard = PendingApprovalGuard {
        pending: Arc::clone(pending_approvals),
        request_id: request_id.to_string(),
    };
    pending_approvals.insert(request_id.to_string(), approval_tx);

    sink.emit(
        "agent-approval-request",
        json!(ApprovalRequestEvent {
            request_id,
            approval_id: approval_id.clone(),
            name: call.name.clone(),
            summary: format!(
                "Allow tool call `{}` with arguments {}",
                call.name, call.arguments
            ),
        }),
    );

    let allowed = tokio::select! {
        biased;
        _ = cancel_rx.changed() => {
            if *cancel_rx.borrow() {
                return Err(());
            }
            false
        }
        r = approval_rx => r.unwrap_or(false),
    };

    sink.emit(
        "agent-approval-result",
        json!(ApprovalResultEvent {
            request_id,
            approval_id,
            allowed
        }),
    );

    if !allowed {
        let summary = format!("User denied the {} tool call", call.name);
        push_activity(session, &call.name, summary, ToolActivityStatus::Denied);
        // Feed the denial back as a tool result so the model can pick a
        // different approach instead of repeating the call.
        messages.push(ToolChatMessage {
            role: "tool".into(),
            content: format!(
                "User denied this tool call ({}). Choose a different approach.",
                call.name
            ),
            tool_call_id: Some(call.id.clone()),
            tool_calls: Vec::new(),
        });
    }
    Ok(allowed)
}

/// Execute one approved tool call, feed its result back to the model,
/// emit the result event and record the activity.
async fn execute_one(
    request_id: &str,
    session: &mut Session,
    workspace: &str,
    call: &ToolCall,
    messages: &mut Vec<ToolChatMessage>,
    sink: &Arc<dyn AgentEventSink>,
    stall_ring: &mut VecDeque<StallProbe>,
) {
    sink.emit(
        "agent-tool-start",
        json!(ToolStartEvent {
            request_id,
            call_id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        }),
    );

    let result = match execute_tool(workspace, &call.name, &call.arguments, true).await {
        Ok(r) => r,
        Err(e) => ToolResult {
            ok: false,
            summary: e,
            content: None,
        },
    };
    let summary = result.summary.clone();
    let status = if result.ok {
        ToolActivityStatus::Ran
    } else {
        ToolActivityStatus::Error
    };
    let status_name_str = status_name(&status);
    push_activity(session, &call.name, summary.clone(), status);
    persist(session).await;

    // The tool result is the model's context for the next round.
    let mut content = summary.clone();
    if let Some(detail) = &result.content {
        content.push('\n');
        content.push_str(detail);
    }
    messages.push(ToolChatMessage {
        role: "tool".into(),
        content,
        tool_call_id: Some(call.id.clone()),
        tool_calls: Vec::new(),
    });

    sink.emit(
        "agent-tool-result",
        json!(ToolResultEvent {
            request_id,
            call_id: call.id.clone(),
            name: call.name.clone(),
            ok: result.ok,
            summary,
            status: status_name_str,
        }),
    );

    stall_ring.push_back(StallProbe {
        name: call.name.clone(),
        args_json: call.arguments.to_string(),
        changed: result.ok && is_mutating(&call.name),
    });
    if stall_ring.len() > STALL_WINDOW {
        stall_ring.pop_front();
    }
}

fn status_name(s: &ToolActivityStatus) -> &'static str {
    match s {
        ToolActivityStatus::Ran => "ran",
        ToolActivityStatus::Denied => "denied",
        ToolActivityStatus::Error => "error",
    }
}

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockCompleter {
        script: Mutex<VecDeque<Result<TurnOutcome, AgentTurnError>>>,
        requests: Mutex<Vec<ToolChatRequest>>,
    }

    impl MockCompleter {
        fn new(script: Vec<Result<TurnOutcome, AgentTurnError>>) -> Self {
            Self {
                script: Mutex::new(script.into()),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn requests(&self) -> Vec<ToolChatRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl TurnCompleter for MockCompleter {
        fn complete(
            &self,
            request: ToolChatRequest,
            _tools: Vec<ToolSpec>,
            _text_cb: Box<dyn Fn(&str) + Send>,
            _cancel_rx: oneshot::Receiver<()>,
        ) -> Pin<Box<dyn Future<Output = Result<TurnOutcome, AgentTurnError>> + Send>> {
            self.requests.lock().unwrap().push(request.clone());
            let next = self.script.lock().unwrap().pop_front();
            Box::pin(async move {
                next.unwrap_or_else(|| {
                    Ok(TurnOutcome {
                        text: String::new(),
                        tool_calls: Vec::new(),
                        usage: ChatUsage {
                            input: 0,
                            output: 0,
                        },
                    })
                })
            })
        }
    }

    #[derive(Default)]
    struct MemSink {
        events: Mutex<Vec<(String, Value)>>,
    }

    impl AgentEventSink for MemSink {
        fn emit(&self, event: &str, payload: Value) {
            self.events
                .lock()
                .unwrap()
                .push((event.to_string(), payload));
        }
    }

    fn probe(sink: &MemSink, event: &str) -> Vec<Value> {
        sink.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, _)| name == event)
            .map(|(_, payload)| payload.clone())
            .collect()
    }

    fn sample_session() -> Session {
        Session {
            version: 2,
            id: Uuid::new_v4().to_string(),
            title: "t".into(),
            project_path: String::new(),
            created_at: now_iso(),
            updated_at: now_iso(),
            agent: crate::session::default_agent_snapshot(),
            goal_mode: false,
            action_mode: ActionMode::AskRisky,
            messages: Vec::new(),
            activities: Vec::new(),
            token_stats: None,
        }
    }

    struct Rig {
        request_id: String,
        session: Session,
        sink: Arc<MemSink>,
        pending: Arc<DashMap<String, oneshot::Sender<bool>>>,
        in_flight: Arc<DashMap<String, watch::Sender<bool>>>,
        // Kept alive for the duration of the run so tools see a real dir.
        _tmp: tempfile::TempDir,
    }

    fn rig() -> Rig {
        let tmp = tempfile::tempdir().unwrap();
        let mut session = sample_session();
        session.project_path = tmp.path().to_string_lossy().to_string();
        Rig {
            request_id: Uuid::new_v4().to_string(),
            session,
            sink: Arc::new(MemSink::default()),
            pending: Arc::new(DashMap::new()),
            in_flight: Arc::new(DashMap::new()),
            _tmp: tmp,
        }
    }

    async fn run(
        rig: &mut Rig,
        completer: Arc<MockCompleter>,
        message: &str,
        action_mode: ActionMode,
        goal_mode: bool,
    ) -> Session {
        let workspace = rig.session.project_path.clone();
        let sink: Arc<dyn AgentEventSink> = rig.sink.clone();
        let completer_dyn: Arc<dyn TurnCompleter> = completer.clone();
        let handle = run_agent_turn(
            rig.request_id.clone(),
            rig.session.clone(),
            "test-model".into(),
            message.to_string(),
            action_mode,
            goal_mode,
            workspace,
            sink,
            rig.pending.clone(),
            rig.in_flight.clone(),
            completer_dyn,
        );
        handle.await.unwrap();
        // Reload the persisted session: validates `persist_turn` too.
        crate::session::load_session(&rig.session.project_path, &rig.session.id)
            .await
            .unwrap()
            .unwrap()
    }

    fn text_outcome(text: &str, input: u64, output: u64) -> Result<TurnOutcome, AgentTurnError> {
        Ok(TurnOutcome {
            text: text.into(),
            tool_calls: Vec::new(),
            usage: ChatUsage { input, output },
        })
    }

    #[tokio::test]
    async fn goal_mode_continue_then_done() {
        let completer = Arc::new(MockCompleter::new(vec![
            text_outcome("part one [continue]", 1, 1),
            text_outcome("part two", 2, 2),
        ]));
        let mut rig = rig();
        let session = run(
            &mut rig,
            completer.clone(),
            "do the work",
            ActionMode::Auto,
            true,
        )
        .await;

        let done = probe(&rig.sink, "agent-done");
        assert_eq!(done.len(), 1);
        assert_eq!(done[0]["content"], "part two");
        // The persisted assistant text has the marker stripped.
        let assistant_msgs: Vec<_> = session
            .messages
            .iter()
            .filter(|m| m.role == "assistant")
            .collect();
        assert_eq!(assistant_msgs.len(), 2);
        assert_eq!(assistant_msgs[0].content, "part one");
        // The second request's last message is the [continue] user marker.
        let requests = completer.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].messages.last().unwrap().content, "[continue]");
    }

    #[tokio::test]
    async fn goal_mode_cap_errors() {
        let script = (0..=GOAL_MAX_TURNS)
            .map(|i| text_outcome("more [continue]", 1, i as u64))
            .collect();
        let completer = Arc::new(MockCompleter::new(script));
        let mut rig = rig();
        run(&mut rig, completer, "keep going", ActionMode::Auto, true).await;

        let errors = probe(&rig.sink, "agent-error");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["kind"], "loop_cap");
    }

    #[tokio::test]
    async fn tool_calls_execute_and_loop() {
        let completer = Arc::new(MockCompleter::new(vec![
            Ok(TurnOutcome {
                text: "checking".into(),
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    name: "list_dir".into(),
                    arguments: json!({"path": "."}),
                }],
                usage: ChatUsage {
                    input: 1,
                    output: 1,
                },
            }),
            text_outcome("done", 2, 2),
        ]));
        let mut rig = rig();
        let session = run(
            &mut rig,
            completer.clone(),
            "look around",
            ActionMode::Auto,
            false,
        )
        .await;

        let starts = probe(&rig.sink, "agent-tool-start");
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0]["name"], "list_dir");
        let results = probe(&rig.sink, "agent-tool-result");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["ok"], true);
        assert_eq!(probe(&rig.sink, "agent-done").len(), 1);
        // The activity record is persisted.
        assert_eq!(session.activities.len(), 1);
        assert_eq!(session.activities[0].status, ToolActivityStatus::Ran);
        // Second request carries the assistant echo + tool result.
        let requests = completer.requests();
        assert_eq!(requests.len(), 2);
        let roles: Vec<&str> = requests[1]
            .messages
            .iter()
            .map(|m| m.role.as_str())
            .collect();
        assert!(roles.windows(2).any(|w| w == ["assistant", "tool"]));
    }

    #[tokio::test]
    async fn risky_tool_requires_approval_and_deny_halts() {
        let completer = Arc::new(MockCompleter::new(vec![
            Ok(TurnOutcome {
                text: "writing".into(),
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    name: "write_file".into(),
                    arguments: json!({"path": "x.txt", "content": "hi"}),
                }],
                usage: ChatUsage {
                    input: 1,
                    output: 1,
                },
            }),
            text_outcome("ok", 2, 2),
        ]));
        let mut rig = rig();
        let request_id = rig.request_id.clone();

        // Deny the approval as soon as the loop registers it.
        let pending_watch = Arc::clone(&rig.pending);
        let responder = tokio::spawn(async move {
            loop {
                if pending_watch.contains_key(&request_id) {
                    let (_, tx) = pending_watch.remove(&request_id).unwrap();
                    let _ = tx.send(false);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });

        let session = run(
            &mut rig,
            completer.clone(),
            "write a file",
            ActionMode::AskRisky,
            false,
        )
        .await;
        let _ = responder.await;

        assert_eq!(probe(&rig.sink, "agent-approval-request").len(), 1);
        assert_eq!(probe(&rig.sink, "agent-tool-start").len(), 0);
        assert_eq!(probe(&rig.sink, "agent-done").len(), 1);
        // The denial is fed back as a tool message in the second request.
        let requests = completer.requests();
        let second = &requests[1].messages;
        assert!(second
            .iter()
            .any(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some("call-1")));
        // The denied activity is recorded and persisted.
        assert!(session
            .activities
            .iter()
            .any(|a| a.status == ToolActivityStatus::Denied));
    }

    #[tokio::test]
    async fn stall_detected() {
        let script = (0..3)
            .map(|_| {
                Ok(TurnOutcome {
                    text: "reading again".into(),
                    tool_calls: vec![ToolCall {
                        id: "call-1".into(),
                        name: "read_file".into(),
                        arguments: json!({"path": "a.txt"}),
                    }],
                    usage: ChatUsage {
                        input: 1,
                        output: 1,
                    },
                })
            })
            .collect();
        let completer = Arc::new(MockCompleter::new(script));
        let mut rig = rig();
        run(
            &mut rig,
            completer,
            "read the file",
            ActionMode::Auto,
            false,
        )
        .await;

        let errors = probe(&rig.sink, "agent-error");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["kind"], "stalled");
    }

    #[tokio::test]
    async fn cancellation_mid_approval_resolves_denied() {
        let completer = Arc::new(MockCompleter::new(vec![Ok(TurnOutcome {
            text: "writing".into(),
            tool_calls: vec![ToolCall {
                id: "call-1".into(),
                name: "write_file".into(),
                arguments: json!({"path": "x.txt", "content": "hi"}),
            }],
            usage: ChatUsage {
                input: 1,
                output: 1,
            },
        })]));
        let mut rig = rig();
        let request_id = rig.request_id.clone();

        // Cancel the turn once the loop is registered in `in_flight`.
        let in_flight_watch = Arc::clone(&rig.in_flight);
        let canceller = tokio::spawn(async move {
            loop {
                if in_flight_watch.contains_key(&request_id) {
                    if let Some((_, tx)) = in_flight_watch.remove(&request_id) {
                        let _ = tx.send(true);
                    }
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });

        run(
            &mut rig,
            completer,
            "write a file",
            ActionMode::AskAll,
            false,
        )
        .await;
        let _ = canceller.await;

        let errors = probe(&rig.sink, "agent-error");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["kind"], "cancelled");
        // The guard resolved the pending approval as denied and removed it.
        assert!(rig.pending.is_empty());
    }
}

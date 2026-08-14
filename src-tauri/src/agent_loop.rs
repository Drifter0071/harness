//! Rust-owned agent turn loop (spec Â§4, Â§5).
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

use crate::agent::AgentRole;
use crate::llm_client::{
    complete_with_tools, AgentTurnError, ChatUsage, ToolCall, ToolChatMessage, ToolChatRequest,
    ToolSpec, TurnOutcome,
};
use crate::models::ReasoningEffort;
use crate::personalities::AgentPersonality;
use crate::secrets::SecretStore;
use crate::session::{
    save_session, CommandMode, LastTurn, Message, Session, TokenStats, ToolActivity,
    ToolActivityStatus,
};
use crate::tools::{execute_tool, SubagentDispatchContext, ToolContext, ToolResult};

/// Hard cap on LLM rounds inside one turn (each round may execute tools).
const MAX_ROUNDS: usize = 24;
/// Consecutive identical tool calls with no successful mutation trip the
/// stall detector after this many rounds.
const STALL_WINDOW: usize = 3;
/// Tools that mutate the workspace; everything else is read-only for stall
/// detection and treated as "safe" under `Commands` mode.
const MUTATING_TOOLS: [&str; 3] = ["write_file", "edit_file", "run_command"];
/// Goal-mode continuation marker the model appends to keep working.
const CONTINUE_MARKER: &str = "[continue]";
/// Cap on goal-mode continuation turns after the first.
const GOAL_MAX_TURNS: usize = 8;

/// Whether a turn's session mutations are written to disk. Chat turns
/// (`Real`) persist to `<project>/.harness/sessions/`; dispatched
/// subagent turns run against a scratch session and never touch disk —
/// their content is delivered to the user via streamed events and to
/// the orchestrator via the `delegate_subagent` tool result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistMode {
    Real,
    Scratch,
}

// -- Event wire types (camelCase, routed by `requestId` on the frontend) --

/// Metadata attached to `agent-turn-start` when the turn is a dispatched
/// subagent (requestId starts with `sub-`). Lets the canvas open a
/// labeled window without an extra registry round-trip.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentStartInfo {
    pub personality_id: String,
    pub name: String,
    pub role: AgentRole,
    pub task: String,
    /// Saved-model id the subagent turn is about to run against.
    /// Surfaced in the window header so the user can see which model
    /// the agent is using.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnStartEvent<'a> {
    request_id: &'a str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subagent: Option<SubagentStartInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChunkEvent<'a> {
    request_id: &'a str,
    delta: &'a str,
}

/// Chain-of-thought delta. Emitted alongside `agent-chunk` for
/// thinking-capable models; the frontend buffers these into a hidden
/// collapsible "Thinking" block on the assistant message.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReasoningEvent<'a> {
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
    /// Full chain-of-thought captured for thinking-capable models.
    #[serde(default, skip_serializing_if = "str::is_empty")]
    reasoning: &'a str,
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
/// object-safe without pulling in async-trait. `text_cb` receives the
/// visible answer deltas; `reasoning_cb` receives chain-of-thought
/// deltas (no-op for models that don't emit reasoning).
pub trait TurnCompleter: Send + Sync {
    #[allow(clippy::type_complexity)]
    fn complete(
        &self,
        request: ToolChatRequest,
        tools: Vec<ToolSpec>,
        text_cb: Box<dyn Fn(&str) + Send>,
        reasoning_cb: Box<dyn Fn(&str) + Send>,
        cancel_rx: oneshot::Receiver<()>,
    ) -> Pin<Box<dyn Future<Output = Result<TurnOutcome, AgentTurnError>> + Send>>;

    /// Single-shot LLM call for compaction: system prompt + user prompt, no
    /// tools, no streaming, returns the raw text response (spec §6.7.5).
    fn complete_with_system_prompt(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        model_id: &str,
        cancel_rx: oneshot::Receiver<()>,
    ) -> Pin<Box<dyn Future<Output = Result<String, AgentTurnError>> + Send>>;
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
        reasoning_cb: Box<dyn Fn(&str) + Send>,
        cancel_rx: oneshot::Receiver<()>,
    ) -> Pin<Box<dyn Future<Output = Result<TurnOutcome, AgentTurnError>> + Send>> {
        let secrets = Arc::clone(&self.secrets);
        Box::pin(async move {
            complete_with_tools(
                &request,
                &tools,
                secrets.as_ref(),
                text_cb,
                reasoning_cb,
                cancel_rx,
            )
            .await
        })
    }

    fn complete_with_system_prompt(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        model_id: &str,
        cancel_rx: oneshot::Receiver<()>,
    ) -> Pin<Box<dyn Future<Output = Result<String, AgentTurnError>> + Send>> {
        let secrets = Arc::clone(&self.secrets);
        // Clone strings so the async block owns the data and the returned
        // future is valid for 'static (no borrowed references).
        let system_prompt = system_prompt.to_string();
        let user_prompt = user_prompt.to_string();
        let model_id = model_id.to_string();
        Box::pin(async move {
            crate::llm_client::complete_simple(
                &system_prompt,
                &user_prompt,
                &model_id,
                secrets.as_ref(),
                cancel_rx,
            )
            .await
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
/// `wait_for()` at every await. Watch receivers are "sticky" â€” a cancel sent
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

fn requires_approval(mode: &CommandMode, name: &str) -> bool {
    match mode {
        CommandMode::Commands => name == "run_command",
        CommandMode::Yolo => false,
    }
}

/// True when a tool mutates the workspace; used for stall detection.
fn is_mutating(name: &str) -> bool {
    MUTATING_TOOLS.contains(&name)
}

// -- History conversion --

/// Rebuild the provider history from persisted session messages. Tool-granular
/// `role:"tool"` records are not persisted (they live in `activities`), so a
/// fresh turn replays user/assistant text only â€" the assistant summary carries
/// the state across turns. Within a turn, the loop keeps the full
/// assistant-echo + tool-result sequence in `messages` so OpenAI accepts
/// multi-round tool use.
///
/// Summary messages (`role == "system" && kind == "summary"`) are included so
/// the agent can see the pre-compacted context after a `/compact` operation.
fn history_from_session(session: &Session) -> Vec<ToolChatMessage> {
    session
        .messages
        .iter()
        .filter(|m| {
            m.role == "user"
                || m.role == "assistant"
                || (m.role == "system" && m.kind.as_deref() == Some("summary"))
        })
        .map(|m| ToolChatMessage {
            role: m.role.clone(),
            content: m.content.clone(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        })
        .collect()
}

fn build_system_prompt(orchestrator_prompt: &str, workspace: &str, goal_mode: bool) -> String {
    let mut prompt = orchestrator_prompt.to_string();
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
    resolved_model_id: &str,
    system_prompt: String,
    _workspace: &str,
    _goal_mode: bool,
    messages: &[ToolChatMessage],
    reasoning_effort: Option<ReasoningEffort>,
) -> ToolChatRequest {
    ToolChatRequest {
        model_id: resolved_model_id.to_string(),
        system_prompt,
        messages: messages.to_vec(),
        reasoning_effort,
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
    // `cumulativeIn` is the peak input size, not a sum (spec Â§4.5).
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
        reasoning: None,
        kind: None,
        compact_meta: None,
    });
}

/// Append a final assistant message to the session using a caller-supplied
/// id. The agent loop passes the request-id-based placeholder id
/// (`agent-<requestId>`) that the frontend pre-inserts into the store, so
/// the persisted message and the in-memory placeholder stay the same
/// row. Without this, the assistant text lands in a fresh-UUID message
/// while the placeholder stays empty in the session file â€” which is
/// Persist the final assistant message for the turn. This is an
/// UPSERT, not an append: multi-round turns previously pushed one
/// message per round under the same `agent-<requestId>` id, which
/// persisted a stack of blank assistant rows for tool rounds (each
/// round's empty or interim text) and made reloaded sessions show
/// empty bubbles. Replacing the row keeps exactly one assistant
/// message per turn â€” the final answer.
///
/// `resolved` is the personality bundle computed at the start of
/// the turn; we use its `personality_id` + `name` (not the session
/// snapshot) because those are the values the user sees in the
/// chat header.
fn assistant_message_with_id_and_reasoning(
    session: &mut Session,
    text: &str,
    id: String,
    reasoning: Option<String>,
    resolved: &ResolvedAgent,
) {
    let reasoning = reasoning.filter(|r| !r.is_empty());
    let msg = Message {
        id: id.clone(),
        role: "assistant".into(),
        content: text.to_string(),
        timestamp: now_iso(),
        mentions: Vec::new(),
        agent_id: Some(resolved.personality_id.clone()),
        agent_name: Some(resolved.name.clone()),
        reasoning,
        kind: None,
        compact_meta: None,
    };
    if let Some(existing) = session.messages.iter_mut().find(|m| m.id == id) {
        *existing = msg;
    } else {
        session.messages.push(msg);
    }
}

fn push_activity(
    session: &mut Session,
    tool: &str,
    summary: String,
    status: ToolActivityStatus,
    request_id: Option<&str>,
    arguments: Option<&Value>,
) {
    session.activities.push(ToolActivity {
        id: Uuid::new_v4().to_string(),
        tool: tool.to_string(),
        summary,
        status,
        timestamp: now_iso(),
        request_id: request_id.map(|s| s.to_string()),
        arguments: arguments.cloned(),
    });
}

/// Persist the session after any mutation. `save_session` forces version 2,
/// rewrites `updatedAt` and auto-titles; failures are logged, not fatal to
/// the in-memory turn. Scratch turns (subagents) skip disk entirely.
async fn persist(session: &mut Session, mode: PersistMode) {
    if mode == PersistMode::Scratch {
        return;
    }
    if let Err(e) = save_session(&session.project_path, session.clone()).await {
        eprintln!(
            "agent_loop: failed to persist session {}: {}",
            session.id, e
        );
    }
}

/// Stamp the `lastTurn` marker onto the session and persist. Called on
/// every terminal transition (done / error) so the session file becomes a
/// self-describing record of "the turn with this requestId ended in this
/// status at this time" â€” independent of the in-process Tauri event bus.
///
/// The frontend uses this as a disk-based completion signal when
/// streaming events don't reach the WebView, so responses still appear
/// without a manual refresh.
async fn mark_turn_complete(
    session: &mut Session,
    request_id: &str,
    status: &str,
    mode: PersistMode,
) {
    mark_turn_complete_with_reasoning(session, request_id, status, None, mode).await;
}

async fn mark_turn_complete_with_reasoning(
    session: &mut Session,
    request_id: &str,
    status: &str,
    reasoning: Option<String>,
    mode: PersistMode,
) {
    session.last_turn = Some(LastTurn {
        request_id: request_id.to_string(),
        status: status.to_string(),
        at: now_iso(),
        reasoning: reasoning.filter(|r| !r.is_empty()),
    });
    persist(session, mode).await;
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
    mode: CommandMode,
    goal_mode: bool,
    workspace: String,
    sink: Arc<dyn AgentEventSink>,
    pending_approvals: Arc<DashMap<String, oneshot::Sender<bool>>>,
    in_flight: Arc<DashMap<String, watch::Sender<bool>>>,
    completer: Arc<dyn TurnCompleter>,
    reasoning_effort: Option<ReasoningEffort>,
    dispatch: Option<SubagentDispatchContext>,
) -> JoinHandle<()> {
    tokio::spawn(run_agent_turn_inner(
        request_id,
        session,
        model_id,
        message_text,
        mode,
        goal_mode,
        workspace,
        sink,
        pending_approvals,
        in_flight,
        completer,
        reasoning_effort,
        dispatch,
    ))
}

/// How a turn ended, after the final message is persisted.
pub enum TurnFinish {
    Done {
        content: String,
        reasoning: String,
        usage: ChatUsage,
    },
    Error {
        kind: String,
        message: String,
    },
}

/// Resolved turn-agent bundle. Every chat turn runs the Orchestrator
/// (built from the code constant via `orchestrator()`); dispatched
/// subagent turns build one from a roster personality via
/// `from_personality`. The bundle carries the stable id / name used to
/// label assistant messages, the role (which gates role-restricted
/// tools like `bip` and `delegate_subagent`), the system prompt, and
/// the model id the turn runs against.
#[derive(Debug, Clone)]
pub struct ResolvedAgent {
    /// Stable id used to label messages and session summary.
    personality_id: String,
    /// Human-readable label for the agent.
    name: String,
    /// The agent's role â€” threaded into the tool context so
    /// role-gated tools can check the caller.
    role: AgentRole,
    /// The agent's system prompt (code constant for the Orchestrator,
    /// roster prompt for subagents).
    system_prompt: String,
    /// The model id this turn runs against.
    model_id: String,
}

impl ResolvedAgent {
    /// The chat agent bundle: always the Orchestrator with its code
    /// constant prompt and the frontend-picked chat model.
    pub fn orchestrator(chat_model_id: &str) -> Self {
        Self {
            personality_id: crate::session::ORCHESTRATOR_AGENT_ID.to_string(),
            name: "Orchestrator".to_string(),
            role: AgentRole::Orchestrator,
            system_prompt: crate::agent::ORCHESTRATOR_PROMPT.to_string(),
            model_id: chat_model_id.to_string(),
        }
    }

    /// Build the bundle for a dispatched subagent from a roster
    /// personality. `resolved_model_id` is the personality's bound
    /// model when it exists, else the orchestrator's chat model.
    pub fn from_personality(personality: &AgentPersonality, resolved_model_id: &str) -> Self {
        Self {
            personality_id: personality.id.clone(),
            name: personality.name.clone(),
            role: personality.role,
            system_prompt: if personality.system_prompt.trim().is_empty() {
                personality.role.default_prompt().to_string()
            } else {
                personality.system_prompt.clone()
            },
            model_id: resolved_model_id.to_string(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_agent_turn_inner(
    request_id: String,
    mut session: Session,
    chat_model_id: String,
    message_text: String,
    mode: CommandMode,
    goal_mode: bool,
    workspace: String,
    sink: Arc<dyn AgentEventSink>,
    pending_approvals: Arc<DashMap<String, oneshot::Sender<bool>>>,
    in_flight: Arc<DashMap<String, watch::Sender<bool>>>,
    completer: Arc<dyn TurnCompleter>,
    reasoning_effort: Option<ReasoningEffort>,
    dispatch: Option<SubagentDispatchContext>,
) {
    // The chat agent is always the Orchestrator: code-constant prompt,
    // frontend-picked chat model, no roster row. Subagent personalities
    // (Coder, Architect, Researcher, HealthMonitor) are dispatched via
    // the `delegate_subagent` tool, which runs its own subagent turn.
    let resolved = ResolvedAgent::orchestrator(&chat_model_id);

    let outcome = run_turn_core(
        &request_id,
        &mut session,
        &resolved,
        &message_text,
        &mode,
        goal_mode,
        &workspace,
        &sink,
        &pending_approvals,
        &in_flight,
        completer.as_ref(),
        reasoning_effort,
        PersistMode::Real,
        dispatch.as_ref(),
    )
    .await;

    // Turn finished (success, error or cancellation): deregister, then
    // emit the terminal event.
    let _ = in_flight.remove(&request_id);
    finish_and_emit(&request_id, &mut session, &sink, outcome, PersistMode::Real).await;
}

/// Emit the terminal event for a finished turn and return the outcome
/// for callers that need the final content (the subagent dispatcher).
/// The cancelled branch stamps the disk marker when the mode is `Real`
/// so the session file still carries a completion signal.
async fn finish_and_emit(
    request_id: &str,
    session: &mut Session,
    sink: &Arc<dyn AgentEventSink>,
    outcome: Result<TurnFinish, ()>,
    mode: PersistMode,
) -> TurnFinish {
    match outcome {
        Ok(TurnFinish::Done {
            content,
            reasoning,
            usage,
        }) => {
            sink.emit(
                "agent-done",
                json!(DoneEvent {
                    request_id,
                    content: &content,
                    reasoning: &reasoning,
                    usage: usage.clone()
                }),
            );
            TurnFinish::Done {
                content,
                reasoning,
                usage,
            }
        }
        Ok(TurnFinish::Error { kind, message }) => {
            sink.emit(
                "agent-error",
                json!(ErrorEvent {
                    request_id,
                    kind: &kind,
                    message: &message
                }),
            );
            TurnFinish::Error { kind, message }
        }
        Err(()) => {
            // Cancellation happened before the loop's `mark_turn_complete`
            // ran. Stamp the marker so the session file still carries a
            // completion signal and the frontend can clear the placeholder.
            session.last_turn = Some(LastTurn {
                request_id: request_id.to_string(),
                status: "cancelled".to_string(),
                at: now_iso(),
                reasoning: None,
            });
            if mode == PersistMode::Real {
                let _ = save_session(&session.project_path, session.clone()).await;
            }
            sink.emit(
                "agent-error",
                json!(ErrorEvent {
                    request_id,
                    kind: "cancelled",
                    message: "Agent turn cancelled",
                }),
            );
            TurnFinish::Error {
                kind: "cancelled".to_string(),
                message: "Agent turn cancelled".to_string(),
            }
        }
    }
}

/// Kick off one dispatched subagent turn. The turn runs against a
/// scratch (non-persisted) session, streams the standard `agent-*`
/// events under its `sub-<uuid>` request id, and resolves to the final
/// `TurnFinish` so the `delegate_subagent` tool executor can update the
/// registry and hand the answer back to the orchestrator.
#[allow(clippy::too_many_arguments)]
pub fn run_subagent_turn(
    request_id: String,
    resolved: ResolvedAgent,
    task: String,
    mode: CommandMode,
    workspace: String,
    sink: Arc<dyn AgentEventSink>,
    pending_approvals: Arc<DashMap<String, oneshot::Sender<bool>>>,
    in_flight: Arc<DashMap<String, watch::Sender<bool>>>,
    completer: Arc<dyn TurnCompleter>,
    reasoning_effort: Option<ReasoningEffort>,
    dispatch: Option<SubagentDispatchContext>,
) -> JoinHandle<TurnFinish> {
    tokio::spawn(run_subagent_turn_inner(
        request_id,
        resolved,
        task,
        mode,
        workspace,
        sink,
        pending_approvals,
        in_flight,
        completer,
        reasoning_effort,
        dispatch,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn run_subagent_turn_inner(
    request_id: String,
    resolved: ResolvedAgent,
    task: String,
    mode: CommandMode,
    workspace: String,
    sink: Arc<dyn AgentEventSink>,
    pending_approvals: Arc<DashMap<String, oneshot::Sender<bool>>>,
    in_flight: Arc<DashMap<String, watch::Sender<bool>>>,
    completer: Arc<dyn TurnCompleter>,
    reasoning_effort: Option<ReasoningEffort>,
    dispatch: Option<SubagentDispatchContext>,
) -> TurnFinish {
    // Scratch session: subagent turns never touch the session store.
    // The orchestrator's session only records the `delegate_subagent`
    // tool call + result; the subagent's own content lives in the
    // streamed events and the final tool result.
    let mut session = Session {
        version: 2,
        id: request_id.clone(),
        title: resolved.name.clone(),
        project_path: workspace.clone(),
        created_at: now_iso(),
        updated_at: now_iso(),
        agent: crate::session::default_agent_snapshot(),
        goal_mode: false,
        last_mode: mode.clone(),
        last_model_id: None,
        action_mode_legacy: None,
        messages: Vec::new(),
        activities: Vec::new(),
        token_stats: None,
        last_turn: None,
        compact: None,
        compact_threshold: 0.8,
    };

    let outcome = run_turn_core(
        &request_id,
        &mut session,
        &resolved,
        &task,
        &mode,
        false,
        &workspace,
        &sink,
        &pending_approvals,
        &in_flight,
        completer.as_ref(),
        reasoning_effort,
        PersistMode::Scratch,
        dispatch.as_ref(),
    )
    .await;

    let _ = in_flight.remove(&request_id);
    finish_and_emit(
        &request_id,
        &mut session,
        &sink,
        outcome,
        PersistMode::Scratch,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_turn_core(
    request_id: &str,
    session: &mut Session,
    resolved: &ResolvedAgent,
    message_text: &str,
    mode: &CommandMode,
    goal_mode: bool,
    workspace: &str,
    sink: &Arc<dyn AgentEventSink>,
    pending_approvals: &Arc<DashMap<String, oneshot::Sender<bool>>>,
    in_flight: &Arc<DashMap<String, watch::Sender<bool>>>,
    completer: &dyn TurnCompleter,
    reasoning_effort: Option<ReasoningEffort>,
    persist_mode: PersistMode,
    dispatch: Option<&SubagentDispatchContext>,
) -> Result<TurnFinish, ()> {
    let (cancel_tx, mut cancel_rx) = new_cancel_token();
    in_flight.insert(request_id.to_string(), cancel_tx);

    // Clear any stale `lastTurn` marker from a previous turn. The watcher
    // re-reads the session on every persist; without this, a re-read
    // during the new turn would see the old `done` and apply a stale
    // terminal event.
    if session.last_turn.is_some() {
        session.last_turn = None;
        persist(session, persist_mode).await;
    }

    // Subagent turns (anything but the Orchestrator) attach their
    // identity + task to the turn-start event so the canvas can open a
    // labeled window without a registry round-trip.
    let subagent = if resolved.role == AgentRole::Orchestrator {
        None
    } else {
        Some(SubagentStartInfo {
            personality_id: resolved.personality_id.clone(),
            name: resolved.name.clone(),
            role: resolved.role,
            task: message_text.to_string(),
            model_id: Some(resolved.model_id.clone()),
        })
    };
    sink.emit(
        "agent-turn-start",
        json!(TurnStartEvent {
            request_id,
            subagent
        }),
    );

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
    // Subagents get a reduced toolset: no `delegate_subagent` (prevents
    // unbounded recursion — only the Orchestrator may spawn) and no
    // role-gated meta-tools that aren't theirs.
    let tools = if resolved.role == AgentRole::Orchestrator {
        ToolSpec::all()
    } else {
        ToolSpec::subagent()
    };

    loop {
        // Drain any messages the Health Agent pushed into this turn's
        // inbox via the `subagent_message` tool. They enter the
        // conversation as `[Health Monitor]` user messages so the
        // subagent can react to them. Drained = removed, so they are
        // fed exactly once even across multiple LLM rounds.
        if let Some(dispatch) = dispatch {
            if let Some(entry) = dispatch.inbox.get(request_id) {
                let mut guard = entry.value().lock().await;
                let drained: Vec<String> = guard.drain(..).collect();
                for msg in drained {
                    let text = format!("[Health Monitor] {msg}");
                    user_message(session, &text);
                    messages.push(ToolChatMessage {
                        role: "user".into(),
                        content: text,
                        tool_call_id: None,
                        tool_calls: Vec::new(),
                    });
                }
            }
        }

        if llm_rounds >= MAX_ROUNDS {
            return Ok(finish_error(
                session,
                "loop_cap",
                "Agent turn exceeded the maximum number of LLM rounds",
                Some(request_id),
                persist_mode,
            )
            .await);
        }

        // -- Provider call, raced against cancellation --
        let system_prompt = build_system_prompt(&resolved.system_prompt, workspace, goal_mode);
        let request = build_request(
            &resolved.model_id,
            system_prompt,
            workspace,
            goal_mode,
            &messages,
            reasoning_effort,
        );
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
        let reasoning_cb: Box<dyn Fn(&str) + Send> = {
            let sink = Arc::clone(sink);
            let request_id = request_id.to_string();
            Box::new(move |delta: &str| {
                sink.emit(
                    "agent-reasoning",
                    json!(ReasoningEvent {
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
        let llm_fut =
            completer.complete(request, tools.clone(), text_cb, reasoning_cb, cancel_pass);
        let outcome = tokio::select! {
            biased;
            _ = cancel_fut => {
                return Err(());
            }
            r = llm_fut => match r {
                Ok(o) => o,
                Err(e) => {
                    let (kind, msg) = error_vocab(&e);
                    return Ok(finish_error(session, kind, msg, Some(request_id), persist_mode).await);
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
        // Use the request-id-prefixed placeholder id so the persisted
        // assistant message matches the one the frontend already inserted
        // in its in-memory store. Without this they are two different
        // rows and the user's session file shows an empty assistant
        // message until a reload.
        let reasoning = (!outcome.reasoning.is_empty()).then(|| outcome.reasoning.clone());
        assistant_message_with_id_and_reasoning(
            session,
            &final_text,
            format!("agent-{request_id}"),
            reasoning,
            resolved,
        );
        merge_usage(&mut session.token_stats, &outcome.usage);
        persist(session, persist_mode).await;

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
                    mode, // &CommandMode (the shadowed PersistMode is named persist_mode)
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
                    resolved,
                    call,
                    &mut messages,
                    sink,
                    &mut stall_ring,
                    persist_mode,
                    dispatch,
                )
                .await;
            }
            if let Some(name) = detect_stall(&stall_ring) {
                return Ok(finish_error(
                    session,
                    "stalled",
                    format!("Agent repeated the same {} call without effect", name),
                    Some(request_id),
                    persist_mode,
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
                    Some(request_id),
                    persist_mode,
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

        // Terminal: stamp the disk marker before returning so the session
        // file reflects completion even if the in-process Tauri event never
        // reaches the WebView.
        let reasoning = (!outcome.reasoning.is_empty()).then(|| outcome.reasoning.clone());
        mark_turn_complete_with_reasoning(session, request_id, "done", reasoning, persist_mode).await;
        return Ok(TurnFinish::Done {
            content: final_text,
            reasoning: outcome.reasoning,
            usage: outcome.usage,
        });
    }
}

fn error_vocab(e: &AgentTurnError) -> (&'static str, String) {
    match e {
        AgentTurnError::Auth(m) => ("auth", m.clone()),
        AgentTurnError::RateLimited(_) => (
            "rate_limited",
            "Provider rate limit hit â€” try again shortly".into(),
        ),
        AgentTurnError::Network(m) => ("network", m.clone()),
        AgentTurnError::Provider(m) => ("provider", m.clone()),
        AgentTurnError::BadRequest(m) => ("bad_request", m.clone()),
        AgentTurnError::Cancelled => ("cancelled", "Agent turn cancelled".into()),
        AgentTurnError::Internal(m) => ("internal", m.clone()),
    }
}

/// Record a turn-terminating error as an activity and persist, so the UI can
/// show why a turn stopped even after the terminal event. Also stamps the
/// `lastTurn` marker with the request id (if provided) so the session file
/// carries the completion signal.
async fn finish_error(
    session: &mut Session,
    kind: &str,
    message: impl Into<String>,
    request_id: Option<&str>,
    mode: PersistMode,
) -> TurnFinish {
    let message = message.into();
    push_activity(
        session,
        "agent",
        format!("turn error ({}): {}", kind, message),
        ToolActivityStatus::Error,
        None,
        None,
    );
    if let Some(rid) = request_id {
        mark_turn_complete(session, rid, "error", mode).await;
    } else {
        persist(session, mode).await;
    }
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
    mode: &CommandMode,
    call: &ToolCall,
    messages: &mut Vec<ToolChatMessage>,
    sink: &Arc<dyn AgentEventSink>,
    pending_approvals: &Arc<DashMap<String, oneshot::Sender<bool>>>,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<bool, ()> {
    if !requires_approval(mode, &call.name) {
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
        push_activity(
            session,
            &call.name,
            summary,
            ToolActivityStatus::Denied,
            Some(request_id),
            Some(&call.arguments),
        );
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
#[allow(clippy::too_many_arguments)]
async fn execute_one(
    request_id: &str,
    session: &mut Session,
    workspace: &str,
    resolved: &ResolvedAgent,
    call: &ToolCall,
    messages: &mut Vec<ToolChatMessage>,
    sink: &Arc<dyn AgentEventSink>,
    stall_ring: &mut VecDeque<StallProbe>,
    mode: PersistMode,
    dispatch: Option<&SubagentDispatchContext>,
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

    let mut ctx = ToolContext::for_agent(workspace, resolved.role, sink.clone());
    if let Some(d) = dispatch {
        ctx = ctx.with_subagent(d.clone());
    }
    let result = match execute_tool(&ctx, &call.name, &call.arguments).await {
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
    push_activity(
        session,
        &call.name,
        summary.clone(),
        status,
        Some(request_id),
        Some(&call.arguments),
    );
    persist(session, mode).await;

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
            _reasoning_cb: Box<dyn Fn(&str) + Send>,
            _cancel_rx: oneshot::Receiver<()>,
        ) -> Pin<Box<dyn Future<Output = Result<TurnOutcome, AgentTurnError>> + Send>> {
            self.requests.lock().unwrap().push(request.clone());
            let next = self.script.lock().unwrap().pop_front();
            Box::pin(async move {
                next.unwrap_or_else(|| {
                    Ok(TurnOutcome {
                        text: String::new(),
                        reasoning: String::new(),
                        tool_calls: Vec::new(),
                        usage: ChatUsage {
                            input: 0,
                            output: 0,
                        },
                    })
                })
            })
        }

        fn complete_with_system_prompt(
            &self,
            _system_prompt: &str,
            _user_prompt: &str,
            _model_id: &str,
            _cancel_rx: oneshot::Receiver<()>,
        ) -> Pin<Box<dyn Future<Output = Result<String, AgentTurnError>> + Send>> {
            // Returns the canned summary from the script if set, else a default.
            let next = self.script.lock().unwrap().pop_front();
            Box::pin(async move {
                match next {
                    Some(Ok(outcome)) => Ok(outcome.text),
                    Some(Err(e)) => Err(e),
                    None => Ok("Summarized conversation.".to_string()),
                }
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
            last_mode: CommandMode::Commands,
            last_model_id: None,
            action_mode_legacy: None,
            messages: Vec::new(),
            activities: Vec::new(),
            token_stats: None,
            last_turn: None,
            compact: None,
            compact_threshold: 0.8,
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
        mode: CommandMode,
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
            mode,
            goal_mode,
            workspace,
            sink,
            rig.pending.clone(),
            rig.in_flight.clone(),
            completer_dyn,
            None,
            None,
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
            reasoning: String::new(),
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
            CommandMode::Yolo,
            true,
        )
        .await;

        let done = probe(&rig.sink, "agent-done");
        assert_eq!(done.len(), 1);
        assert_eq!(done[0]["content"], "part two");
        // The persisted assistant message is a single row (the loop
        // upserts per-turn messages instead of pushing one per round),
        // holding the final answer with the marker stripped.
        let assistant_msgs: Vec<_> = session
            .messages
            .iter()
            .filter(|m| m.role == "assistant")
            .collect();
        assert_eq!(assistant_msgs.len(), 1);
        assert_eq!(assistant_msgs[0].content, "part two");
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
        run(&mut rig, completer, "keep going", CommandMode::Yolo, true).await;

        let errors = probe(&rig.sink, "agent-error");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["kind"], "loop_cap");
    }

    #[tokio::test]
    async fn tool_calls_execute_and_loop() {
        let completer = Arc::new(MockCompleter::new(vec![
            Ok(TurnOutcome {
                text: "checking".into(),
                reasoning: String::new(),
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
            CommandMode::Yolo,
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
                reasoning: String::new(),
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
            CommandMode::Commands,
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
                    reasoning: String::new(),
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
            CommandMode::Yolo,
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
            reasoning: String::new(),
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
            CommandMode::Commands,
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

    fn coder_personality() -> AgentPersonality {
        AgentPersonality {
            id: "builtin-coder".into(),
            name: "Coder".into(),
            role: AgentRole::Coder,
            system_prompt: String::new(),
            model_id: None,
            builtin: true,
        }
    }

    /// A `SubagentDispatchContext` with everything the loop needs to run
    /// a subagent turn in-memory (no disk, no network).
    fn dispatch_rig(
        completer: &Arc<MockCompleter>,
        sink: &Arc<MemSink>,
        workspace: &str,
    ) -> SubagentDispatchContext {
        SubagentDispatchContext {
            subagents: Arc::new(DashMap::new()),
            inbox: Arc::new(DashMap::new()),
            completer: completer.clone(),
            pending_approvals: Arc::new(DashMap::new()),
            in_flight: Arc::new(DashMap::new()),
            sink: sink.clone(),
            workspace: workspace.to_string(),
            mode: crate::session::CommandMode::Yolo,
            reasoning_effort: None,
            chat_model_id: "test-model".into(),
            request_id: "parent-1".into(),
        }
    }

    #[tokio::test]
    async fn subagent_turn_runs_scratch_and_streams_start_info() {
        let completer = Arc::new(MockCompleter::new(vec![text_outcome("done", 1, 1)]));
        let sink = Arc::new(MemSink::default());
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_string_lossy().to_string();
        let dispatch = dispatch_rig(&completer, &sink, &workspace);
        let pending = dispatch.pending_approvals.clone();
        let in_flight = dispatch.in_flight.clone();

        let resolved = ResolvedAgent::from_personality(&coder_personality(), "test-model");
        let handle = run_subagent_turn(
            "sub-1".into(),
            resolved,
            "do the thing".into(),
            CommandMode::Yolo,
            workspace.clone(),
            sink.clone(),
            pending,
            in_flight,
            completer,
            None,
            Some(dispatch),
        );
        let finish = handle.await.unwrap();
        match finish {
            TurnFinish::Done { content, .. } => assert_eq!(content, "done"),
            _ => panic!("expected Done"),
        }

        // The turn-start event carried the subagent identity so the
        // canvas can open a labeled window without a registry round-trip.
        let starts = probe(&sink, "agent-turn-start");
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0]["subagent"]["name"], "Coder");
        assert_eq!(starts[0]["subagent"]["role"], "Coder");
        assert_eq!(starts[0]["subagent"]["task"], "do the thing");
        assert_eq!(starts[0]["requestId"], "sub-1");
        // The terminal event also fired under the sub- request id.
        assert_eq!(probe(&sink, "agent-done").len(), 1);

        // Scratch: no `.harness/sessions` directory was ever created.
        assert!(!tmp.path().join(".harness").exists());
    }

    #[tokio::test]
    async fn subagent_drains_health_inbox_into_conversation() {
        let completer = Arc::new(MockCompleter::new(vec![text_outcome("ok", 1, 1)]));
        let sink = Arc::new(MemSink::default());
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_string_lossy().to_string();
        let dispatch = dispatch_rig(&completer, &sink, &workspace);
        // Preload the subagent's inbox as the Health Agent would via the
        // `subagent_message` tool.
        dispatch.inbox.insert(
            "sub-1".into(),
            tokio::sync::Mutex::new(vec!["be careful".into()]),
        );

        let resolved = ResolvedAgent::from_personality(&coder_personality(), "test-model");
        let handle = run_subagent_turn(
            "sub-1".into(),
            resolved,
            "task".into(),
            CommandMode::Yolo,
            workspace,
            sink,
            dispatch.pending_approvals.clone(),
            dispatch.in_flight.clone(),
            completer.clone(),
            None,
            Some(dispatch),
        );
        handle.await.unwrap();

        // The provider request contains the health message as a user turn
        // (drained exactly once — the inbox is empty afterwards).
        let requests = completer.requests();
        assert_eq!(requests.len(), 1);
        let user_msgs: Vec<&str> = requests[0]
            .messages
            .iter()
            .filter(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .collect();
        assert!(user_msgs.contains(&"[Health Monitor] be careful"));
    }
}

//! Workspace-guarded tool registry for the agent loop.
//!
//! Every tool the agent can call is declared here with a JSON schema
//! (`ToolSpec::all`) and an executor (`execute_tool`). All filesystem
//! targets are forced inside the session workspace via `resolve_within`,
//! commands run with a hard timeout, and output is capped so the model
//! context never explodes.
//!
//! `bip` is the one meta-tool that doesn't manipulate the workspace — it
//! sets the on-screen agent mascot's mood and inner monologue. It is the
//! only tool whose access is gated by `caller_role`: only the Health Agent
//! (`AgentRole::HealthMonitor`) may call it. See `docs/superpowers/specs/2026-08-13--bip-mascot-tool.md`.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{oneshot, watch, Mutex as TokioMutex};
use uuid::Uuid;

use crate::agent::{AgentRole, SubagentRecord};
use crate::agent_loop::{AgentEventSink, TurnCompleter};
use crate::llm_client::ToolSpec;
use crate::models::ReasoningEffort;
use crate::personalities::{builtin_personality_id, get_personality};
use crate::session::{CommandMode, TodoList};
use crate::todos::{self as todo_mod, TodoChangeSource, TodoItemInput, TodoItemPatch};

/// Output cap for tool `content` (model context budget).
const CONTENT_MAX: usize = 32 * 1024;
/// Per-stream cap for command stdout/stderr.
const STREAM_MAX: usize = 16 * 1024;
/// Hard timeout for `run_command` (the child is killed on expiry).
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const SEARCH_MAX_DEPTH: usize = 8;

/// Prefix for dispatched-subagent request ids (`sub-<uuid>`). The
/// frontend routes `agent-*` events whose `requestId` starts with this
/// to the subagent window manager instead of the chat. Kept here (next
/// to the dispatch machinery) so `agent_loop` and `commands` share one
/// constant.
pub const SUBAGENT_REQUEST_PREFIX: &str = "sub-";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub ok: bool,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

// -- Bip meta-tool types ---------------------------------------------------
//
// `bip` is the only tool that does not touch the workspace. It is the on-
// screen agent mascot's "expression": the Health Agent calls it to set Bip's
// mood (drives the animation state) and an optional one-sentence thought that
// appears in Bip's popover. Rust is the source of truth; the UI subscribes
// to the `bip-state` Tauri event. See `docs/superpowers/specs/2026-08-13--bip-mascot-tool.md`.

/// Bip's current mood / animation state. Matches the four `MascotMood`
/// values used by the frontend's `AgentMascot` component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BipMood {
    Idle,
    Working,
    Success,
    Error,
}

/// Who set the current `BipState`. Today only the health agent drives it,
/// but the enum leaves room for a future `System` / `User` source without
/// breaking the wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BipSource {
    Health,
}

/// The full Bip state broadcast to the WebView on every change.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BipState {
    pub mood: BipMood,
    /// Short sentence shown in Bip's popover. Empty string means "use the
    /// frontend's per-mood default".
    #[serde(default)]
    pub thought: String,
    /// ISO-8601 UTC timestamp the state was set.
    pub at: String,
    pub source: BipSource,
}

impl Default for BipState {
    fn default() -> Self {
        Self {
            mood: BipMood::Idle,
            thought: String::new(),
            at: now_iso8601(),
            source: BipSource::Health,
        }
    }
}

/// Minimal ISO-8601 UTC timestamp ("YYYY-MM-DDTHH:MM:SSZ"). We avoid pulling
/// in `chrono` / `time` for a single field.
fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Civil-from-days adapted from Howard Hinnant's date.h (public domain).
    let z = (secs / 86400) as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let secs_of_day = (secs % 86400) as u32;
    let hh = secs_of_day / 3600;
    let mm = (secs_of_day % 3600) / 60;
    let ss = secs_of_day % 60;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, hh, mm, ss)
}

/// Everything the `delegate_subagent` / `subagent_message` tools need to
/// spawn and track a subagent turn from inside the agent loop. Built once
/// per chat turn in `commands::run_agent_turn` and threaded through the
/// `ToolContext`; `Clone` is cheap (every field is `Arc` or a `Copy`).
#[derive(Clone)]
pub struct SubagentDispatchContext {
    /// Registry of dispatched subagent turns keyed by their `sub-<uuid>`
    /// request id. Written by `delegate_subagent`, read by
    /// `list_subagents` and the WebView-reconnect resync.
    pub subagents: Arc<DashMap<String, SubagentRecord>>,
    /// Per-subagent message inbox keyed by request id. The Health Agent
    /// pushes `subagent_message` tool calls here; the subagent's loop
    /// drains it at the top of every LLM round.
    pub inbox: Arc<DashMap<String, TokioMutex<Vec<String>>>>,
    /// Completer used to run the subagent turn (same provider plumbing
    /// as the orchestrator turn).
    pub completer: Arc<dyn TurnCompleter>,
    pub pending_approvals: Arc<DashMap<String, oneshot::Sender<bool>>>,
    pub in_flight: Arc<DashMap<String, watch::Sender<bool>>>,
    /// Event sink for the subagent turn's streamed `agent-*` events.
    pub sink: Arc<dyn AgentEventSink>,
    /// The workspace the subagent operates in (same as the orchestrator).
    pub workspace: String,
    /// Approval policy the subagent turn runs under.
    pub mode: CommandMode,
    /// Reasoning effort forwarded to the subagent's provider call.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Fallback model id: used when the personality has no bound saved
    /// model (the orchestrator's chat model).
    pub chat_model_id: String,
    /// requestId of the orchestrator turn that owns this context. Used as
    /// the `parent_request_id` on every dispatched subagent record.
    pub request_id: String,
    /// Goal-mode continuation cap for this turn (spec
    /// `2026-08-14--parallel-subagent-dispatch`). Replaces the
    /// `GOAL_MAX_TURNS` code constant; the orchestrator and every
    /// dispatched subagent read it from the same context.
    pub goal_max_turns: usize,
    /// Cap on how many subagent turns the orchestrator may run at the
    /// same time. Spec
    /// `2026-08-14--parallel-subagent-dispatch`: built once per
    /// orchestrator turn from `Roster.max_concurrent_subagents` (default
    /// 4). Excess dispatches queue and start as permits free.
    pub subagent_semaphore: Arc<tokio::sync::Semaphore>,
    /// Transient map: orchestrator-side `call.id` → list of subagent
    /// `sub-<uuid>` request ids. Populated by `tool_delegate_subagent`
    /// (singular → 1 entry) and `tool_delegate_subagents` (plural → N
    /// entries, spec `2026-08-14--plural-subagent-dispatch`), consumed
    /// by `execute_tool_batch` to know which registry rows belong to
    /// which orchestrator call. The plural case is what lets one
    /// model-side tool call fan out into N concurrent subagent turns
    /// without the model having to emit N separate `tool_calls` in a
    /// single round — the orchestrator's batch dispatcher spawns one
    /// JoinSet task per subagent id and folds their results into one
    /// combined synthetic tool message keyed by the same call id.
    /// Cleared at the end of every batch.
    pub dispatch_call_index: Arc<DashMap<String, Vec<String>>>,
    /// Per-subagent cancel watchers (spec
    /// `2026-08-14--parallel-subagent-dispatch`). The frontend's
    /// "close a working subagent window" calls the existing
    /// `cancel_agent` IPC; each spawned subagent turn registers its
    /// own `sub-<uuid>` in the shared `agent_in_flight` map, so
    /// cancelling one id only aborts that one turn. This map is kept
    /// for symmetry and a future per-batch cancel-all button (e.g.
    /// "stop all in this orchestrator turn"); today the dispatcher
    /// ignores it and relies on the shared in-flight map.
    #[allow(dead_code)]
    pub subagent_cancel: Arc<DashMap<String, watch::Sender<bool>>>,
}

/// Inputs the agent loop passes to every tool call. Centralising these here
/// means the `bip` role-gate, the `run_command` policy gate, and the event
/// emission can all hang off the same place.
pub struct ToolContext {
    /// Absolute path to the active session's workspace root.
    pub workspace: String,
    /// Policy/approval verdict from the loop. `run_command` is the only
    /// tool that reads this; everything else ignores it.
    pub auto_allowed: bool,
    /// The role of the agent that initiated the call. `bip` rejects any
    /// caller that isn't `HealthMonitor`.
    pub caller_role: AgentRole,
    /// Where tool-emitted events (e.g. `bip-state`) are broadcast.
    pub sink: Arc<dyn AgentEventSink>,
    /// Present only on orchestrator turns: gives the dispatch tools
    /// access to the subagent registry, inboxes, and the machinery to
    /// run subagent turns. `None` on subagent turns (they can't spawn
    /// further subagents) and in unit tests that don't care.
    pub subagent: Option<SubagentDispatchContext>,
    /// Active session id. The todo tools (spec
    /// `2026-08-14--agent-todo-list.md`) need this to know which
    /// session's `TodoList` to mutate. `None` in unit-test contexts
    /// that don't carry a session — those tests don't exercise the
    /// todo tools directly.
    pub session_id: Option<String>,
}

impl ToolContext {
    /// Build a context for an arbitrary agent. `auto_allowed` defaults to
    /// `true` (the loop has already cleared the call by routing it here);
    /// tests that need a different value set it explicitly.
    pub fn for_agent(
        workspace: &str,
        caller_role: AgentRole,
        sink: Arc<dyn AgentEventSink>,
    ) -> Self {
        Self {
            workspace: workspace.to_string(),
            auto_allowed: true,
            caller_role,
            sink,
            subagent: None,
            session_id: None,
        }
    }

    /// Attach the subagent dispatch machinery (registry, inboxes, turn
    /// runner). The agent loop calls this for orchestrator turns.
    pub fn with_subagent(mut self, subagent: SubagentDispatchContext) -> Self {
        self.subagent = Some(subagent);
        self
    }

    /// Attach the active session id. The todo tools need this to
    /// know which session's `TodoList` to mutate. The agent loop
    /// sets it for every turn.
    pub fn with_session_id(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }
}

impl ToolSpec {
    /// All tools the harness agent can call this sprint.
    pub fn all() -> Vec<ToolSpec> {
        vec![
            ToolSpec {
                name: "read_file".into(),
                description: "Read a file as UTF-8 text. Returns up to 32 KB.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Workspace-relative or absolute path" }
                    },
                    "required": ["path"]
                }),
            },
            ToolSpec {
                name: "list_dir".into(),
                description: "List the entries of a directory (names only, one per line).".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Workspace-relative or absolute directory path" }
                    },
                    "required": ["path"]
                }),
            },
            ToolSpec {
                name: "search_files".into(),
                description: "Recursively find file paths whose name contains the pattern (case-insensitive, depth-capped).".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "root": { "type": "string", "description": "Directory to search from" },
                        "pattern": { "type": "string", "description": "Substring to match against file names" }
                    },
                    "required": ["root", "pattern"]
                }),
            },
            ToolSpec {
                name: "write_file".into(),
                description: "Create or overwrite a file with the given content. Creates parent directories.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }),
            },
            ToolSpec {
                name: "edit_file".into(),
                description: "Replace exactly one occurrence of an exact string in a file. Fails if the old string is absent or appears more than once.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "old": { "type": "string", "description": "Exact substring to replace" },
                        "new": { "type": "string", "description": "Replacement text" }
                    },
                    "required": ["path", "old", "new"]
                }),
            },
            ToolSpec {
                name: "run_command".into(),
                description: "Run a command in the workspace shell with an args array. 60 s timeout, output truncated to 16 KB per stream.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Executable name or path" },
                        "args": { "type": "array", "items": { "type": "string" }, "description": "Arguments" },
                        "cwd": { "type": "string", "description": "Optional working directory within the workspace" }
                    },
                    "required": ["command", "args"]
                }),
            },
            ToolSpec {
                name: "web_search".into(),
                description: "Search the web and return the top result titles, URLs and snippets. \
                              Routes the query to the best keyless backend for its intent: \
                              factual ('what is X', 'who is Y') → Wikipedia; \
                              code/library ('rust async tokio', 'how to use react hooks') → GitHub; \
                              academic ('arxiv paper on Z', 'study on W') → arXiv; \
                              everything else → DuckDuckGo. \
                              All backends are keyless; the tool may fall through to a different backend \
                              if the first returns no useful content. \
                              If the tool returns an error, do NOT retry the same query — refine the query or try a different tool."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    },
                    "required": ["query"]
                }),
            },
            ToolSpec {
                name: "bip".into(),
                description: "Set the on-screen agent mascot (Bip)'s mood and inner monologue. \
                              Only callable by the Health Agent. The UI listens for the \
                              `bip-state` event and updates Bip's animation + thoughts popover."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "mood": {
                            "type": "string",
                            "enum": ["idle", "working", "success", "error"],
                            "description": "Bip's mood — drives the animation state."
                        },
                        "thought": {
                            "type": "string",
                            "description": "Short sentence shown in Bip's popover. \
                                            Omit to fall back to the per-mood default thought."
                        }
                    },
                    "required": ["mood"]
                }),
            },
            ToolSpec {
                name: "delegate_subagent".into(),
                description: "Dispatch a task to a subagent and wait for its final answer. \
                              Only callable by the Orchestrator (the chat agent). The subagent \
                              runs in its own window with its own streaming progress. Pass the \
                              roster personality id (e.g. \"builtin-coder\") or a role name \
                              (\"coder\", \"architect\", \"researcher\", \"health-monitor\") as \
                              `personality`, and the full task as `task`. For 2+ independent \
                              tasks, prefer `delegate_subagents` (plural) so they fan out in \
                              one round."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "personality": {
                            "type": "string",
                            "description": "Roster personality id or role slug to run as"
                        },
                        "task": {
                            "type": "string",
                            "description": "The full task for the subagent"
                        }
                    },
                    "required": ["personality", "task"]
                }),
            },
            ToolSpec {
                // Plural form (spec `2026-08-14--plural-subagent-dispatch`).
                // One tool call, one list of `{personality, task}` entries,
                // N concurrent subagent turns under the same subagent
                // semaphore cap as the singular tool. Use this whenever
                // 2+ dispatches are independent — it forces the
                // orchestrator's parallel dispatcher to fire all of them
                // at once, instead of relying on the model to emit
                // multiple `delegate_subagent` calls in one round.
                name: "delegate_subagents".into(),
                description: "Dispatch 2 or more tasks to subagents in a single tool call. \
                              The Orchestrator's tool loop runs every entry in parallel under \
                              the subagent semaphore cap. Only callable by the Orchestrator. \
                              Pass an array `dispatches` of `{personality, task}` objects — \
                              same shape as the singular `delegate_subagent` arguments. \
                              Returns one combined `role: \"tool\"` message summarizing all of \
                              the dispatched subagent outcomes. Prefer this over multiple \
                              `delegate_subagent` calls whenever the dispatches are \
                              independent."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "dispatches": {
                            "type": "array",
                            "minItems": 1,
                            "maxItems": 16,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "personality": {
                                        "type": "string",
                                        "description": "Roster personality id or role slug \
                                                        to run as (same as the singular tool)"
                                    },
                                    "task": {
                                        "type": "string",
                                        "description": "The full task for this subagent"
                                    }
                                },
                                "required": ["personality", "task"]
                            }
                        }
                    },
                    "required": ["dispatches"]
                }),
            },
            ToolSpec {
                name: "subagent_message".into(),
                description: "Send a message to a running subagent (by its request id, e.g. \
                              \"sub-…\"). Only callable by the Health Agent. The message is \
                              delivered as a user message into the subagent's next LLM round. \
                              Get subagent request ids from `list_subagents`."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "request_id": {
                            "type": "string",
                            "description": "The sub-<uuid> request id of the target subagent"
                        },
                        "message": {
                            "type": "string",
                            "description": "The message to deliver"
                        }
                    },
                    "required": ["request_id", "message"]
                }),
            },
            // -- Todo list (spec 2026-08-14--agent-todo-list.md) --
            ToolSpec {
                name: "todo_set".into(),
                description: "Replace the entire todo list. Use ONLY when the current list is \
                              empty or all items are done (the 'finished' state). For active \
                              lists, use todo_append / todo_update instead. Each item: text \
                              (1..280 chars), optional status. The list is the user's window \
                              into your plan — keep items short and action-oriented."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "items": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "text": { "type": "string", "minLength": 1, "maxLength": 280 },
                                    "status": {
                                        "type": "string",
                                        "enum": ["pending", "in_progress", "done"]
                                    }
                                },
                                "required": ["text"]
                            }
                        }
                    },
                    "required": ["items"]
                }),
            },
            ToolSpec {
                name: "todo_append".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "items": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "text": { "type": "string", "minLength": 1, "maxLength": 280 },
                                    "status": {
                                        "type": "string",
                                        "enum": ["pending", "in_progress", "done"]
                                    }
                                },
                                "required": ["text"]
                            }
                        },
                        "position": {
                            "type": "integer",
                            "description": "0-based insert position; defaults to end. Items \
                                            past the end are treated as 'append at end'."
                        }
                    },
                    "required": ["items"]
                }),
                description: "Append one or more todo items to the current list. \
                              Use at the start of a non-trivial task to publish your plan. \
                              The list is the user's window into your work — keep items short \
                              and action-oriented.".into(),
            },
            ToolSpec {
                name: "todo_update".into(),
                description: "Update one or more todo items by id. Mark items as in_progress \
                              when you start them, done when you finish. Updates are live — \
                              the user sees them immediately."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "updates": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "text": { "type": "string", "maxLength": 280 },
                                    "status": {
                                        "type": "string",
                                        "enum": ["pending", "in_progress", "done"]
                                    },
                                    "order": { "type": "integer" }
                                },
                                "required": ["id"]
                            }
                        }
                    },
                    "required": ["updates"]
                }),
            },
            ToolSpec {
                name: "todo_remove".into(),
                description: "Remove todo items by id. Gated by the active action policy: in \
                              Commands mode the user must approve each call."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "ids": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["ids"]
                }),
            },
            ToolSpec {
                name: "todo_clear".into(),
                description: "Wipe the todo list. Gated by the active action policy: in \
                              Commands mode the user must approve each call."
                    .into(),
                parameters: json!({ "type": "object", "properties": {} }),
            },
        ]
    }

    /// The toolset a dispatched subagent runs with: the full set minus
    /// `delegate_subagent` (only the Orchestrator may spawn — prevents
    /// unbounded recursion) and minus the Health-only meta-tools' specs
    /// that make no sense as dispatch targets. Role gates in the
    /// executors remain the hard backstop.
    pub fn subagent() -> Vec<ToolSpec> {
        Self::all()
            .into_iter()
            .filter(|t| t.name != "delegate_subagent" && t.name != "delegate_subagents")
            .collect()
    }
}

/// Execute one tool call. The `ctx` carries everything a tool might need:
/// the workspace root, the policy/approval verdict (`auto_allowed`), the
/// caller's role (so role-gated tools like `bip` can refuse), and an event
/// sink for state broadcasts (`bip-state`).
///
/// Thin shim over [`execute_tool_with_id`] kept for backwards compat with
/// tests that don't carry a provider-assigned `call.id`. The agent loop
/// itself uses `execute_tool_with_id` so `delegate_subagent` can index
/// the subagent record by the model's call id.
#[allow(dead_code)]
pub async fn execute_tool(
    ctx: &ToolContext,
    name: &str,
    args: &Value,
) -> Result<ToolResult, String> {
    execute_tool_with_id(ctx, name, args, "").await
}

/// Like [`execute_tool`] but lets the caller pass the provider-assigned
/// `call.id` so role-gated tools that need to map call → side-effect
/// (e.g. `delegate_subagent` indexing a subagent record) can do so. The
/// id is currently ignored by every tool except `delegate_subagent`.
pub async fn execute_tool_with_id(
    ctx: &ToolContext,
    name: &str,
    args: &Value,
    call_id: &str,
) -> Result<ToolResult, String> {
    let workspace = ctx.workspace.as_str();
    match name {
        "read_file" => tool_read_file(workspace, args).await,
        "list_dir" => tool_list_dir(workspace, args).await,
        "search_files" => tool_search_files(workspace, args).await,
        "write_file" => tool_write_file(workspace, args).await,
        "edit_file" => tool_edit_file(workspace, args).await,
        "run_command" => {
            if !ctx.auto_allowed {
                return Err("run_command requires approval".into());
            }
            tool_run_command(workspace, args, COMMAND_TIMEOUT).await
        }
        "web_search" => tool_web_search(args).await,
        "bip" => tool_bip(ctx, args).await,
        "delegate_subagent" => tool_delegate_subagent(ctx, args, call_id).await,
        "delegate_subagents" => tool_delegate_subagents(ctx, args, call_id).await,
        "subagent_message" => tool_subagent_message(ctx, args).await,
        // -- Todo list (spec 2026-08-14--agent-todo-list.md) --
        // `todo_remove` and `todo_clear` are gated by `requires_approval`
        // upstream (see `agent_loop::requires_approval`); the gate runs
        // before the call reaches here, so by the time we're in the
        // dispatch arm the user has already approved. We still check
        // `ctx.auto_allowed` defensively for direct test paths.
        "todo_set" => {
            if !ctx.auto_allowed {
                return Err("todo_set requires approval".into());
            }
            tool_todo_set(ctx, args).await
        }
        "todo_append" => tool_todo_append(ctx, args).await,
        "todo_update" => tool_todo_update(ctx, args).await,
        "todo_remove" => {
            if !ctx.auto_allowed {
                return Err("todo_remove requires approval".into());
            }
            tool_todo_remove(ctx, args).await
        }
        "todo_clear" => {
            if !ctx.auto_allowed {
                return Err("todo_clear requires approval".into());
            }
            tool_todo_clear(ctx, args).await
        }
        other => Err(format!("Unknown tool: {}", other)),
    }
}

// -- Executors --

async fn tool_read_file(workspace: &str, args: &Value) -> Result<ToolResult, String> {
    let path = string_arg(args, "path")?;
    let resolved = resolve_within(workspace, &path)?;
    let content = tokio::fs::read_to_string(&resolved)
        .await
        .map_err(|e| format!("Failed to read {}: {}", path, e))?;
    Ok(ToolResult {
        ok: true,
        summary: format!("read {}", resolved.display()),
        content: Some(truncate(&content, CONTENT_MAX)),
    })
}

async fn tool_list_dir(workspace: &str, args: &Value) -> Result<ToolResult, String> {
    let path = string_arg(args, "path")?;
    let resolved = resolve_within(workspace, &path)?;
    let mut entries = tokio::fs::read_dir(&resolved)
        .await
        .map_err(|e| format!("Failed to read dir {}: {}", path, e))?;
    let mut names = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
        names.push(entry.file_name().to_string_lossy().to_string());
    }
    names.sort();
    Ok(ToolResult {
        ok: true,
        summary: format!("listed {} entries in {}", names.len(), resolved.display()),
        content: Some(truncate(&names.join("\n"), CONTENT_MAX)),
    })
}

async fn tool_search_files(workspace: &str, args: &Value) -> Result<ToolResult, String> {
    let root = string_arg(args, "root")?;
    let pattern = string_arg(args, "pattern")?.to_lowercase();
    let resolved = resolve_within(workspace, &root)?;
    let mut results = Vec::new();
    walk_search(&resolved, &pattern, &mut results, 0).await?;
    Ok(ToolResult {
        ok: true,
        summary: format!(
            "found {} files matching {:?} under {}",
            results.len(),
            pattern,
            resolved.display()
        ),
        content: Some(truncate(&results.join("\n"), CONTENT_MAX)),
    })
}

async fn walk_search(
    dir: &Path,
    pattern: &str,
    results: &mut Vec<String>,
    depth: usize,
) -> Result<(), String> {
    if depth > SEARCH_MAX_DEPTH {
        return Ok(());
    }
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .map_err(|e| format!("Failed to read dir {}: {}", dir.display(), e))?;
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.to_lowercase().contains(pattern) {
            results.push(entry.path().to_string_lossy().to_string());
        }
        if entry.metadata().await.map_err(|e| e.to_string())?.is_dir() {
            Box::pin(walk_search(&entry.path(), pattern, results, depth + 1)).await?;
        }
    }
    Ok(())
}

async fn tool_write_file(workspace: &str, args: &Value) -> Result<ToolResult, String> {
    let path = string_arg(args, "path")?;
    let content = string_arg(args, "content")?;
    let resolved = resolve_within(workspace, &path)?;
    if let Some(parent) = resolved.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create parent dirs: {}", e))?;
    }
    tokio::fs::write(&resolved, &content)
        .await
        .map_err(|e| format!("Failed to write {}: {}", path, e))?;
    Ok(ToolResult {
        ok: true,
        summary: format!("wrote {} ({} bytes)", resolved.display(), content.len()),
        content: None,
    })
}

async fn tool_edit_file(workspace: &str, args: &Value) -> Result<ToolResult, String> {
    let path = string_arg(args, "path")?;
    let old = string_arg(args, "old")?;
    let new = string_arg(args, "new")?;
    if old.is_empty() {
        return Err("edit_file `old` must not be empty".into());
    }
    let resolved = resolve_within(workspace, &path)?;
    let haystack = tokio::fs::read_to_string(&resolved)
        .await
        .map_err(|e| format!("Failed to read {}: {}", path, e))?;
    let matches = haystack.matches(&old).count();
    if matches == 0 {
        return Err(format!("edit_file: old string not found in {}", path));
    }
    if matches > 1 {
        return Err(format!(
            "edit_file: old string appears {} times in {}; include more context",
            matches, path
        ));
    }
    let updated = haystack.replacen(&old, &new, 1);
    tokio::fs::write(&resolved, &updated)
        .await
        .map_err(|e| format!("Failed to write {}: {}", path, e))?;
    Ok(ToolResult {
        ok: true,
        summary: format!("edited {} (1 replacement)", resolved.display()),
        content: None,
    })
}

async fn tool_run_command(
    workspace: &str,
    args: &Value,
    timeout: Duration,
) -> Result<ToolResult, String> {
    let command = string_arg(args, "command")?;
    let mut cmd = Command::new(&command);
    cmd.args(string_array_arg(args, "args")?)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match args.get("cwd").and_then(|v| v.as_str()) {
        Some(cwd) => {
            let resolved = resolve_within(workspace, cwd)?;
            cmd.current_dir(resolved);
        }
        None => {
            cmd.current_dir(workspace);
        }
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to start {}: {}", command, e))?;
    let mut stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| "run_command: stdout not piped".to_string())?;
    let mut stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| "run_command: stderr not piped".to_string())?;

    let exit = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Err(format!("Failed to run {}: {}", command, e)),
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Ok(ToolResult {
                ok: false,
                summary: format!(
                    "command timed out after {}s: {}",
                    timeout.as_secs(),
                    command
                ),
                content: None,
            });
        }
    };
    let code = exit.code().unwrap_or(-1);

    // Pipes hit EOF once the process exits; drain them now.
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    stdout_pipe
        .read_to_end(&mut stdout)
        .await
        .map_err(|e| format!("Failed to read stdout: {}", e))?;
    stderr_pipe
        .read_to_end(&mut stderr)
        .await
        .map_err(|e| format!("Failed to read stderr: {}", e))?;

    let stdout = truncate(&String::from_utf8_lossy(&stdout), STREAM_MAX);
    let stderr = truncate(&String::from_utf8_lossy(&stderr), STREAM_MAX);
    let mut body = String::new();
    if !stdout.is_empty() {
        body.push_str("stdout:\n");
        body.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !body.is_empty() {
            body.push_str("\n");
        }
        body.push_str("stderr:\n");
        body.push_str(&stderr);
    }
    Ok(ToolResult {
        ok: code == 0,
        summary: format!("{} exited with code {}", command, code),
        content: if body.is_empty() {
            None
        } else {
            Some(truncate(&body, CONTENT_MAX))
        },
    })
}

async fn tool_web_search(args: &Value) -> Result<ToolResult, String> {
    let query = string_arg(args, "query")?;
    // Route through the multi-backend `search()` entry point. It
    // classifies the query's intent (factual / code / academic /
    // general) and tries the matching backend first, falling through
    // to DuckDuckGo as the general-purpose last resort. Each backend
    // is keyless; the router is the one that fixes the "DDG throttles
    // 16 parallel calls" failure mode from rounds 1-2.
    let results = crate::web_search::search(&query)
        .await
        .map_err(|e| format!("web search failed: {}", e))?;
    let mut lines = Vec::new();
    for r in results.iter().take(8) {
        lines.push(format!("{}\n{}\n{}", r.title, r.url, r.snippet));
    }
    let content = if lines.is_empty() {
        "No results.".to_string()
    } else {
        truncate(&lines.join("\n\n"), CONTENT_MAX)
    };
    // Empty / too-thin content has historically caused subagent loops:
    // the LLM sees the tool "succeed" with useless text, decides the
    // search wasn't specific enough, and re-calls `web_search` with
    // variations — eventually stalling the whole turn.
    //
    // The guard has two checks (both must pass for the tool to return
    // `ok: true`):
    //   1. The total formatted content must be at least
    //      `MIN_USEFUL_CONTENT_CHARS` characters — catches the
    //      "0 chars" / "30 chars" / "no results" failure mode in
    //      round-2 telemetry.
    //   2. At least one result must have a non-empty snippet — DDG
    //      sometimes returns 8 result blocks with every snippet blank
    //      (soft-throttled or anti-bot page). The block count alone
    //      doesn't tell the agent the content is empty, since
    //      title+url+separators add up to ~55 chars per result and
    //      sneak past check 1.
    let total_chars = content.chars().count();
    let has_any_snippet = results.iter().any(|r| !r.snippet.trim().is_empty());
    if total_chars < MIN_USEFUL_CONTENT_CHARS || !has_any_snippet {
        return Err(format!(
            "Web search returned no useful content for {:?} \
             ({} parsed blocks, {} chars). \
             Try a more specific query, or fall back to another tool.",
            query,
            results.len(),
            total_chars
        ));
    }
    Ok(ToolResult {
        ok: true,
        summary: format!("web search for {:?} ({} results)", query, results.len()),
        content: Some(content),
    })
}

/// Minimum total character count of a `web_search` response for the
/// tool to consider it "useful" and return `ok: true`. Below this, the
/// tool returns an error so the calling agent sees a clear failure
/// instead of an empty tool message that prompts another retry.
const MIN_USEFUL_CONTENT_CHARS: usize = 50;

/// `bip` — set the on-screen mascot's mood and (optionally) inner monologue.
///
/// Hard-gated: only the Health Agent (`AgentRole::HealthMonitor`) may call
/// this. Any other role gets a refused `ToolResult` with a clear message
/// (we don't error out the whole turn — the model can recover and keep
/// using the rest of its toolset).
async fn tool_bip(ctx: &ToolContext, args: &Value) -> Result<ToolResult, String> {
    if ctx.caller_role != AgentRole::HealthMonitor {
        return Ok(ToolResult {
            ok: false,
            summary: format!(
                "bip is restricted to the Health Agent; {:?} is not allowed",
                ctx.caller_role
            ),
            content: None,
        });
    }

    // Parse + validate the mood. We do this by hand (rather than
    // `serde_json::from_value::<BipMood>`) so the error message names the
    // rejected value — the agent's debug logs get the exact string back.
    let mood_str = match args.get("mood").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => {
            return Ok(ToolResult {
                ok: false,
                summary: "bip: `mood` is required and must be a string".into(),
                content: None,
            });
        }
    };
    let mood = match mood_str {
        "idle" => BipMood::Idle,
        "working" => BipMood::Working,
        "success" => BipMood::Success,
        "error" => BipMood::Error,
        other => {
            return Ok(ToolResult {
                ok: false,
                summary: format!(
                    "bip: unknown mood {:?} (expected one of: idle, working, success, error)",
                    other
                ),
                content: None,
            });
        }
    };

    // `thought` is optional. If the agent passes a non-string we reject it
    // (rather than silently ignoring) so the model gets clear feedback.
    let thought = match args.get("thought") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(_) => {
            return Ok(ToolResult {
                ok: false,
                summary: "bip: `thought` must be a string when provided".into(),
                content: None,
            });
        }
    };

    // Cap the thought length defensively — the popover is small.
    let thought = if thought.len() > 200 {
        truncate(&thought, 200)
    } else {
        thought
    };

    let state = BipState {
        mood,
        thought: thought.clone(),
        at: now_iso8601(),
        source: BipSource::Health,
    };

    // Persist + broadcast. The persist path lives in `commands::AppState`;
    // here we only emit the event so listeners (UI + future log shippers)
    // see the change immediately. The `init_bip_state` command lets a
    // freshly-connected WebView fetch the current value.
    if let Ok(payload) = serde_json::to_value(&state) {
        ctx.sink.emit("bip-state", payload);
    }

    Ok(ToolResult {
        ok: true,
        summary: format!(
            "bip: {} ({} chars of thought)",
            mood_str,
            thought.chars().count()
        ),
        content: None,
    })
}

// -- Subagent dispatch (delegate_subagent / subagent_message) --

/// `delegate_subagent` — spawn a subagent turn and wait for its final
/// answer. Hard-gated to the Orchestrator: subagents get the reduced
/// `ToolSpec::subagent()` (no `delegate_subagent` / `delegate_subagents`)
/// AND this role check, so recursion is impossible through either path.
///
/// The turn runs on a scratch session, streams the standard `agent-*`
/// events under its `sub-<uuid>` request id (the frontend opens a
/// tiled window for it), and the final content is handed back to the
/// orchestrator as this tool's result.
async fn tool_delegate_subagent(
    ctx: &ToolContext,
    args: &Value,
    call_id: &str,
) -> Result<ToolResult, String> {
    if ctx.caller_role != AgentRole::Orchestrator {
        return Ok(ToolResult {
            ok: false,
            summary: format!(
                "delegate_subagent is restricted to the Orchestrator; {:?} is not allowed",
                ctx.caller_role
            ),
            content: None,
        });
    }
    let Some(dispatch) = &ctx.subagent else {
        return Err("delegate_subagent: no subagent dispatch context on this turn".into());
    };

    let target = string_arg(args, "personality")?;
    let task = string_arg(args, "task")?;

    let registered = match register_one_dispatch(dispatch, &target, &task, call_id).await? {
        Some(r) => r,
        None => {
            return Ok(ToolResult {
                ok: false,
                summary: format!(
                    "delegate_subagent: {:?} is not a roster personality id or subagent role",
                    target
                ),
                content: None,
            });
        }
    };

    // Spec `2026-08-14--parallel-subagent-dispatch`: the orchestrator's
    // tool loop drives dispatch via `execute_tool_batch` in
    // `agent_loop.rs`. Here we just record the dispatch — the actual
    // subagent turn is admitted and awaited by the loop, gated by
    // `dispatch.subagent_semaphore` (cap = `max_concurrent_subagents`).
    // We return immediately with a queued summary; the loop polls the
    // resulting subagent record when it needs the final content.
    let queued = format!(
        "queued subagent {} ({}) — running in background",
        registered.request_id, registered.personality_name
    );
    Ok(ToolResult {
        ok: true,
        summary: queued,
        content: None,
    })
}

/// `delegate_subagents` (plural, spec `2026-08-14--plural-subagent-dispatch`)
/// — fire N subagent dispatches in a single tool call. Equivalent to
/// emitting N `delegate_subagent` tool calls in one LLM round, but lets
/// the model fan out without the per-round output budget pressure that
/// otherwise makes it emit one dispatch per round and wait between
/// them. The model still gets exactly one combined `role: "tool"`
/// message back, summarizing every dispatched subagent's outcome.
///
/// Hard-gated to the Orchestrator — same role check as the singular
/// tool. Subagent recursion is impossible: `ToolSpec::subagent()`
/// filters out both `delegate_subagent` and `delegate_subagents`, and
/// the role check here is the second backstop.
async fn tool_delegate_subagents(
    ctx: &ToolContext,
    args: &Value,
    call_id: &str,
) -> Result<ToolResult, String> {
    if ctx.caller_role != AgentRole::Orchestrator {
        return Ok(ToolResult {
            ok: false,
            summary: format!(
                "delegate_subagents is restricted to the Orchestrator; {:?} is not allowed",
                ctx.caller_role
            ),
            content: None,
        });
    }
    let Some(dispatch) = &ctx.subagent else {
        return Err("delegate_subagents: no subagent dispatch context on this turn".into());
    };

    // Parse the array. Reject any non-array shape up front so the
    // model gets a clear error instead of a silent empty batch.
    let entries = match args.get("dispatches").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => {
            return Ok(ToolResult {
                ok: false,
                summary: "delegate_subagents: `dispatches` must be a non-empty array of \
                          {personality, task} objects"
                    .into(),
                content: None,
            });
        }
    };
    if entries.is_empty() {
        return Ok(ToolResult {
            ok: false,
            summary: "delegate_subagents: `dispatches` must contain at least one entry".into(),
            content: None,
        });
    }
    // Cap the fan-out: matches the subagent semaphore's hard upper
    // bound (range 1..=16) so the model can never request more
    // dispatches than the roster permits anyway. Belt-and-braces
    // because the LLM might ask for 50 at once.
    const MAX_FANOUT: usize = 16;
    if entries.len() > MAX_FANOUT {
        return Ok(ToolResult {
            ok: false,
            summary: format!(
                "delegate_subagents: {} dispatches requested, max is {}",
                entries.len(),
                MAX_FANOUT
            ),
            content: None,
        });
    }

    // Register every dispatch. We collect the (sub_id, personality
    // name) pairs that succeeded so the queued summary can name them
    // all, and push the failures into a per-entry error list so the
    // model knows exactly which ones didn't take. The dispatcher
    // runs the registered ones in parallel under the subagent
    // semaphore, same as for N `delegate_subagent` calls in one
    // round.
    let mut registered: Vec<RegisteredDispatch> = Vec::with_capacity(entries.len());
    let mut failures: Vec<String> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let target = match entry.get("personality").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                failures.push(format!("#{}: missing `personality` (string)", i));
                continue;
            }
        };
        let task = match entry.get("task").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s,
            Some(_) => {
                failures.push(format!("#{}: empty `task`", i));
                continue;
            }
            None => {
                failures.push(format!("#{}: missing `task` (string)", i));
                continue;
            }
        };
        match register_one_dispatch(dispatch, target, task, call_id).await? {
            Some(r) => registered.push(r),
            None => failures.push(format!(
                "#{}: {:?} is not a roster personality id or subagent role",
                i, target
            )),
        }
    }

    // If everything failed, surface a single error result so the model
    // sees the problem instead of an "queued 0 subagents" lie.
    if registered.is_empty() {
        return Ok(ToolResult {
            ok: false,
            summary: format!(
                "delegate_subagents: no dispatches registered ({})",
                failures.join("; ")
            ),
            content: None,
        });
    }

    // Build the queued summary. We name the first few personalities
    // and tail with "and N more" if the list is long, so the
    // ToolResult summary stays scannable in the chat log.
    let names: Vec<String> = registered
        .iter()
        .map(|r| r.personality_name.clone())
        .collect();
    let name_preview = if names.len() <= 4 {
        names.join(", ")
    } else {
        format!("{}, …and {} more", names[..4].join(", "), names.len() - 4)
    };
    let queued = if failures.is_empty() {
        format!(
            "queued {} subagents ({}) — running in background",
            registered.len(),
            name_preview
        )
    } else {
        format!(
            "queued {} subagents ({}); skipped {} ({})",
            registered.len(),
            name_preview,
            failures.len(),
            failures.join("; ")
        )
    };
    Ok(ToolResult {
        ok: true,
        summary: queued,
        content: None,
    })
}

/// What `register_one_dispatch` returns on success. Bundles the new
/// subagent request id and the resolved personality's display name so
/// both the singular and plural tools can build a "queued ..." summary
/// without re-resolving the personality.
pub struct RegisteredDispatch {
    pub request_id: String,
    pub personality_name: String,
}

/// Register one subagent dispatch against the orchestrator's
/// `SubagentDispatchContext`. Inserts the `Working` registry row, the
/// inbox slot, the per-subagent cancel watcher, and the
/// `call_id → sub_id` index entry. The actual subagent turn is driven
/// later by `execute_tool_batch` in `agent_loop.rs` under the
/// `subagent_semaphore` cap.
///
/// Returns `Some(RegisteredDispatch)` on success, `None` if the target
/// didn't resolve to a subagent-eligible personality (the singular
/// and plural tools each format their own "not a roster …" message
/// from this so the per-entry context is preserved for the plural
/// case).
pub(crate) async fn register_one_dispatch(
    dispatch: &SubagentDispatchContext,
    target: &str,
    task: &str,
    call_id: &str,
) -> Result<Option<RegisteredDispatch>, String> {
    // Resolve the personality: roster id first ("builtin-coder", a user
    // row), then a role slug ("coder", "health-monitor", …).
    let personality = match get_personality(target).await? {
        Some(p) => p,
        None => match AgentRole::from_slug(target) {
            Some(role) if role.is_subagent() => get_personality(&builtin_personality_id(role))
                .await?
                .ok_or_else(|| format!("delegate_subagent: no roster row for role {:?}", role))?,
            _ => return Ok(None),
        },
    };
    // The Orchestrator is not a dispatch target: no roster row exists
    // for it, but guard anyway (a user could theoretically craft a
    // personality with the Orchestrator role — refuse to recurse).
    if !personality.role.is_subagent() {
        // Mirror the singular tool's wording so the model sees a
        // consistent "cannot dispatch to Orchestrator" error no
        // matter which entrypoint it used.
        return Err("delegate_subagent: cannot dispatch to the Orchestrator".into());
    }

    // Personality-bound saved model when it exists, else the
    // orchestrator's chat model.
    let model_id =
        crate::models::resolve_personality_model(&personality, &dispatch.chat_model_id).await;

    let request_id = format!("{SUBAGENT_REQUEST_PREFIX}{}", Uuid::new_v4());
    dispatch.subagents.insert(
        request_id.clone(),
        SubagentRecord::working(
            &request_id,
            &personality.id,
            &personality.name,
            personality.role,
            task,
            Some(&model_id),
            Some(&dispatch.request_id),
        ),
    );
    dispatch
        .inbox
        .insert(request_id.clone(), TokioMutex::new(Vec::new()));

    // Per-subagent cancel watcher. The shared `agent_in_flight` map
    // (keyed by `sub-<uuid>`) already lets `cancel_agent` target one
    // subagent without touching siblings, so this watcher is dormant
    // today — we insert a sender anyway so the dispatcher can wire
    // up a `select!` arm without the per-call map lookup returning
    // `None`. A future "cancel all subagents in this batch" button
    // would iterate this map.
    let (cancel_tx, _cancel_rx_init) = watch::channel(false);
    dispatch
        .subagent_cancel
        .insert(request_id.clone(), cancel_tx);

    // Index orchestrator-side call id → subagent id so the batch
    // dispatcher can pair the result with the right call when it
    // writes the combined tool message. The index is now a Vec
    // because one model-side call (the plural tool) can fan out to
    // N subagent turns. The singular tool inserts a one-element
    // vec; the plural tool appends to (or creates) the vec for its
    // call id. Empty `call_id` is a soft warning — the batch still
    // works, it just can't pair the synthetic tool message by id.
    if !call_id.is_empty() {
        dispatch
            .dispatch_call_index
            .entry(call_id.to_string())
            .or_default()
            .push(request_id.clone());
    }

    Ok(Some(RegisteredDispatch {
        request_id,
        personality_name: personality.name,
    }))
}

/// Register every subagent dispatch in a `delegate_subagents` (plural)
/// tool call against the orchestrator's `SubagentDispatchContext`.
/// Returns the list of successfully-registered `(request_id,
/// personality_name)` pairs. Per-entry validation failures
/// (missing `personality` / `task`, unknown target) are collected in
/// the `failures` out-param so the caller can surface them in the
/// queued summary.
///
/// Hard-fails (returns `Err`) only on the role-gate denial, which is
/// already handled by `tool_delegate_subagents` before calling here.
pub(crate) async fn register_plural_dispatches(
    dispatch: &SubagentDispatchContext,
    args: &Value,
    call_id: &str,
) -> (Vec<RegisteredDispatch>, Vec<String>) {
    let mut registered = Vec::new();
    let mut failures = Vec::new();
    let entries = match args.get("dispatches").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => {
            failures.push("`dispatches` must be a non-empty array".into());
            return (registered, failures);
        }
    };
    if entries.is_empty() {
        failures.push("`dispatches` must contain at least one entry".into());
        return (registered, failures);
    }
    for (i, entry) in entries.iter().enumerate() {
        let target = match entry.get("personality").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                failures.push(format!("#{}: missing `personality` (string)", i));
                continue;
            }
        };
        let task = match entry.get("task").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s,
            Some(_) => {
                failures.push(format!("#{}: empty `task`", i));
                continue;
            }
            None => {
                failures.push(format!("#{}: missing `task` (string)", i));
                continue;
            }
        };
        // We treat per-entry resolution errors as soft failures
        // (logged in `failures`, no registration) so one bad
        // entry doesn't kill the whole batch. The only hard error
        // is the Orchestrator-role guard inside
        // `register_one_dispatch`, which we propagate.
        match register_one_dispatch(dispatch, target, task, call_id).await {
            Ok(Some(r)) => registered.push(r),
            Ok(None) => failures.push(format!(
                "#{}: {:?} is not a roster personality id or subagent role",
                i, target
            )),
            Err(e) => {
                // Hard failure (e.g. Orchestrator-role target).
                // Surface it as a per-entry failure so the rest of
                // the batch still gets a chance.
                failures.push(format!("#{}: {}", i, e));
            }
        }
    }
    (registered, failures)
}

/// `subagent_message` — deliver a message to a running subagent. The
/// message lands in the subagent's inbox and is fed into its next LLM
/// round as a `[Health Monitor]` user message. Hard-gated to the Health
/// Agent (`AgentRole::HealthMonitor`), mirroring the `bip` tool's gate.
async fn tool_subagent_message(ctx: &ToolContext, args: &Value) -> Result<ToolResult, String> {
    if ctx.caller_role != AgentRole::HealthMonitor {
        return Ok(ToolResult {
            ok: false,
            summary: format!(
                "subagent_message is restricted to the Health Agent; {:?} is not allowed",
                ctx.caller_role
            ),
            content: None,
        });
    }
    let Some(dispatch) = &ctx.subagent else {
        return Err("subagent_message: no subagent dispatch context on this turn".into());
    };

    let request_id = string_arg(args, "request_id")?;
    let message = string_arg(args, "message")?;
    if message.trim().is_empty() {
        return Ok(ToolResult {
            ok: false,
            summary: "subagent_message: `message` must not be empty".into(),
            content: None,
        });
    }
    let Some(entry) = dispatch.inbox.get(&request_id) else {
        return Ok(ToolResult {
            ok: false,
            summary: format!(
                "subagent_message: no active subagent with request id {}",
                request_id
            ),
            content: None,
        });
    };
    entry.value().lock().await.push(message.clone());

    // Broadcast so the subagent window can display the incoming message
    // without polling. Routed by requestId like the other agent events.
    ctx.sink.emit(
        "subagent-message",
        json!({ "requestId": request_id, "message": message }),
    );

    Ok(ToolResult {
        ok: true,
        summary: format!("subagent_message: delivered to {}", request_id),
        content: None,
    })
}

// -- Workspace guard --

/// Resolve `target` (absolute or workspace-relative) to a path that is
/// guaranteed to live inside `workspace`, canonicalizing so symlinks cannot
/// smuggle reads/writes outside. Non-existent targets (e.g. a file being
/// written) canonicalize their deepest existing ancestor instead.
pub fn resolve_within(workspace: &str, target: &str) -> Result<PathBuf, String> {
    let ws_path = Path::new(workspace);
    let ws_canon = ws_path
        .canonicalize()
        .map_err(|e| format!("Workspace {} is not accessible: {}", workspace, e))?;

    // Relative targets resolve from the canonical workspace so spellings
    // (drive case, `\\?\` verbatim prefixes, 8.3 short names) always agree.
    let joined = if Path::new(target).is_absolute() {
        PathBuf::from(target)
    } else {
        ws_canon.join(target)
    };
    let normalized = lexically_normalize(&joined);
    if normalized.is_relative() {
        return Err(format!("Target escapes workspace: {}", target));
    }

    // Correctness comes from canonicalization, not the lexical shape:
    // existing paths canonicalize fully (expanding short names and symlinks),
    // so a canonical-to-canonical prefix compare is authoritative.
    if normalized.exists() {
        let canon = normalized
            .canonicalize()
            .map_err(|e| format!("Cannot resolve {}: {}", target, e))?;
        if path_starts_with(&ws_canon, &canon) {
            return Ok(deverbatim(canon));
        }
        return Err(format!(
            "Target resolves outside the workspace (symlink?): {}",
            target
        ));
    }

    // New path: canonicalize the deepest existing ancestor, then re-attach
    // the remaining components so parent symlinks cannot escape.
    let mut ancestor = normalized.as_path();
    let mut suffix: Vec<PathBuf> = Vec::new();
    while !ancestor.exists() {
        match ancestor.file_name() {
            Some(name) => {
                suffix.push(PathBuf::from(name));
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| format!("No existing ancestor for target: {}", target))?;
            }
            None => return Err(format!("No existing ancestor for target: {}", target)),
        }
    }
    let canon_ancestor = ancestor
        .canonicalize()
        .map_err(|e| format!("Cannot resolve {}: {}", target, e))?;
    if !path_starts_with(&ws_canon, &canon_ancestor) {
        return Err(format!(
            "Target resolves outside the workspace (symlink?): {}",
            target
        ));
    }
    let mut final_path = deverbatim(canon_ancestor);
    for comp in suffix.iter().rev() {
        final_path.push(comp);
    }
    Ok(final_path)
}

/// Windows `canonicalize` returns `\\?\`-prefixed verbatim paths; strip the
/// prefix so returned paths (and summaries) stay user-friendly. No-op
/// elsewhere.
fn deverbatim(p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    p
}

fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                // Pop a normal component when there is one; never climb past
                // the root/prefix (out.pop returns false at the root).
                if !matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.push(comp.as_os_str());
                } else {
                    out.pop();
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                out.push(comp.as_os_str())
            }
        }
    }
    out
}

/// Component-wise prefix check; case-insensitive on Windows.
fn path_starts_with(base: &Path, path: &Path) -> bool {
    let base = deverbatim(base.to_path_buf());
    let path = deverbatim(path.to_path_buf());
    let base_comps: Vec<_> = base.components().collect();
    let path_comps: Vec<_> = path.components().collect();
    if path_comps.len() < base_comps.len() {
        return false;
    }
    for (b, p) in base_comps.iter().zip(path_comps.iter()) {
        let bs = b.as_os_str().to_string_lossy();
        let ps = p.as_os_str().to_string_lossy();
        #[cfg(windows)]
        {
            if !bs.eq_ignore_ascii_case(&ps) {
                return false;
            }
        }
        #[cfg(not(windows))]
        {
            if bs != ps {
                return false;
            }
        }
    }
    true
}

// -- Arg helpers --

fn string_arg(args: &Value, name: &str) -> Result<String, String> {
    args.get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("Tool argument {:?} must be a string", name))
}

fn string_array_arg(args: &Value, name: &str) -> Result<Vec<String>, String> {
    let arr = args
        .get(name)
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("Tool argument {:?} must be an array", name))?;
    arr.iter()
        .map(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| format!("Tool argument {:?} must contain only strings", name))
        })
        .collect()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(12)).collect();
        format!("{}…[truncated]", head)
    }
}

// -- Todo list (spec 2026-08-14--agent-todo-list.md) --
//
// The todo tools do their own load + apply + save + emit. They share
// the per-session write lock in `AppState::todo_locks` with the user
// IPC commands, so the two paths can't race. The agent loop is
// expected to re-read the session from disk on its next `persist()`
// after a todo tool commits; the spec's "agent loop integration"
// section calls this out.
//
// We don't pass `AppState` through `ToolContext` (the existing
// pattern keeps the context small). Instead, the tool handler takes
// the active session id from `ctx.session_id` (set by the loop) and
// uses `session::resolve_active_session_id` / `load_session` /
// `save_session` directly. Tools that need `AppState` are dispatched
// from the IPC layer (which has `State<'_, AppState>`); agent tools
// in this codebase don't take `AppState` (see e.g. `bip`).

/// Shared helper for the agent-side todo tool path. Acquires the
/// per-session write lock (lazily created on first use), loads the
/// session, applies the mutation, saves, and emits the
/// `todo_changed` event with `source: "agent"`. Returns the new
/// `TodoList` so the model can see the post-mutation state in its
/// next round.
async fn agent_todo_mutation<F>(ctx: &ToolContext, f: F) -> Result<TodoList, String>
where
    F: FnOnce(&mut TodoList) -> Result<(), String>,
{
    let Some(session_id) = ctx.session_id.as_deref() else {
        return Err("todo tool called without a session id in context".into());
    };
    let project_path = ctx.workspace.as_str();

    // The lock lives in `AppState::todo_locks`. We can't get at
    // `AppState` from `ToolContext` (the existing pattern keeps
    // these separate), so for the agent path we use a local
    // per-process lock keyed by (project, session) instead. This is
    // weaker than the global `AppState` lock — it doesn't serialize
    // against a user IPC running on the same instance — but for the
    // current single-process Tauri model, this is enough. The
    // user-IPC path uses its own lock in `AppState`; the agent path
    // uses this `DashMap` here. Both use the same `Mutex<()>`
    // primitive. Future work: thread `AppState` through
    // `ToolContext` and unify the locks.
    use dashmap::DashMap;
    use std::sync::OnceLock;
    static AGENT_TODO_LOCKS: OnceLock<DashMap<(String, String), Arc<TokioMutex<()>>>> =
        OnceLock::new();
    let locks = AGENT_TODO_LOCKS.get_or_init(|| DashMap::new());
    let key = (project_path.to_string(), session_id.to_string());
    let lock = if let Some(l) = locks.get(&key) {
        l.value().clone()
    } else {
        let l = Arc::new(TokioMutex::new(()));
        locks
            .entry(key)
            .or_insert_with(|| l.clone())
            .value()
            .clone()
    };
    let _guard = lock.lock().await;

    let mut session = crate::session::load_session(project_path, session_id)
        .await?
        .ok_or_else(|| format!("Session {} not found", session_id))?;

    f(&mut session.todos)?;
    let new_list = session.todos.clone();
    let _saved = crate::session::save_session(project_path, session).await?;

    todo_mod::emit_todo_changed(&ctx.sink, session_id, &new_list, TodoChangeSource::Agent);

    Ok(new_list)
}

async fn tool_todo_set(ctx: &ToolContext, args: &Value) -> Result<ToolResult, String> {
    let items_v = match args.get("items").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => {
            return Ok(ToolResult {
                ok: false,
                summary: "todo_set: `items` is required and must be an array".into(),
                content: None,
            });
        }
    };
    let inputs: Vec<TodoItemInput> = match serde_json::from_value(Value::Array(items_v.clone())) {
        Ok(v) => v,
        Err(e) => {
            return Ok(ToolResult {
                ok: false,
                summary: format!("todo_set: invalid item shape: {}", e),
                content: None,
            });
        }
    };

    // Validate the "finished" rule before doing the load+save round-trip.
    let project_path = ctx.workspace.as_str();
    let Some(session_id) = ctx.session_id.as_deref() else {
        return Err("todo_set: missing session id".into());
    };
    let existing = crate::session::load_session(project_path, session_id)
        .await?
        .ok_or_else(|| format!("Session {} not found", session_id))?;
    if let Err(hint) = todo_mod::validate_todo_set(&existing.todos) {
        return Ok(ToolResult {
            ok: false,
            summary: hint,
            content: None,
        });
    }

    let list = agent_todo_mutation(ctx, |l| todo_mod::apply_set(l, inputs)).await?;
    Ok(ToolResult {
        ok: true,
        summary: format!(
            "Todo list replaced ({} items, v{}).",
            list.items.len(),
            list.version
        ),
        content: Some(serde_json::to_string(&list).unwrap_or_default()),
    })
}

async fn tool_todo_append(ctx: &ToolContext, args: &Value) -> Result<ToolResult, String> {
    let items_v = match args.get("items").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => {
            return Ok(ToolResult {
                ok: false,
                summary: "todo_append: `items` is required and must be an array".into(),
                content: None,
            });
        }
    };
    let inputs: Vec<TodoItemInput> = match serde_json::from_value(Value::Array(items_v.clone())) {
        Ok(v) => v,
        Err(e) => {
            return Ok(ToolResult {
                ok: false,
                summary: format!("todo_append: invalid item shape: {}", e),
                content: None,
            });
        }
    };
    let position = args
        .get("position")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);
    let list = agent_todo_mutation(ctx, |l| todo_mod::apply_append(l, inputs, position)).await?;
    Ok(ToolResult {
        ok: true,
        summary: format!(
            "Todo list now has {} items (v{}).",
            list.items.len(),
            list.version
        ),
        content: Some(serde_json::to_string(&list).unwrap_or_default()),
    })
}

async fn tool_todo_update(ctx: &ToolContext, args: &Value) -> Result<ToolResult, String> {
    let updates_v = match args.get("updates").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => {
            return Ok(ToolResult {
                ok: false,
                summary: "todo_update: `updates` is required and must be an array".into(),
                content: None,
            });
        }
    };
    let updates: Vec<TodoItemPatch> = match serde_json::from_value(Value::Array(updates_v.clone()))
    {
        Ok(v) => v,
        Err(e) => {
            return Ok(ToolResult {
                ok: false,
                summary: format!("todo_update: invalid update shape: {}", e),
                content: None,
            });
        }
    };
    let list = agent_todo_mutation(ctx, |l| todo_mod::apply_update(l, updates)).await?;
    Ok(ToolResult {
        ok: true,
        summary: format!("Todo list updated (v{}).", list.version),
        content: Some(serde_json::to_string(&list).unwrap_or_default()),
    })
}

async fn tool_todo_remove(ctx: &ToolContext, args: &Value) -> Result<ToolResult, String> {
    let ids_v = match args.get("ids").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => {
            return Ok(ToolResult {
                ok: false,
                summary: "todo_remove: `ids` is required and must be an array".into(),
                content: None,
            });
        }
    };
    let ids: Vec<String> = ids_v
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    let list = agent_todo_mutation(ctx, |l| todo_mod::apply_remove(l, ids)).await?;
    Ok(ToolResult {
        ok: true,
        summary: format!(
            "Todo list now has {} items (v{}).",
            list.items.len(),
            list.version
        ),
        content: Some(serde_json::to_string(&list).unwrap_or_default()),
    })
}

async fn tool_todo_clear(ctx: &ToolContext, args: &Value) -> Result<ToolResult, String> {
    let _ = args; // No args expected.
    let list = agent_todo_mutation(ctx, |l| {
        todo_mod::apply_clear(l);
        Ok(())
    })
    .await?;
    Ok(ToolResult {
        ok: true,
        summary: "Todo list cleared.".into(),
        content: Some(serde_json::to_string(&list).unwrap_or_default()),
    })
}

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentRole;
    use crate::agent_loop::AgentEventSink;
    use crate::llm_client::{AgentTurnError, ChatUsage, ToolChatRequest, TurnOutcome};
    use std::sync::Mutex;
    use tempfile::TempDir;

    fn ws(dir: &TempDir) -> String {
        dir.path().to_string_lossy().to_string()
    }

    /// Test sink that records every emitted (event, payload) pair.
    #[derive(Default)]
    struct TestSink {
        events: Mutex<Vec<(String, Value)>>,
    }
    impl AgentEventSink for TestSink {
        fn emit(&self, event: &str, payload: Value) {
            self.events
                .lock()
                .unwrap()
                .push((event.to_string(), payload));
        }
    }

    /// Build a fresh `ToolContext` for a test. Defaults to `Coder` + a fresh
    /// `TestSink`; tests for `bip` override the role and inspect the sink.
    fn ctx_for(workspace: &str, caller_role: AgentRole) -> (ToolContext, std::sync::Arc<TestSink>) {
        let sink = std::sync::Arc::new(TestSink::default());
        let ctx = ToolContext {
            workspace: workspace.to_string(),
            auto_allowed: true,
            caller_role,
            sink: sink.clone() as std::sync::Arc<dyn AgentEventSink>,
            subagent: None,
            session_id: None,
        };
        (ctx, sink)
    }

    // -- Workspace guard matrix --

    #[tokio::test]
    async fn guard_allows_inside_paths() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main(){}").unwrap();
        let w = ws(&dir);

        // Relative and absolute forms both resolve inside.
        let rel = resolve_within(&w, "src/main.rs").unwrap();
        let abs = resolve_within(&w, &root.join("src/main.rs").to_string_lossy()).unwrap();
        assert_eq!(rel, abs);
        assert!(rel.ends_with("src/main.rs"));

        // Non-existent file with a real parent is fine.
        let fresh = resolve_within(&w, "src/new.rs").unwrap();
        assert!(fresh.ends_with("src/new.rs"));
    }

    #[tokio::test]
    async fn guard_rejects_escapes() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);

        assert!(resolve_within(&w, "..").is_err());
        assert!(resolve_within(&w, "../x").is_err());
        assert!(resolve_within(&w, "a/../../x").is_err());
        // Absolute path outside the workspace.
        let outside = dir.path().parent().unwrap().join("outside.txt");
        std::fs::write(&outside, "x").unwrap();
        let outside_s = outside.to_string_lossy().to_string();
        assert!(resolve_within(&w, &outside_s).is_err());
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn guard_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let outside = dir.path().parent().unwrap().join("secret.txt");
        std::fs::write(&outside, "top secret").unwrap();
        symlink(&outside, root.join("link.txt")).unwrap();
        let w = ws(&dir);
        assert!(resolve_within(&w, "link.txt").is_err());
    }

    #[tokio::test]
    async fn guard_resolves_empty_target_to_workspace() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        // "" resolves to the workspace root itself; tools fail harmlessly
        // when they try to read/write a directory as a file.
        let resolved = resolve_within(&w, "").unwrap();
        let expected = deverbatim(Path::new(&w).canonicalize().unwrap());
        assert_eq!(resolved, expected);
    }

    // -- edit_file semantics --

    #[tokio::test]
    async fn edit_file_single_occurrence() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let path = root.join("src/app.rs");
        std::fs::write(&path, "let x = 1;\n// TODO: fix\n").unwrap();
        let w = ws(&dir);

        let res = execute_tool(
            &ctx_for(&w, AgentRole::Coder).0,
            "edit_file",
            &json!({"path": "src/app.rs", "old": "TODO: fix", "new": "fixed"}),
        )
        .await
        .unwrap();
        assert!(res.ok);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "let x = 1;\n// fixed\n"
        );
    }

    #[tokio::test]
    async fn edit_file_requires_exactly_one_match() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a.txt"), "no match here").unwrap();
        let w = ws(&dir);
        let (ctx, _) = ctx_for(&w, AgentRole::Coder);
        let res = execute_tool(
            &ctx,
            "edit_file",
            &json!({"path": "a.txt", "old": "missing", "new": "x"}),
        )
        .await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("not found"));

        std::fs::write(root.join("b.txt"), "dup dup").unwrap();
        let res = execute_tool(
            &ctx,
            "edit_file",
            &json!({"path": "b.txt", "old": "dup", "new": "x"}),
        )
        .await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("2 times"));
    }

    // -- write_file creates parents --

    #[tokio::test]
    async fn write_file_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let res = execute_tool(
            &ctx_for(&w, AgentRole::Coder).0,
            "write_file",
            &json!({"path": "deep/nested/file.txt", "content": "hello"}),
        )
        .await
        .unwrap();
        assert!(res.ok);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("deep/nested/file.txt")).unwrap(),
            "hello"
        );
    }

    // -- read/list/search --

    #[tokio::test]
    async fn read_and_list_and_search() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main(){}").unwrap();
        std::fs::write(root.join("README.md"), "docs").unwrap();
        let w = ws(&dir);

        let (ctx, _) = ctx_for(&w, AgentRole::Coder);
        let read = execute_tool(&ctx, "read_file", &json!({"path": "src/main.rs"}))
            .await
            .unwrap();
        assert!(read.ok);
        assert_eq!(read.content.as_deref(), Some("fn main(){}"));

        let list = execute_tool(&ctx, "list_dir", &json!({"path": "."}))
            .await
            .unwrap();
        assert!(list.ok);
        assert!(list.content.unwrap().contains("README.md"));

        let search = execute_tool(
            &ctx,
            "search_files",
            &json!({"root": ".", "pattern": "main"}),
        )
        .await
        .unwrap();
        assert!(search.ok);
        assert!(search.content.unwrap().contains("main.rs"));
    }

    // -- run_command: gating, truncation, timeout --

    #[tokio::test]
    async fn run_command_requires_approval_flag() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let (mut ctx, _) = ctx_for(&w, AgentRole::Coder);
        ctx.auto_allowed = false;
        let res = execute_tool(
            &ctx,
            "run_command",
            &json!({"command": "echo", "args": ["hi"]}),
        )
        .await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("requires approval"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn run_command_truncates_long_output() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let res = execute_tool(
            &ctx_for(&w, AgentRole::Coder).0,
            "run_command",
            &json!({
                "command": "powershell",
                "args": ["-NoProfile", "-Command", "Write-Output ('x' * 50000)"]
            }),
        )
        .await
        .unwrap();
        assert!(res.ok, "summary: {}", res.summary);
        let content = res.content.unwrap();
        // A single 50 KB line exceeds the 16 KB stream cap.
        assert!(content.len() <= CONTENT_MAX);
        assert!(content.contains("truncated"));
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn run_command_truncates_long_output() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let res = execute_tool(
            &ctx_for(&w, AgentRole::Coder).0,
            "run_command",
            &json!({"command": "sh", "args": ["-c", "yes x | head -c 100000"]}),
        )
        .await
        .unwrap();
        assert!(res.ok);
        assert!(res.content.unwrap().len() <= CONTENT_MAX);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn run_command_times_out_and_kills_child() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        // Use the internal variant with a short timeout to keep the test fast.
        let res = tool_run_command(
            &w,
            &json!({
                "command": "powershell",
                "args": ["-NoProfile", "-Command", "Start-Sleep -Seconds 10"]
            }),
            Duration::from_millis(300),
        )
        .await
        .unwrap();
        assert!(!res.ok);
        assert!(res.summary.contains("timed out"));
    }

    // -- web_search arg validation (no network) --

    #[tokio::test]
    async fn web_search_validates_query_arg() {
        let (ctx, _) = ctx_for("", AgentRole::Coder);
        let res = execute_tool(&ctx, "web_search", &json!({})).await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("query"));

        let res = execute_tool(&ctx, "web_search", &json!({"query": 42})).await;
        assert!(res.is_err());
    }

    // -- unknown tool --

    #[tokio::test]
    async fn unknown_tool_is_rejected() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let res = execute_tool(&ctx_for(&w, AgentRole::Coder).0, "nope", &json!({})).await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("Unknown tool"));
    }

    // -- bip meta-tool --

    #[tokio::test]
    async fn bip_refuses_non_health_caller() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let (ctx, sink) = ctx_for(&w, AgentRole::Coder);
        let res = execute_tool(&ctx, "bip", &json!({"mood": "working"}))
            .await
            .unwrap();
        assert!(
            !res.ok,
            "bip must refuse non-Health callers; got: {:?}",
            res
        );
        assert!(res.summary.contains("Health Agent"));
        // No event was emitted — the refusal happens before we touch the sink.
        assert!(sink.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn bip_emits_event_for_health_caller() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let (ctx, sink) = ctx_for(&w, AgentRole::HealthMonitor);
        let res = execute_tool(
            &ctx,
            "bip",
            &json!({"mood": "success", "thought": "All quiet on the disk."}),
        )
        .await
        .unwrap();
        assert!(res.ok, "summary: {}", res.summary);
        assert!(res.summary.contains("success"));

        let events = sink.events.lock().unwrap().clone();
        assert_eq!(events.len(), 1, "exactly one event");
        assert_eq!(events[0].0, "bip-state");
        let payload = &events[0].1;
        assert_eq!(payload["mood"], "success");
        assert_eq!(payload["thought"], "All quiet on the disk.");
        assert_eq!(payload["source"], "health");
        assert!(payload["at"].as_str().unwrap().ends_with('Z'));
    }

    #[tokio::test]
    async fn bip_omitted_thought_falls_back_to_empty() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let (ctx, sink) = ctx_for(&w, AgentRole::HealthMonitor);
        let res = execute_tool(&ctx, "bip", &json!({"mood": "idle"}))
            .await
            .unwrap();
        assert!(res.ok);
        let events = sink.events.lock().unwrap().clone();
        assert_eq!(events[0].1["thought"], "");
        assert_eq!(events[0].1["mood"], "idle");
    }

    #[tokio::test]
    async fn bip_rejects_unknown_mood() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let (ctx, sink) = ctx_for(&w, AgentRole::HealthMonitor);
        let res = execute_tool(&ctx, "bip", &json!({"mood": "vibing"}))
            .await
            .unwrap();
        assert!(!res.ok);
        assert!(res.summary.contains("unknown mood"));
        assert!(sink.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn bip_rejects_missing_mood() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let (ctx, sink) = ctx_for(&w, AgentRole::HealthMonitor);
        let res = execute_tool(&ctx, "bip", &json!({})).await.unwrap();
        assert!(!res.ok);
        assert!(res.summary.contains("`mood` is required"));
        assert!(sink.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn bip_rejects_non_string_thought() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let (ctx, sink) = ctx_for(&w, AgentRole::HealthMonitor);
        let res = execute_tool(&ctx, "bip", &json!({"mood": "working", "thought": 42}))
            .await
            .unwrap();
        assert!(!res.ok);
        assert!(res.summary.contains("`thought` must be a string"));
        assert!(sink.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn bip_truncates_overlong_thought() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let (ctx, sink) = ctx_for(&w, AgentRole::HealthMonitor);
        let huge = "x".repeat(5000);
        let res = execute_tool(&ctx, "bip", &json!({"mood": "working", "thought": huge}))
            .await
            .unwrap();
        assert!(res.ok);
        let events = sink.events.lock().unwrap().clone();
        let payload_thought = events[0].1["thought"].as_str().unwrap();
        // The cap is 200 chars (the popover is small). Allow a tiny byte
        // overage from the ASCII suffix in `truncate`.
        assert!(
            payload_thought.chars().count() <= 200,
            "thought should be truncated, got {} chars",
            payload_thought.chars().count()
        );
    }

    // -- delegate_subagent / subagent_message --

    /// Redirect the global personalities file into a tempdir for the
    /// duration of a test (resolution reads the roster from disk).
    struct DataDirGuard {
        prev: Option<std::path::PathBuf>,
        _tmp: TempDir,
    }
    impl DataDirGuard {
        fn new() -> Self {
            let tmp = TempDir::new().unwrap();
            let prev = dirs::data_dir();
            // SAFETY: tests are single-threaded for this guard's lifetime.
            unsafe {
                std::env::set_var("HARNESS_DATA_DIR_OVERRIDE", tmp.path());
            }
            Self { prev, _tmp: tmp }
        }
    }
    impl Drop for DataDirGuard {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var("HARNESS_DATA_DIR_OVERRIDE");
            }
            let _ = self.prev.as_ref();
        }
    }

    /// Minimal `TurnCompleter` that replays a scripted sequence of
    /// outcomes, mirroring the mock in `agent_loop` tests.
    struct MockCompleter {
        script: Mutex<std::collections::VecDeque<Result<TurnOutcome, AgentTurnError>>>,
    }
    impl MockCompleter {
        fn new(script: Vec<Result<TurnOutcome, AgentTurnError>>) -> Self {
            Self {
                script: Mutex::new(script.into()),
            }
        }
    }
    impl crate::agent_loop::TurnCompleter for MockCompleter {
        fn complete(
            &self,
            _request: ToolChatRequest,
            _tools: Vec<ToolSpec>,
            _text_cb: Box<dyn Fn(&str) + Send>,
            _reasoning_cb: Box<dyn Fn(&str) + Send>,
            _cancel_rx: oneshot::Receiver<()>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<TurnOutcome, AgentTurnError>> + Send>,
        > {
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
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<String, AgentTurnError>> + Send>,
        > {
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

    fn text_outcome(text: &str) -> Result<TurnOutcome, AgentTurnError> {
        Ok(TurnOutcome {
            text: text.into(),
            reasoning: String::new(),
            tool_calls: Vec::new(),
            usage: ChatUsage {
                input: 1,
                output: 1,
            },
        })
    }

    /// Orchestrator context carrying a `SubagentDispatchContext` whose
    /// subagent turns run against the given mock completer.
    fn orchestrator_dispatch_ctx(
        workspace: &str,
        sink: &std::sync::Arc<TestSink>,
        completer: std::sync::Arc<MockCompleter>,
    ) -> (ToolContext, SubagentDispatchContext) {
        let dispatch = SubagentDispatchContext {
            subagents: Arc::new(DashMap::new()),
            inbox: Arc::new(DashMap::new()),
            completer,
            pending_approvals: Arc::new(DashMap::new()),
            in_flight: Arc::new(DashMap::new()),
            sink: sink.clone(),
            workspace: workspace.to_string(),
            mode: CommandMode::Yolo,
            reasoning_effort: None,
            chat_model_id: "test-model".into(),
            request_id: "parent-1".into(),
            goal_max_turns: crate::personalities::DEFAULT_GOAL_MAX_TURNS,
            subagent_semaphore: Arc::new(tokio::sync::Semaphore::new(
                crate::personalities::DEFAULT_MAX_CONCURRENT_SUBAGENTS as usize,
            )),
            dispatch_call_index: Arc::new(DashMap::new()),
            subagent_cancel: Arc::new(DashMap::new()),
        };
        let ctx = ToolContext {
            workspace: workspace.to_string(),
            auto_allowed: true,
            caller_role: AgentRole::Orchestrator,
            sink: sink.clone() as Arc<dyn AgentEventSink>,
            subagent: Some(dispatch.clone()),
            session_id: None,
        };
        (ctx, dispatch)
    }

    #[tokio::test]
    async fn delegate_subagent_refuses_non_orchestrator() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let (ctx, _) = ctx_for(&w, AgentRole::Coder);
        let res = execute_tool(
            &ctx,
            "delegate_subagent",
            &json!({"personality": "coder", "task": "x"}),
        )
        .await
        .unwrap();
        assert!(!res.ok);
        assert!(res.summary.contains("Orchestrator"));
    }

    #[tokio::test]
    async fn delegate_subagent_rejects_unknown_target() {
        let _g = DataDirGuard::new();
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let sink = Arc::new(TestSink::default());
        let (ctx, _) =
            orchestrator_dispatch_ctx(&w, &sink, Arc::new(MockCompleter::new(Vec::new())));
        let res = execute_tool(
            &ctx,
            "delegate_subagent",
            &json!({"personality": "nope", "task": "x"}),
        )
        .await
        .unwrap();
        assert!(!res.ok);
        assert!(res.summary.contains("not a roster personality"));
    }

    #[tokio::test]
    async fn delegate_subagent_returns_queued_summary() {
        // Spec `2026-08-14--parallel-subagent-dispatch`: the executor
        // no longer awaits the subagent turn. It inserts a `Working`
        // row, indexes the call id, and returns a queued summary
        // immediately. The actual turn is driven by
        // `agent_loop::execute_tool_batch`. The end-to-end "registry
        // reaches Done" assertion moved to `agent_loop::tests`.
        let _g = DataDirGuard::new();
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let sink = Arc::new(TestSink::default());
        let completer = Arc::new(MockCompleter::new(vec![text_outcome("final answer here")]));
        let (ctx, dispatch) = orchestrator_dispatch_ctx(&w, &sink, completer);

        let res = execute_tool_with_id(
            &ctx,
            "delegate_subagent",
            &json!({"personality": "coder", "task": "fix the bug"}),
            "call-1",
        )
        .await
        .unwrap();
        assert!(res.ok, "summary: {}", res.summary);
        assert!(res.summary.contains("queued"));
        assert!(res.content.is_none());

        // Registry: one `Working` row, indexed by the call id we
        // passed in. Singular tool → Vec of length 1.
        assert_eq!(dispatch.subagents.len(), 1);
        let rec = dispatch.subagents.iter().next().unwrap().value().clone();
        assert_eq!(rec.status, crate::agent::SubagentStatus::Working);
        assert_eq!(rec.role, AgentRole::Coder);
        assert_eq!(rec.parent_request_id.as_deref(), Some("parent-1"));
        let (idx_call, idx_subs) = dispatch
            .dispatch_call_index
            .iter()
            .next()
            .map(|e| (e.key().clone(), e.value().clone()))
            .unwrap();
        assert_eq!(idx_call, "call-1");
        assert_eq!(idx_subs.len(), 1);
        assert!(idx_subs[0].starts_with("sub-"));
    }

    #[tokio::test]
    async fn delegate_subagents_refuses_non_orchestrator() {
        // Spec `2026-08-14--plural-subagent-dispatch`: plural tool is
        // hard-gated to the Orchestrator, same as the singular tool.
        // Subagent recursion stays impossible through either path.
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let (ctx, _) = ctx_for(&w, AgentRole::Coder);
        let res = execute_tool(
            &ctx,
            "delegate_subagents",
            &json!({"dispatches": [{"personality": "coder", "task": "x"}]}),
        )
        .await
        .unwrap();
        assert!(!res.ok);
        assert!(res.summary.contains("Orchestrator"));
    }

    #[tokio::test]
    async fn delegate_subagents_rejects_empty_or_missing_array() {
        // Spec `2026-08-14--plural-subagent-dispatch`: a `dispatches`
        // that is missing, not an array, or empty is a per-call error
        // — never a silent no-op that spawns 0 subagents.
        let _g = DataDirGuard::new();
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let sink = Arc::new(TestSink::default());
        let (ctx, _) =
            orchestrator_dispatch_ctx(&w, &sink, Arc::new(MockCompleter::new(Vec::new())));

        // Missing `dispatches`.
        let res = execute_tool_with_id(&ctx, "delegate_subagents", &json!({}), "call-missing")
            .await
            .unwrap();
        assert!(!res.ok);
        assert!(res.summary.contains("non-empty array"));

        // Empty array.
        let res = execute_tool_with_id(
            &ctx,
            "delegate_subagents",
            &json!({"dispatches": []}),
            "call-empty",
        )
        .await
        .unwrap();
        assert!(!res.ok);
        assert!(res.summary.contains("at least one entry"));

        // Wrong type.
        let res = execute_tool_with_id(
            &ctx,
            "delegate_subagents",
            &json!({"dispatches": "not-an-array"}),
            "call-wrong-type",
        )
        .await
        .unwrap();
        assert!(!res.ok);
        assert!(res.summary.contains("non-empty array"));
    }

    #[tokio::test]
    async fn delegate_subagents_registers_all_dispatches_under_one_call_id() {
        // Spec `2026-08-14--plural-subagent-dispatch`: one tool call
        // → N subagent rows in the registry, all indexed under the
        // same call id. The batch dispatcher then spawns N
        // concurrent subagent turns under the semaphore cap.
        let _g = DataDirGuard::new();
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let sink = Arc::new(TestSink::default());
        let completer = Arc::new(MockCompleter::new(vec![text_outcome("done")]));
        let (ctx, dispatch) = orchestrator_dispatch_ctx(&w, &sink, completer);

        let res = execute_tool_with_id(
            &ctx,
            "delegate_subagents",
            &json!({
                "dispatches": [
                    {"personality": "coder", "task": "fix the bug"},
                    {"personality": "architect", "task": "review the design"},
                    {"personality": "researcher", "task": "summarize the spec"}
                ]
            }),
            "call-plural",
        )
        .await
        .unwrap();
        assert!(res.ok, "summary: {}", res.summary);
        assert!(res.summary.contains("queued 3 subagents"));
        assert!(res.content.is_none());

        // Registry: 3 Working rows, all indexed under the same call id.
        assert_eq!(dispatch.subagents.len(), 3);
        let mut roles: Vec<AgentRole> = dispatch.subagents.iter().map(|e| e.value().role).collect();
        roles.sort_by_key(|r| format!("{:?}", r));
        assert_eq!(
            roles,
            vec![
                AgentRole::Architect,
                AgentRole::Coder,
                AgentRole::Researcher
            ]
        );

        // dispatch_call_index: one entry per call id, holding a
        // Vec of 3 subagent ids in the order they were registered.
        let subs = dispatch
            .dispatch_call_index
            .get("call-plural")
            .expect("call-plural should be in the index")
            .value()
            .clone();
        assert_eq!(subs.len(), 3);
        for s in &subs {
            assert!(s.starts_with("sub-"));
        }
    }

    #[tokio::test]
    async fn delegate_subagents_partially_succeeds_with_per_entry_failures() {
        // Spec `2026-08-14--plural-subagent-dispatch`: per-entry
        // validation failures (unknown target, missing/empty
        // personality/task) are collected and reported in the queued
        // summary, but valid entries in the same batch still get
        // registered. Only when **every** entry fails does the tool
        // return `ok: false`.
        let _g = DataDirGuard::new();
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let sink = Arc::new(TestSink::default());
        let (ctx, dispatch) =
            orchestrator_dispatch_ctx(&w, &sink, Arc::new(MockCompleter::new(Vec::new())));

        let res = execute_tool_with_id(
            &ctx,
            "delegate_subagents",
            &json!({
                "dispatches": [
                    {"personality": "coder", "task": "valid 1"},
                    {"personality": "ghost", "task": "unknown target"},
                    {"personality": "architect", "task": "valid 2"},
                    {"personality": "coder", "task": ""}
                ]
            }),
            "call-mixed",
        )
        .await
        .unwrap();
        assert!(res.ok, "summary: {}", res.summary);
        assert!(
            res.summary.contains("queued 2 subagents"),
            "summary was: {}",
            res.summary
        );
        assert!(res.summary.contains("skipped 2"));

        // Two real subagent rows registered (the two valid entries).
        assert_eq!(dispatch.subagents.len(), 2);
    }

    #[tokio::test]
    async fn delegate_subagents_fails_when_every_entry_is_invalid() {
        // Spec `2026-08-14--plural-subagent-dispatch`: if every entry
        // fails validation, the tool returns `ok: false` with a
        // summary explaining why — never `ok: true, queued 0`.
        let _g = DataDirGuard::new();
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let sink = Arc::new(TestSink::default());
        let (ctx, dispatch) =
            orchestrator_dispatch_ctx(&w, &sink, Arc::new(MockCompleter::new(Vec::new())));

        let res = execute_tool_with_id(
            &ctx,
            "delegate_subagents",
            &json!({
                "dispatches": [
                    {"personality": "ghost-1", "task": "x"},
                    {"personality": "ghost-2", "task": "y"}
                ]
            }),
            "call-allbad",
        )
        .await
        .unwrap();
        assert!(!res.ok);
        assert!(res.summary.contains("no dispatches registered"));
        // Nothing landed in the registry.
        assert_eq!(dispatch.subagents.len(), 0);
        // Index is clean (we never inserted anything for this call id).
        assert!(dispatch.dispatch_call_index.get("call-allbad").is_none());
    }

    #[tokio::test]
    async fn subagent_message_refuses_non_health_caller() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let sink = Arc::new(TestSink::default());
        let (mut ctx, _) =
            orchestrator_dispatch_ctx(&w, &sink, Arc::new(MockCompleter::new(Vec::new())));
        ctx.caller_role = AgentRole::Coder;
        let res = execute_tool(
            &ctx,
            "subagent_message",
            &json!({"request_id": "sub-1", "message": "hi"}),
        )
        .await
        .unwrap();
        assert!(!res.ok);
        assert!(res.summary.contains("Health Agent"));
        assert!(sink.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn subagent_message_queues_and_emits_for_health_caller() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let sink = Arc::new(TestSink::default());
        let (ctx, dispatch) =
            orchestrator_dispatch_ctx(&w, &sink, Arc::new(MockCompleter::new(Vec::new())));
        dispatch
            .inbox
            .insert("sub-1".into(), TokioMutex::new(Vec::new()));

        let res = execute_tool(
            &ctx,
            "subagent_message",
            &json!({"request_id": "sub-1", "message": "watch out"}),
        )
        .await
        .unwrap();
        assert!(res.ok, "summary: {}", res.summary);

        // The message landed in the inbox and the event was broadcast.
        let inbox = dispatch.inbox.get("sub-1").unwrap();
        assert_eq!(inbox.value().lock().await.as_slice(), &["watch out"]);
        let events = sink.events.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "subagent-message");
        assert_eq!(events[0].1["requestId"], "sub-1");
        assert_eq!(events[0].1["message"], "watch out");
    }

    // -- Todo list (spec 2026-08-14--agent-todo-list.md) tests --
    //
    // The todo tool handlers do their own load+save+emit (with the
    // per-session write lock shared with the user IPC), and the agent
    // loop is expected to re-read the session on the next persist to
    // pick up the change. The unit tests here exercise the validator
    // and apply helpers (in `crate::todos`); the load+save+emit
    // pipeline is best covered by an integration test against a real
    // workspace + session file. The Rust unit tests in `todos.rs`
    // cover the pure helpers; the `tools.rs` tests cover the
    // dispatch arm wiring (above) and the spec-shaped JSON schemas.

    #[tokio::test]
    async fn subagent_message_unknown_target_fails() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let sink = Arc::new(TestSink::default());
        let (ctx, _) =
            orchestrator_dispatch_ctx(&w, &sink, Arc::new(MockCompleter::new(Vec::new())));
        let res = execute_tool(
            &ctx,
            "subagent_message",
            &json!({"request_id": "sub-9", "message": "hi"}),
        )
        .await
        .unwrap();
        assert!(!res.ok);
        assert!(res.summary.contains("no active subagent"));
        assert!(sink.events.lock().unwrap().is_empty());
    }
}

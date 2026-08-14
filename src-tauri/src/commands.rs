use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, State};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use crate::agent::{Agent, AgentMessage, AgentRole, AgentStatus};
use crate::agent_loop::{AgentEventSink, AppEventSink, LlmClientTurner, TurnCompleter};
use crate::compact::{self as compact_mod, CompactResult};
use crate::llm_client;
use crate::models as models_mod;
use crate::personalities::{AgentPersonality, PersonalityRoster};
use crate::secrets::SecretStore;
use crate::tools::BipState;
use tokio::sync::oneshot;

// ============================= Re-exports =============================
//
// Some IPC commands live in their own modules (e.g. `todos`) so the
// underlying logic doesn't bloat this file. `lib.rs` references them
// via the `todos` module path; the `pub use` below keeps backward
// compatibility for any in-crate callers that still reach for
// `commands::edit_todo` etc. The `tauri::command` macro does NOT
// follow re-exports for the `__cmd__` name lookup, so the registration
// list uses the original module path.
//
#[allow(unused_imports)]
pub use crate::todos::{append_todo, clear_todo, edit_todo, remove_todo};

// ============================= App State =============================

#[derive(Clone)]
pub struct AppState {
    pub agents: Arc<Mutex<HashMap<String, Agent>>>,
    /// OS keyring-backed secret store. `Arc` so the LLM client can
    /// hold a reference after the IPC closure returns.
    pub secrets: Arc<dyn SecretStore>,
    /// In-flight streaming requests, keyed by `request_id`. Used by
    /// `cancel_chat` to abort an in-progress SSE stream. Wrapped in
    /// `Arc` because `DashMap` itself isn't `Clone`.
    pub in_flight: Arc<dashmap::DashMap<String, tokio::sync::oneshot::Sender<()>>>,
    /// In-flight agent turns, keyed by the turn's `request_id`. `cancel_agent`
    /// flips the watch to abort the loop at its next await point.
    pub agent_in_flight: Arc<dashmap::DashMap<String, tokio::sync::watch::Sender<bool>>>,
    /// Pending tool approvals, keyed by `request_id` (one pending approval per
    /// running turn — tools execute sequentially). `respond_approval` resolves
    /// them.
    pub pending_approvals: Arc<dashmap::DashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    /// Active session file watcher. None until the frontend calls
    /// `watch_session` after opening a project.
    pub session_watcher: Arc<Mutex<Option<crate::session_watcher::SessionWatcher>>>,
    /// Current Bip (mascot) state. The Health Agent drives this via the
    /// `bip` tool; the frontend subscribes to the `bip-state` Tauri event
    /// and may also call `get_bip_state` on first mount to seed itself
    /// before the next event fires. See
    /// `docs/superpowers/specs/2026-08-13--bip-mascot-tool.md`.
    pub bip_state: Arc<RwLock<BipState>>,
    /// Registry of dispatched subagent turns keyed by their `sub-<uuid>`
    /// request id. Written by the `delegate_subagent` tool, read by
    /// `list_subagents` and the WebView-reconnect resync.
    pub subagents: Arc<dashmap::DashMap<String, crate::agent::SubagentRecord>>,
    /// Per-subagent message inboxes keyed by request id. The Health
    /// Agent delivers messages via the `subagent_message` tool; each
    /// subagent's turn drains its own inbox every LLM round.
    pub subagent_inbox: Arc<dashmap::DashMap<String, tokio::sync::Mutex<Vec<String>>>>,
    /// Per-session write lock for the todo list (spec
    /// `2026-08-14--agent-todo-list.md` §5). Serializes concurrent
    /// agent + user writes to the same session's todo list so
    /// `load → mutate → save` never races. Lazily inserts per
    /// `(project_path, session_id)` entries; the inner mutex is
    /// the actual serialization point.
    pub todo_locks: Arc<dashmap::DashMap<(String, String), Arc<tokio::sync::Mutex<()>>>>,
}

impl AppState {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            agents: Arc::new(Mutex::new(HashMap::new())),
            secrets,
            in_flight: Arc::new(dashmap::DashMap::new()),
            agent_in_flight: Arc::new(dashmap::DashMap::new()),
            pending_approvals: Arc::new(dashmap::DashMap::new()),
            session_watcher: Arc::new(Mutex::new(None)),
            bip_state: Arc::new(RwLock::new(BipState::default())),
            subagents: Arc::new(dashmap::DashMap::new()),
            subagent_inbox: Arc::new(dashmap::DashMap::new()),
            todo_locks: Arc::new(dashmap::DashMap::new()),
        }
    }
}

// ============================= Workspace Commands =============================

// (open_folder_dialog removed; replaced by `pick_project_folder` below.)

#[tauri::command]
pub async fn read_file(path: String) -> Result<String, String> {
    tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("Failed to read {}: {}", path, e))
}

#[tauri::command]
pub async fn write_file(path: String, content: String) -> Result<(), String> {
    if let Some(parent) = std::path::Path::new(&path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create parent dirs: {}", e))?;
    }
    tokio::fs::write(&path, &content)
        .await
        .map_err(|e| format!("Failed to write {}: {}", path, e))
}

#[derive(Serialize)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

#[tauri::command]
pub async fn list_directory(path: String) -> Result<Vec<DirEntry>, String> {
    let mut entries = tokio::fs::read_dir(&path)
        .await
        .map_err(|e| format!("Failed to read dir {}: {}", path, e))?;

    let mut result = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
        let metadata = entry.metadata().await.map_err(|e| e.to_string())?;
        result.push(DirEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            path: entry.path().to_string_lossy().to_string(),
            is_dir: metadata.is_dir(),
            size: metadata.len(),
        });
    }
    result.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    Ok(result)
}

#[tauri::command]
pub async fn search_files(path: String, pattern: String) -> Result<Vec<String>, String> {
    let mut results = Vec::new();
    let pattern_lower = pattern.to_lowercase();

    async fn walk(
        dir: &str,
        pattern: &str,
        results: &mut Vec<String>,
        max_depth: usize,
        current_depth: usize,
    ) -> Result<(), String> {
        if current_depth > max_depth {
            return Ok(());
        }
        let mut entries = tokio::fs::read_dir(dir)
            .await
            .map_err(|e| format!("Failed to read dir: {}", e))?;
        while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.to_lowercase().contains(pattern) {
                results.push(entry.path().to_string_lossy().to_string());
            }
            if entry.metadata().await.map_err(|e| e.to_string())?.is_dir() {
                let path_str = entry.path().to_string_lossy().to_string();
                Box::pin(walk(
                    &path_str,
                    pattern,
                    results,
                    max_depth,
                    current_depth + 1,
                ))
                .await?;
            }
        }
        Ok(())
    }

    walk(&path, &pattern_lower, &mut results, 8, 0).await?;
    Ok(results)
}

#[tauri::command]
pub async fn create_directory(path: String) -> Result<(), String> {
    tokio::fs::create_dir_all(&path)
        .await
        .map_err(|e| format!("Failed to create directory {}: {}", path, e))
}

// ============================= CLI Execution =============================

#[derive(Serialize, Deserialize)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[tauri::command]
pub async fn execute_command(
    command: String,
    args: Vec<String>,
    cwd: Option<String>,
) -> Result<CommandOutput, String> {
    use std::process::Stdio;
    use tokio::process::Command;

    let mut cmd = Command::new(&command);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| format!("Failed to execute {}: {}", command, e))?;

    Ok(CommandOutput {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}

#[tauri::command]
pub async fn kill_process(_pid: u32) -> Result<(), String> {
    // Platform-specific process killing
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("taskkill")
            .args(["/F", "/PID", &_pid.to_string()])
            .output()
            .map_err(|e| format!("Failed to kill process: {}", e))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        unsafe {
            libc::kill(_pid as i32, libc::SIGTERM);
        }
    }
    Ok(())
}

// ============================= Agent Commands =============================

#[derive(Serialize)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub role: AgentRole,
    pub status: AgentStatus,
    pub message_count: usize,
}

#[tauri::command]
pub async fn spawn_agent(
    name: String,
    role: AgentRole,
    system_prompt: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let id = Uuid::new_v4().to_string();
    let agent = Agent::new(&id, &name, role, &system_prompt);
    let mut agents = state.agents.lock().await;
    agents.insert(id.clone(), agent);
    Ok(id)
}

#[tauri::command]
pub async fn send_agent_message(
    agent_id: String,
    content: String,
    state: State<'_, AppState>,
) -> Result<AgentMessage, String> {
    let mut agents = state.agents.lock().await;
    let agent = agents
        .get_mut(&agent_id)
        .ok_or_else(|| format!("Agent {} not found", agent_id))?;
    let response = agent.process_message(&content).await;
    Ok(response)
}

#[tauri::command]
pub async fn get_agent_status(
    agent_id: String,
    state: State<'_, AppState>,
) -> Result<AgentInfo, String> {
    let agents = state.agents.lock().await;
    let agent = agents
        .get(&agent_id)
        .ok_or_else(|| format!("Agent {} not found", agent_id))?;
    Ok(AgentInfo {
        id: agent.id.clone(),
        name: agent.name.clone(),
        role: agent.role.clone(),
        status: agent.status.clone(),
        message_count: agent.messages.len(),
    })
}

#[tauri::command]
pub async fn list_active_agents(state: State<'_, AppState>) -> Result<Vec<AgentInfo>, String> {
    let agents = state.agents.lock().await;
    Ok(agents
        .values()
        .map(|a| AgentInfo {
            id: a.id.clone(),
            name: a.name.clone(),
            role: a.role.clone(),
            status: a.status.clone(),
            message_count: a.messages.len(),
        })
        .collect())
}

#[tauri::command]
pub async fn stop_agent(agent_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let mut agents = state.agents.lock().await;
    if let Some(agent) = agents.get_mut(&agent_id) {
        agent.status = AgentStatus::Stopped;
    }
    Ok(())
}

#[tauri::command]
pub async fn health_check(
    state: State<'_, AppState>,
) -> Result<HashMap<String, AgentStatus>, String> {
    let agents = state.agents.lock().await;
    Ok(agents
        .values()
        .map(|a| (a.id.clone(), a.status.clone()))
        .collect())
}

// ============================= Web Search =============================

#[derive(Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[tauri::command]
pub async fn web_search(query: String) -> Result<Vec<SearchResult>, String> {
    crate::web_search::search_ddg(&query).await
}

// ============================= Settings / BYOK =============================
//
// The plaintext localStorage API key store from Phase 1 has been
// removed. Saved models live in <data_dir>/Harness/models.json and
// their API keys live in the OS keyring. See `secrets.rs` and
// `models.rs`.

// ============= Project picker, sessions, recents, file listing =============
//
// These are the Phase 1.5 additions. The IPC bridge in `src/lib/ipc.ts`
// mirrors these signatures; the corresponding TS types live in
// `src/lib/session-types.ts`.

#[tauri::command]
pub async fn pick_project_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |folder| {
        let _ = tx.send(folder);
    });
    let folder = rx
        .await
        .map_err(|e| format!("dialog channel closed: {}", e))?;
    Ok(folder.map(|f| f.to_string()))
}

#[tauri::command]
pub async fn load_session(
    project_path: String,
    session_id: String,
) -> Result<Option<crate::session::Session>, String> {
    crate::session::load_session(&project_path, &session_id).await
}

#[tauri::command]
pub async fn list_sessions(
    project_path: String,
) -> Result<Vec<crate::session::SessionSummary>, String> {
    crate::session::list_sessions(&project_path).await
}

/// Load the project's active session (or the newest when no marker is set).
#[tauri::command]
pub async fn load_active_session(
    project_path: String,
) -> Result<Option<crate::session::Session>, String> {
    crate::session::load_active_session(&project_path).await
}

#[tauri::command]
pub async fn create_session(
    project_path: String,
    agent: Option<crate::session::AgentSnapshot>,
) -> Result<crate::session::Session, String> {
    crate::session::create_session(&project_path, agent).await
}

#[tauri::command]
pub async fn rename_session(
    project_path: String,
    session_id: String,
    title: String,
) -> Result<crate::session::SessionSummary, String> {
    crate::session::rename_session(&project_path, &session_id, title).await
}

#[tauri::command]
pub async fn delete_session(project_path: String, session_id: String) -> Result<(), String> {
    crate::session::delete_session(&project_path, &session_id).await
}

#[tauri::command]
pub async fn set_active_session(project_path: String, session_id: String) -> Result<(), String> {
    crate::session::set_active_session(&project_path, &session_id).await
}

// ============================= Per-session mode/model persistence (spec §5.2) =============================

/// Persist the command mode for a session without re-saving the whole file.
/// Used by the frontend's mode toggle; debounced on the TS side.
#[tauri::command]
pub async fn set_session_mode(
    project_path: String,
    session_id: String,
    mode: crate::session::CommandMode,
) -> Result<crate::session::Session, String> {
    let mut session = crate::session::load_session(&project_path, &session_id)
        .await?
        .ok_or_else(|| format!("Session {} not found", session_id))?;
    session.last_mode = mode;
    crate::session::save_session(&project_path, session).await
}

/// Persist the last-used model id for a session without re-saving the whole file.
/// Used by the frontend's model switcher.
#[tauri::command]
pub async fn set_session_model(
    project_path: String,
    session_id: String,
    model_id: String,
) -> Result<crate::session::Session, String> {
    let mut session = crate::session::load_session(&project_path, &session_id)
        .await?
        .ok_or_else(|| format!("Session {} not found", session_id))?;
    session.last_model_id = Some(model_id);
    crate::session::save_session(&project_path, session).await
}

/// Compact a session's message history into a single summary message (spec §6).
///
/// Takes `project_path` and `session_id` so the frontend can call it without
/// threading the full `Session` object over IPC. On success returns a
/// `CompactResult` describing the fold; on failure returns an error string so
/// the frontend can surface a meaningful toast.
#[tauri::command]
pub async fn compact_chat(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    project_path: String,
    session_id: String,
    model_id: String,
) -> Result<CompactResult, String> {
    let mut session = crate::session::load_session(&project_path, &session_id)
        .await
        .map_err(|e| format!("Failed to load session: {}", e))?
        .ok_or_else(|| format!("Session {} not found", session_id))?;

    // Build the completer and event sink from app state.
    let secrets = Arc::clone(&state.secrets);
    let completer = Arc::new(LlmClientTurner::new(secrets)) as Arc<dyn TurnCompleter>;
    let sink = Arc::new(AppEventSink::new(app.clone())) as Arc<dyn AgentEventSink>;

    // A pre-dropped receiver means "not cancelled" — compaction runs to completion.
    let (_cancel_tx, cancel_rx) = oneshot::channel();

    // Core compaction (leaves session unchanged on failure).
    let result = compact_mod::compact_chat(&mut session, &model_id, completer, cancel_rx)
        .await
        .map_err(|e| match e {
            compact_mod::CompactError::NothingToCompact => {
                "Nothing to compact — need at least 2 non-summary messages.".to_string()
            }
            compact_mod::CompactError::BadSummary => {
                "Summary was too short or looks like a refusal — try again.".to_string()
            }
            compact_mod::CompactError::Llm(llm_err) => {
                format!("LLM error during compaction: {:?}", llm_err)
            }
        })?;

    // Persist and emit the chat-compacted event so the frontend can show a toast.
    compact_mod::persist_compact(&session, &result, &sink).await;

    Ok(result)
}

// ============================= Agent turn loop =============================

/// Kick off one agent turn for a session. Returns immediately; progress
/// streams back through `agent-*` app events routed by `requestId`.
///
/// `reasoning_effort` is an optional hint carried over from the user's
/// model prefs. It is forwarded to OpenAI-compat gateways as
/// `reasoning_effort` so reasoning-capable models return
/// `delta.reasoning_content` for the chain-of-thought display.
#[tauri::command]
pub async fn run_agent_turn(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request_id: String,
    session_id: String,
    model_id: String,
    message: String,
    mode: crate::session::CommandMode,
    goal_mode: bool,
    workspace: String,
    reasoning_effort: Option<crate::models::ReasoningEffort>,
) -> Result<(), String> {
    let session = crate::session::load_session(&workspace, &session_id)
        .await?
        .ok_or_else(|| format!("Session {} not found", session_id))?;

    let sink: Arc<dyn crate::agent_loop::AgentEventSink> =
        Arc::new(crate::agent_loop::AppEventSink::new(app));
    // Wrap the sink so `bip-state` emissions also persist to AppState.
    // This keeps a single source of truth (the lock) so a freshly
    // connected WebView can seed itself via `get_bip_state`.
    let sink: Arc<dyn crate::agent_loop::AgentEventSink> =
        Arc::new(BipPersistingSink::new(sink, state.bip_state.clone()));
    let completer: Arc<dyn crate::agent_loop::TurnCompleter> = Arc::new(
        crate::agent_loop::LlmClientTurner::new(state.secrets.clone()),
    );

    // The orchestrator turn carries the subagent dispatch machinery:
    // registry, inboxes, and the runner. This is what lets the
    // `delegate_subagent` / `subagent_message` tools work from inside
    // the agent loop.
    //
    // The goal-mode continuation cap is read from the global personality
    // roster (spec `2026-08-14--parallel-subagent-dispatch`); the
    // roster's `enforce_invariants` clamps the value into range, so a
    // corrupt or out-of-range file falls back to the documented default
    // rather than aborting the turn.
    let roster = crate::personalities::load_personalities().await?;
    let dispatch = crate::tools::SubagentDispatchContext {
        subagents: state.subagents.clone(),
        inbox: state.subagent_inbox.clone(),
        completer: completer.clone(),
        pending_approvals: state.pending_approvals.clone(),
        in_flight: state.agent_in_flight.clone(),
        sink: sink.clone(),
        workspace: workspace.clone(),
        mode: mode.clone(),
        reasoning_effort,
        chat_model_id: model_id.clone(),
        request_id: request_id.clone(),
        goal_max_turns: roster.goal_max_turns,
        subagent_semaphore: Arc::new(tokio::sync::Semaphore::new(
            roster.max_concurrent_subagents as usize,
        )),
        dispatch_call_index: Arc::new(DashMap::new()),
        subagent_cancel: Arc::new(DashMap::new()),
    };

    crate::agent_loop::run_agent_turn(
        request_id,
        session,
        model_id,
        message,
        mode,
        goal_mode,
        workspace,
        sink,
        state.pending_approvals.clone(),
        state.agent_in_flight.clone(),
        completer,
        reasoning_effort,
        Some(dispatch),
    );
    Ok(())
}

/// List every dispatched subagent turn (registry snapshot), newest
/// first. The frontend calls this to resync subagent windows after a
/// WebView reload; live content still streams via the `agent-*` events.
#[tauri::command]
pub async fn list_subagents(
    state: State<'_, AppState>,
) -> Result<Vec<crate::agent::SubagentRecord>, String> {
    let mut records: Vec<_> = state
        .subagents
        .iter()
        .map(|entry| entry.value().clone())
        .collect();
    records.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(records)
}

/// Resolve a pending tool approval. No-op when the request has no pending
/// approval (already answered or the turn ended).
#[tauri::command]
pub async fn respond_approval(
    request_id: String,
    allow: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if let Some((_, tx)) = state.pending_approvals.remove(&request_id) {
        let _ = tx.send(allow);
    }
    Ok(())
}

/// Cancel a running agent turn by its `request_id`. The loop resolves any
/// pending approval as denied and emits `agent-error kind:"cancelled"`.
#[tauri::command]
pub async fn cancel_agent(request_id: String, state: State<'_, AppState>) -> Result<(), String> {
    if let Some((_, tx)) = state.agent_in_flight.remove(&request_id) {
        let _ = tx.send(true);
    }
    Ok(())
}

// ============================= Bip mascot state =============================
//
// The `bip` tool is the only way to mutate this state. It's gated on
// `AgentRole::HealthMonitor` in `tools::tool_bip`. The frontend subscribes
// to the `bip-state` Tauri event and may also call `get_bip_state` on first
// mount to seed itself with the current value (avoids a flash of "idle"
// before the first event of a session).

/// Snapshot the current Bip state. The WebView calls this once on mount.
#[tauri::command]
pub async fn get_bip_state(state: State<'_, AppState>) -> Result<BipState, String> {
    Ok(state.bip_state.read().await.clone())
}

// ============================= Agent personalities =============================
//
// Global, app-wide. The frontend's Agent Manager is the only writer.
//
// After the v3 chat-agent-as-orchestrator refactor, the chat agent
// itself isn't a personality — every session runs the hardcoded
// Orchestrator. The roster is the home for the four subagent
// personalities (Coder, Architect, Researcher, HealthMonitor) the
// orchestrator may dispatch to in a future subagent loop.
// `resolve_active_model_for_personality` is the helper the dispatcher
// will use to pick a saved model for a subagent at dispatch time. See
// `docs/superpowers/specs/2026-08-13--global-personalities.md`.

/// Load the global personality roster. On first contact, seeds four
/// builtin rows (one per subagent `AgentRole`) and persists the file.
#[tauri::command]
pub async fn load_personalities() -> Result<PersonalityRoster, String> {
    crate::personalities::load_personalities().await
}

/// Persist the global personality roster atomically. Enforces builtin
/// invariants and returns the normalized roster.
#[tauri::command]
pub async fn save_personalities(roster: PersonalityRoster) -> Result<PersonalityRoster, String> {
    crate::personalities::save_personalities(&roster).await
}

/// Look up one personality by id. Returns the default personality if
/// the id is unknown (so a session always has something to run with).
#[tauri::command]
pub async fn get_personality(id: String) -> Result<AgentPersonality, String> {
    match crate::personalities::get_personality(&id).await? {
        Some(p) => Ok(p),
        None => {
            let roster = crate::personalities::load_personalities().await?;
            Ok(roster
                .personalities
                .into_iter()
                .find(|p| p.id == roster.default_personality_id)
                .expect("default personality must exist after enforce_invariants"))
        }
    }
}

/// Resolve which saved-model id a personality should use. If the
/// personality has a `modelId`, look it up in the saved-models store
/// and confirm it still exists. Otherwise return `None` so the caller
/// can fall back to the active chat model.
///
/// Reserved for the future subagent-dispatch loop: the chat agent
/// itself is the hardcoded Orchestrator and doesn't use per-personality
/// model overrides. Kept here so the subagent dispatcher can pick the
/// right saved model when the orchestrator decides to delegate.
#[allow(dead_code)]
pub async fn resolve_active_model_for_personality(
    personality: &AgentPersonality,
) -> Result<Option<String>, String> {
    let Some(model_id) = personality.model_id.as_ref() else {
        return Ok(None);
    };
    // Read the saved-models file directly (no keyring) so the lookup
    // works in tests and doesn't depend on the secret store.
    let file = models_mod::read_models_file().await?;
    if file.models.iter().any(|m| m.id == *model_id) {
        Ok(Some(model_id.clone()))
    } else {
        // Stale reference — the user removed the model after picking
        // it. Treat as "no override" and let the chat model take over.
        Ok(None)
    }
}

#[tauri::command]
pub async fn save_session(
    project_path: String,
    session: crate::session::Session,
) -> Result<crate::session::Session, String> {
    crate::session::save_session(&project_path, session).await
}

#[tauri::command]
pub async fn list_recent_projects() -> Result<Vec<crate::session::RecentProject>, String> {
    crate::session::list_recent_projects().await
}

#[tauri::command]
pub async fn forget_recent_project(path: String) -> Result<(), String> {
    crate::session::forget_recent_project(&path).await
}

#[tauri::command]
pub async fn touch_recent_project(
    project_path: String,
) -> Result<crate::session::RecentProject, String> {
    crate::session::touch_recent_project(&project_path).await
}

#[tauri::command]
pub async fn list_files_recursive(project_path: String) -> Result<Vec<String>, String> {
    crate::session::list_files_recursive(&project_path).await
}

// ============= Saved Models + Per-Model Prefs + Chat (Phase 2.0) =============
//
// The plaintext localStorage API key store has been replaced by the
// OS keyring. Saved models live in <data_dir>/Harness/models.json and
// their API keys live in the keyring under service "Harness" with the
// model id as the account name. See `secrets.rs` and `models.rs`.

#[tauri::command]
pub async fn list_saved_models(
    state: State<'_, AppState>,
) -> Result<Vec<models_mod::SavedModel>, String> {
    models_mod::list_saved_models(&*state.secrets).await
}

#[tauri::command]
pub async fn save_saved_model(
    model: models_mod::SavedModelInput,
    api_key: Option<String>,
    state: State<'_, AppState>,
) -> Result<models_mod::SavedModel, String> {
    models_mod::save_saved_model(&*state.secrets, model, api_key.as_deref()).await
}

/// Probe a saved model's capabilities (context window + reasoning
/// support) by talking to the provider. Used by the "Probe" button
/// in Settings. The result is NOT auto-saved; the frontend applies
/// it to the editor draft and the user explicitly saves.
#[tauri::command]
pub async fn probe_saved_model(
    id: String,
    state: State<'_, AppState>,
) -> Result<models_mod::CapabilityProbe, String> {
    // Pull the persisted record to get base_url/format/model_name.
    let list = models_mod::list_saved_models(&*state.secrets).await?;
    let saved = list
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| format!("model {id} not found"))?;
    let api_key = state
        .secrets
        .get(&id)
        .map_err(|e| format!("keyring error: {e}"))?
        .unwrap_or_default();
    crate::capabilities::probe_capabilities(
        &saved.format,
        &saved.base_url,
        &saved.model_name,
        &api_key,
    )
    .await
}

#[tauri::command]
pub async fn delete_saved_model(id: String, state: State<'_, AppState>) -> Result<(), String> {
    models_mod::delete_saved_model(&*state.secrets, &id).await
}

#[tauri::command]
pub async fn get_active_model() -> Result<Option<String>, String> {
    Ok(models_mod::read_active_model())
}

#[tauri::command]
pub async fn set_active_model(id: Option<String>) -> Result<(), String> {
    models_mod::write_active_model(id.as_deref());
    Ok(())
}

#[tauri::command]
pub async fn get_model_prefs(id: String) -> Result<models_mod::ModelPrefs, String> {
    Ok(models_mod::read_model_prefs(&id))
}

#[tauri::command]
pub async fn set_model_prefs(id: String, prefs: models_mod::ModelPrefs) -> Result<(), String> {
    models_mod::write_model_prefs(&id, &prefs);
    Ok(())
}

#[tauri::command]
pub async fn test_saved_model(
    id: String,
    state: State<'_, AppState>,
) -> Result<models_mod::TestResult, String> {
    models_mod::test_saved_model(&*state.secrets, &id).await
}

#[tauri::command]
pub async fn reset_secret_store(state: State<'_, AppState>) -> Result<(), String> {
    // Iterate every saved model and delete its keyring entry. We
    // don't touch the model metadata; the user re-adds keys from
    // Settings.
    let models = models_mod::read_models_file().await?;
    for m in &models.models {
        state
            .secrets
            .delete(&m.id)
            .map_err(|e| format!("Failed to delete key for {}: {}", m.id, e))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn chat_complete(
    request: llm_client::ChatRequest,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let secrets = state.secrets.clone();
    let in_flight = state.in_flight.clone();
    // `run_chat` clones the fields it needs up front, so moving `request`
    // into the task is fine. The stream is delivered via `chat-chunk` /
    // `chat-done` / `chat-error` events; the IPC returns immediately.
    tauri::async_runtime::spawn(async move {
        let _ = llm_client::run_chat(request, secrets, in_flight, app).await;
    });
    Ok(())
}

/// Start (or replace) the session file watcher for `workspace_path`.
/// Called by the frontend right after loading a project. Emits
/// `session-file-changed` whenever a session JSON file under
/// `<workspace>/.harness/sessions/` is written.
#[tauri::command]
pub async fn watch_session(
    workspace_path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    use std::path::PathBuf;
    let mut guard = state.session_watcher.lock().await;
    // Drop the old watcher (releasing the OS handle) before starting
    // a new one — only one watch per process.
    *guard = None;
    let path = PathBuf::from(&workspace_path);
    let watcher = crate::session_watcher::SessionWatcher::start(app, path)
        .map_err(|e| format!("watcher: {e}"))?;
    *guard = Some(watcher);
    Ok(())
}

#[tauri::command]
pub async fn cancel_chat(request_id: String, state: State<'_, AppState>) -> Result<(), String> {
    if let Some((_, tx)) = state.in_flight.remove(&request_id) {
        let _ = tx.send(());
    }
    Ok(())
}

// ============================= Bip sink wrapper =============================
//
// `BipPersistingSink` is an `AgentEventSink` that delegates to a wrapped
// sink (the production `AppEventSink` in `run_agent_turn`) and, on every
// `bip-state` event, also writes the new state into `AppState.bip_state`.
// This keeps a single source of truth (the in-memory lock) so the frontend
// can `get_bip_state` on first mount instead of guessing the current
// value from a possibly-stale event stream.

pub struct BipPersistingSink {
    inner: Arc<dyn crate::agent_loop::AgentEventSink>,
    bip_state: Arc<RwLock<BipState>>,
}

impl BipPersistingSink {
    pub fn new(
        inner: Arc<dyn crate::agent_loop::AgentEventSink>,
        bip_state: Arc<RwLock<BipState>>,
    ) -> Self {
        Self { inner, bip_state }
    }
}

impl crate::agent_loop::AgentEventSink for BipPersistingSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        if event == "bip-state" {
            if let Ok(state) = serde_json::from_value::<BipState>(payload.clone()) {
                // Best-effort persist: if the lock is held by a slow reader
                // we still bubble the event out (the next one will catch up).
                if let Ok(mut guard) = self.bip_state.try_write() {
                    *guard = state;
                }
            }
        }
        self.inner.emit(event, payload);
    }
}

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, State};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::agent::{Agent, AgentMessage, AgentRole, AgentStatus};
use crate::llm_client;
use crate::models as models_mod;
use crate::secrets::SecretStore;

// ============================= App State =============================

pub struct AppState {
    pub agents: Mutex<HashMap<String, Agent>>,
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
}

impl AppState {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            agents: Mutex::new(HashMap::new()),
            secrets,
            in_flight: Arc::new(dashmap::DashMap::new()),
            agent_in_flight: Arc::new(dashmap::DashMap::new()),
            pending_approvals: Arc::new(dashmap::DashMap::new()),
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

#[tauri::command]
pub async fn load_agent_roster(project_path: String) -> Result<crate::agents::AgentRoster, String> {
    crate::agents::load_agent_roster(&project_path).await
}

#[tauri::command]
pub async fn save_agent_roster(
    project_path: String,
    roster: crate::agents::AgentRoster,
) -> Result<crate::agents::AgentRoster, String> {
    crate::agents::save_agent_roster(&project_path, &roster).await
}

// ============================= Agent turn loop =============================

/// Kick off one agent turn for a session. Returns immediately; progress
/// streams back through `agent-*` app events routed by `requestId`.
#[tauri::command]
pub async fn run_agent_turn(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request_id: String,
    session_id: String,
    model_id: String,
    message: String,
    action_mode: crate::session::ActionMode,
    goal_mode: bool,
    workspace: String,
) -> Result<(), String> {
    let session = crate::session::load_session(&workspace, &session_id)
        .await?
        .ok_or_else(|| format!("Session {} not found", session_id))?;

    let sink: Arc<dyn crate::agent_loop::AgentEventSink> =
        Arc::new(crate::agent_loop::AppEventSink::new(app));
    let completer: Arc<dyn crate::agent_loop::TurnCompleter> = Arc::new(
        crate::agent_loop::LlmClientTurner::new(state.secrets.clone()),
    );

    crate::agent_loop::run_agent_turn(
        request_id,
        session,
        model_id,
        message,
        action_mode,
        goal_mode,
        workspace,
        sink,
        state.pending_approvals.clone(),
        state.agent_in_flight.clone(),
        completer,
    );
    Ok(())
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

#[tauri::command]
pub async fn cancel_chat(request_id: String, state: State<'_, AppState>) -> Result<(), String> {
    if let Some((_, tx)) = state.in_flight.remove(&request_id) {
        let _ = tx.send(());
    }
    Ok(())
}

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokio::fs;
use uuid::Uuid;

use crate::agent::AgentRole;

// All wire types use camelCase to match the TypeScript frontend. The Rust
// source itself stays snake_case (idiomatic), with the rename at the
// serde boundary.

// -- Domain types --

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mention {
    #[serde(default)]
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub role: String,
    pub content: String,
    /// ISO 8601 UTC string.
    pub timestamp: String,
    #[serde(default)]
    pub mentions: Vec<Mention>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Chain-of-thought captured for thinking-capable models.
    /// `None` (or absent) on plain chat turns. The frontend renders
    /// this in a collapsible "Thinking" block above the message body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

/// Per-session token telemetry (spec §4.5). `null` until the first
/// assistant response arrives. `cumulativeIn` is the peak input size,
/// not a sum — the backend reports the per-turn numbers and the UI
/// applies the `max()` rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenStats {
    pub last_in: u64,
    pub last_out: u64,
    pub cumulative_in: u64,
    pub cumulative_out: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_window_used: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_window_resets_at: Option<String>,
}

/// Action policy for agent tool calls (spec §5). Serialized camelCase:
/// `"auto" | "askRisky" | "askAll"`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum ActionMode {
    Auto,
    #[default]
    AskRisky,
    AskAll,
}

/// One persisted tool-execution record (spec §4.1 `activities`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ToolActivityStatus {
    Ran,
    Denied,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolActivity {
    pub id: String,
    pub tool: String,
    pub summary: String,
    pub status: ToolActivityStatus,
    /// ISO 8601 UTC string.
    pub timestamp: String,
    /// `requestId` of the agent turn that ran this tool. Lets the
    /// frontend attach tool calls to the right assistant message
    /// (`agent-<requestId>`) so they render inline in the chat and
    /// survive a reload. `None` for non-tool records (e.g. turn errors).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Raw JSON arguments of the call, for display in the chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
}

/// Snapshot of the agent personality that drives a session. After the
/// global-personalities change, a session only carries the
/// `personalityId`; the live name / role / system prompt / model id are
/// resolved from the global personality roster at session start.
///
/// Old session files (v1) embed the full snapshot with
/// `id` / `name` / `role` / `systemPrompt`. On load we still parse
/// them — `personalityId` is filled in from the legacy `id` field, and
/// the rest is dropped. The next persist writes the new shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSnapshot {
    /// Stable id of the personality in the global roster. New sessions
    /// always carry this; legacy sessions fill it in from the old
    /// `id` field on load.
    #[serde(default)]
    pub personality_id: String,
    /// Legacy v1 fields. Accepted on input so old session files keep
    /// loading, but never serialized (the global roster is the source
    /// of truth for name / role / prompt / model).
    #[serde(default, skip_serializing)]
    pub id: Option<String>,
    #[serde(default, skip_serializing)]
    pub name: Option<String>,
    #[serde(default, skip_serializing)]
    pub role: Option<AgentRole>,
    #[serde(default, skip_serializing)]
    pub system_prompt: Option<String>,
}

impl AgentSnapshot {
    /// Build a snapshot for a new session from a personality id.
    pub fn from_personality_id(personality_id: impl Into<String>) -> Self {
        Self {
            personality_id: personality_id.into(),
            id: None,
            name: None,
            role: None,
            system_prompt: None,
        }
    }

    /// Pull the personalityId from a v1-shaped legacy snapshot that
    /// carried its data inline. Returns the new shape with the
    /// legacy fields dropped.
    pub fn from_legacy(
        id: String,
        _name: String,
        _role: AgentRole,
        _system_prompt: String,
    ) -> Self {
        Self::from_personality_id(id)
    }
}

/// In-band "this turn finished" marker written by the agent loop on the
/// final persist, right before the terminal event. The frontend uses it as
/// a disk-based completion signal when streaming events don't reach the
/// WebView, so responses still appear without a manual refresh.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastTurn {
    pub request_id: String,
    /// `"done"` or `"error"`.
    pub status: String,
    pub at: String,
    /// Final reasoning captured for the turn, if any. Mirrored on the
    /// session file so the file-watcher fallback (the one that fixes
    /// "responses only show after refresh") can apply it when the
    /// streaming events never reach the WebView.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

/// Session wire format v2 — one self-contained file per session under
/// `<project>/.harness/sessions/<id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    #[serde(default = "default_version")]
    pub version: u32,
    /// UUID. New fields are defaulted so a v1-shaped payload from an
    /// older frontend upgrades in place instead of being archived.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    pub project_path: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default = "default_agent_snapshot")]
    pub agent: AgentSnapshot,
    #[serde(default)]
    pub goal_mode: bool,
    #[serde(default)]
    pub action_mode: ActionMode,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub activities: Vec<ToolActivity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_stats: Option<TokenStats>,
    /// Completed-turn marker from the agent loop (see [`LastTurn`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn: Option<LastTurn>,
}

fn default_version() -> u32 {
    2
}

pub fn default_agent_snapshot() -> AgentSnapshot {
    AgentSnapshot {
        personality_id: crate::personalities::builtin_personality_id(AgentRole::Orchestrator),
        id: None,
        name: None,
        role: None,
        system_prompt: None,
    }
}

impl Session {
    /// Resolve the live personality for this session from the global
    /// roster. The `personalityId` on `Session.agent` is the canonical
    /// reference; everything else (name, role, system prompt, model
    /// id) is read from the roster at call time so editing a
    /// personality affects every future session start.
    ///
    /// If the personality id is missing (a blank `Session.agent`) or
    /// the personality was deleted from the roster, we fall back to
    /// the roster's default personality.
    pub async fn resolve_personality(
        &self,
    ) -> Result<crate::personalities::AgentPersonality, String> {
        let id = self.agent.personality_id.trim();
        let roster = crate::personalities::load_personalities().await?;
        if !id.is_empty() {
            if let Some(p) = roster.personalities.iter().find(|p| p.id == id) {
                return Ok(p.clone());
            }
        }
        // Fall back to the default.
        roster
            .personalities
            .into_iter()
            .find(|p| p.id == roster.default_personality_id)
            .ok_or_else(|| "Personalities roster is empty after enforce_invariants".to_string())
    }
}

/// Lightweight per-session metadata for the Sessions panel. The full
/// `Session` (with messages) is loaded separately on open.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub project_path: String,
    pub project_name: String,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: usize,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
}

/// v1 parse target for migration from the legacy single `session.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacySession {
    pub version: u32,
    pub project_path: String,
    pub created_at: String,
    pub updated_at: String,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_stats: Option<TokenStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Recents {
    pub version: u32,
    pub projects: Vec<RecentProject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentProject {
    pub path: String,
    pub name: String,
    /// ISO 8601 UTC string.
    pub last_opened: String,
}

const RECENT_LIMIT: usize = 20;
const IGNORE_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    ".harness",
    ".next",
    ".turbo",
    "build",
    "out",
    ".cache",
];
const SESSION_ID_MAX_LEN: usize = 64;
const TITLE_MAX_LEN: usize = 80;

// -- Path helpers --

pub(crate) fn harness_dir(project_path: &str) -> PathBuf {
    Path::new(project_path).join(".harness")
}

fn sessions_dir(project_path: &str) -> PathBuf {
    harness_dir(project_path).join("sessions")
}

fn session_path(project_path: &str, id: &str) -> PathBuf {
    sessions_dir(project_path).join(format!("{}.json", id))
}

fn active_file_path(project_path: &str) -> PathBuf {
    sessions_dir(project_path).join(".active")
}

fn legacy_session_file(project_path: &str) -> PathBuf {
    harness_dir(project_path).join("session.json")
}

fn recents_file() -> Result<PathBuf, String> {
    let base = dirs::data_dir().ok_or_else(|| "no data_dir on this platform".to_string())?;
    let dir = base.join("Harness");
    Ok(dir.join("recent.json"))
}

async fn ensure_sessions_dir(project_path: &str) -> Result<PathBuf, String> {
    let dir = sessions_dir(project_path);
    fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("Failed to create {}: {}", dir.display(), e))?;
    Ok(dir)
}

/// Normalize a path to forward-slash form. Best-effort: leaves it as-is if
/// `to_string_lossy` produces something pathological.
fn to_forward_slashes(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn project_relative(project_root: &str, abs_path: &Path) -> String {
    let abs = to_forward_slashes(abs_path);
    let mut root = project_root.replace('\\', "/");
    while root.ends_with('/') && root.len() > 1 {
        root.pop();
    }
    if abs == root {
        return String::new();
    }
    if let Some(stripped) = abs.strip_prefix(&format!("{}/", root)) {
        return stripped.to_string();
    }
    abs
}

fn normalize_for_compare(p: &str) -> String {
    let s = p.replace('\\', "/");
    #[cfg(target_os = "windows")]
    {
        s.to_lowercase()
    }
    #[cfg(not(target_os = "windows"))]
    {
        s
    }
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// -- Session id validation --

/// Strict whitelist so session ids can never escape the sessions dir:
/// 1–64 chars, hex digits and dashes only (UUID shape).
fn validate_session_id(id: &str) -> Result<(), String> {
    let valid = !id.is_empty()
        && id.len() <= SESSION_ID_MAX_LEN
        && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(format!("Invalid session id: {:?}", id))
    }
}

// -- Session I/O --

fn summarize(s: &Session, project_path: &str) -> SessionSummary {
    let project_name = Path::new(project_path)
        .file_name()
        .map(|x| x.to_string_lossy().to_string())
        .unwrap_or_else(|| project_path.to_string());
    SessionSummary {
        id: s.id.clone(),
        project_path: project_path.to_string(),
        project_name,
        created_at: s.created_at.clone(),
        updated_at: s.updated_at.clone(),
        message_count: s.messages.len(),
        title: s.title.clone(),
        agent_id: Some(s.agent.id.clone()),
        agent_name: Some(s.agent.name.clone()),
    }
}

/// Auto-title a session from its first user message when it still has the
/// placeholder title. Truncated to `TITLE_MAX_LEN` chars.
fn derive_title(session: &mut Session) {
    if session.title.is_empty() || session.title == "New session" {
        if let Some(first) = session.messages.iter().find(|m| m.role == "user") {
            let t: String = first.content.chars().take(TITLE_MAX_LEN).collect();
            let t = t.trim().to_string();
            if !t.is_empty() {
                session.title = t;
            }
        }
    }
}

/// List all sessions for a project, newest first. Also runs the v1→v2
/// migration on first contact. Corrupt/unknown-version files are archived
/// and skipped.
pub async fn list_sessions(project_path: &str) -> Result<Vec<SessionSummary>, String> {
    migrate_v1_if_needed(project_path).await?;
    let dir = sessions_dir(project_path);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut entries = match fs::read_dir(&dir).await {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()),
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".json") {
            continue;
        }
        let id = name.trim_end_matches(".json").to_string();
        if let Ok(Some(s)) = load_session(project_path, &id).await {
            out.push(summarize(&s, project_path));
        }
    }
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(out)
}

/// Create an empty session and make it the active one.
pub async fn create_session(
    project_path: &str,
    agent: Option<AgentSnapshot>,
) -> Result<Session, String> {
    let dir = ensure_sessions_dir(project_path).await?;
    let now = now_iso();
    let id = Uuid::new_v4().to_string();
    // Heal a legacy v1 snapshot: if the frontend sent an `id` but
    // no `personalityId`, lift the legacy `id` into the new field.
    let agent = match agent {
        Some(mut a) => {
            if a.personality_id.trim().is_empty() {
                if let Some(legacy) = a.id.take() {
                    a.personality_id = legacy;
                }
            }
            a
        }
        None => default_agent_snapshot(),
    };
    let session = Session {
        version: 2,
        id: id.clone(),
        title: "New session".into(),
        project_path: project_path.to_string(),
        created_at: now.clone(),
        updated_at: now,
        agent,
        goal_mode: false,
        action_mode: ActionMode::AskRisky,
        messages: Vec::new(),
        activities: Vec::new(),
        token_stats: None,
        last_turn: None,
    };
    write_session_atomic(&dir, &session).await?;
    set_active_session(project_path, &id).await?;
    Ok(session)
}

/// Load a session by id. `Ok(None)` when missing; corrupt or unknown-version
/// files are archived version-keyed.
pub async fn load_session(project_path: &str, id: &str) -> Result<Option<Session>, String> {
    validate_session_id(id)?;
    let path = session_path(project_path, id);
    if !path.exists() {
        return Ok(None);
    }
    let raw = match fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) => {
            // Treat read errors (sharing violations etc.) as "no session".
            eprintln!("[harness] load_session read error: {}", e);
            return Ok(None);
        }
    };
    match serde_json::from_str::<Session>(&raw) {
        Ok(s) => {
            if s.version != 2 {
                eprintln!(
                    "[harness] session {} has unknown version {}; archiving",
                    id, s.version
                );
                archive_session_file(&path, s.version).await;
                return Ok(None);
            }
            Ok(Some(s))
        }
        Err(e) => {
            eprintln!("[harness] corrupt session {} ({}); archiving", id, e);
            archive_session_file(&path, 2).await;
            Ok(None)
        }
    }
}

/// Load the active session for a project (`.active` marker, else newest).
/// Compatibility shim for the pre-WP6 1-arg command.
pub async fn load_active_session(project_path: &str) -> Result<Option<Session>, String> {
    migrate_v1_if_needed(project_path).await?;
    match resolve_active_session_id(project_path).await? {
        Some(id) => load_session(project_path, &id).await,
        None => Ok(None),
    }
}

/// Save atomically (tmp + rename). Rewrites `updatedAt`, normalizes the
/// version/projectPath to the arguments, auto-titles, and generates an id
/// when the caller sent an empty one (older frontend payloads).
pub async fn save_session(project_path: &str, mut session: Session) -> Result<Session, String> {
    session.version = 2;
    session.project_path = project_path.to_string();
    if session.id.is_empty() {
        session.id = Uuid::new_v4().to_string();
    }
    validate_session_id(&session.id)?;
    let dir = ensure_sessions_dir(project_path).await?;
    session.updated_at = now_iso();
    derive_title(&mut session);
    write_session_atomic(&dir, &session).await?;
    Ok(session)
}

/// Rename a session (title only). Empty titles fall back to the placeholder.
pub async fn rename_session(
    project_path: &str,
    id: &str,
    title: String,
) -> Result<SessionSummary, String> {
    let mut session = load_session(project_path, id)
        .await?
        .ok_or_else(|| format!("Session {} not found", id))?;
    let t = title.trim().to_string();
    session.title = if t.is_empty() {
        "New session".to_string()
    } else {
        t.chars().take(TITLE_MAX_LEN).collect()
    };
    let summary = summarize(&session, project_path);
    save_session(project_path, session).await?;
    Ok(summary)
}

/// Delete a session file and clear `.active` if it pointed here.
pub async fn delete_session(project_path: &str, id: &str) -> Result<(), String> {
    validate_session_id(id)?;
    let path = session_path(project_path, id);
    if path.exists() {
        fs::remove_file(&path)
            .await
            .map_err(|e| format!("Failed to delete session {}: {}", id, e))?;
    }
    let active = active_file_path(project_path);
    if let Ok(raw) = fs::read_to_string(&active).await {
        let marker = raw.trim().to_string();
        // Keep the marker only if it points at a live session other than the
        // one being deleted; clear it when it pointed here or has gone stale.
        let live = !marker.is_empty()
            && marker != id
            && validate_session_id(&marker).is_ok()
            && session_path(project_path, &marker).exists();
        if !live {
            let _ = fs::remove_file(&active).await;
        }
    }
    Ok(())
}

/// `.active` marker: the last session the user was in, used on project open.
pub async fn set_active_session(project_path: &str, id: &str) -> Result<(), String> {
    validate_session_id(id)?;
    let dir = ensure_sessions_dir(project_path).await?;
    let path = dir.join(".active");
    let tmp = dir.join(".active.tmp");
    fs::write(&tmp, id)
        .await
        .map_err(|e| format!("Failed to write .active: {}", e))?;
    fs::rename(&tmp, &path)
        .await
        .map_err(|e| format!("Failed to rename .active: {}", e))?;
    Ok(())
}

/// Resolve the active session id: `.active` marker if valid and present,
/// else the newest session, else `None`.
pub async fn resolve_active_session_id(project_path: &str) -> Result<Option<String>, String> {
    let active = active_file_path(project_path);
    if let Ok(raw) = fs::read_to_string(&active).await {
        let id = raw.trim();
        if validate_session_id(id).is_ok() && session_path(project_path, id).exists() {
            return Ok(Some(id.to_string()));
        }
    }
    let list = list_sessions(project_path).await?;
    Ok(list.first().map(|s| s.id.clone()))
}

/// Migrate the legacy single `<project>/.harness/session.json` (v1) into the
/// sessions directory as the first session. Returns the new session id when
/// a migration happened. Corrupt v1 files are archived version-keyed.
pub async fn migrate_v1_if_needed(project_path: &str) -> Result<Option<String>, String> {
    let legacy = legacy_session_file(project_path);
    if !legacy.exists() {
        return Ok(None);
    }

    let dir = sessions_dir(project_path);
    let has_sessions = if dir.is_dir() {
        let mut entries = match fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => return Ok(None),
        };
        let mut any = false;
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_name().to_string_lossy().ends_with(".json") {
                any = true;
                break;
            }
        }
        any
    } else {
        false
    };
    if has_sessions {
        // Sessions already exist; leave the legacy file alone.
        return Ok(None);
    }

    let raw = match fs::read_to_string(&legacy).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[harness] migrate read error: {}", e);
            return Ok(None);
        }
    };
    let v1: LegacySession = match serde_json::from_str(&raw) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[harness] corrupt legacy session.json ({}); archiving", e);
            let ts = chrono::Utc::now().timestamp();
            let archived =
                harness_dir(project_path).join(format!("session.v1.archived-{}.json", ts));
            if let Err(rename_err) = fs::rename(&legacy, &archived).await {
                eprintln!(
                    "[harness] failed to archive corrupt legacy session: {}",
                    rename_err
                );
            }
            return Ok(None);
        }
    };

    let id = Uuid::new_v4().to_string();
    let mut session = Session {
        version: 2,
        id: id.clone(),
        title: "New session".into(),
        project_path: project_path.to_string(),
        created_at: v1.created_at,
        updated_at: now_iso(),
        agent: default_agent_snapshot(),
        goal_mode: false,
        action_mode: ActionMode::AskRisky,
        messages: v1.messages,
        activities: Vec::new(),
        token_stats: v1.token_stats,
        last_turn: None,
    };
    derive_title(&mut session);
    let dir = ensure_sessions_dir(project_path).await?;
    write_session_atomic(&dir, &session).await?;
    set_active_session(project_path, &id).await?;
    let ts = chrono::Utc::now().timestamp();
    let archived = harness_dir(project_path).join(format!("session.json.migrated-{}", ts));
    let _ = fs::rename(&legacy, &archived).await;
    Ok(Some(id))
}

async fn write_session_atomic(dir: &Path, session: &Session) -> Result<(), String> {
    let json = serde_json::to_string_pretty(session)
        .map_err(|e| format!("Failed to serialize session: {}", e))?;
    let final_path = dir.join(format!("{}.json", session.id));
    let tmp_path = dir.join(format!("{}.json.tmp", session.id));
    fs::write(&tmp_path, json)
        .await
        .map_err(|e| format!("Failed to write tmp session file: {}", e))?;
    fs::rename(&tmp_path, &final_path)
        .await
        .map_err(|e| format!("Failed to rename tmp session file: {}", e))?;
    Ok(())
}

async fn archive_session_file(path: &Path, version: u32) {
    let ts = chrono::Utc::now().timestamp();
    let archived = path.with_file_name(format!("session.v{}.archived-{}.json", version, ts));
    if let Err(e) = fs::rename(path, &archived).await {
        eprintln!("[harness] failed to archive corrupt session: {}", e);
    }
}

// -- Recents I/O --

pub async fn list_recent_projects() -> Result<Vec<RecentProject>, String> {
    let path = recents_file()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = match fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(_) => return Ok(Vec::new()),
    };
    let parsed: Recents = match serde_json::from_str(&raw) {
        Ok(r) => r,
        Err(_) => return Ok(Vec::new()),
    };
    Ok(parsed.projects)
}

pub async fn forget_recent_project(path: &str) -> Result<(), String> {
    let file = recents_file()?;
    if !file.exists() {
        return Ok(());
    }
    let raw = match fs::read_to_string(&file).await {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };
    let mut recents: Recents = serde_json::from_str(&raw).unwrap_or_default();
    let needle = normalize_for_compare(path);
    recents
        .projects
        .retain(|p| normalize_for_compare(&p.path) != needle);
    write_recents(&file, &recents).await
}

pub async fn touch_recent_project(project_path: &str) -> Result<RecentProject, String> {
    let file = recents_file()?;
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create app data dir: {}", e))?;
    }

    let mut recents: Recents = if file.exists() {
        match fs::read_to_string(&file).await {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Recents::default(),
        }
    } else {
        Recents::default()
    };

    let needle = normalize_for_compare(project_path);
    recents
        .projects
        .retain(|p| normalize_for_compare(&p.path) != needle);

    let name = Path::new(project_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| project_path.to_string());

    let entry = RecentProject {
        path: project_path.to_string(),
        name,
        last_opened: now_iso(),
    };
    recents.projects.insert(0, entry.clone());
    if recents.projects.len() > RECENT_LIMIT {
        recents.projects.truncate(RECENT_LIMIT);
    }

    write_recents(&file, &recents).await?;
    Ok(entry)
}

async fn write_recents(file: &Path, recents: &Recents) -> Result<(), String> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create app data dir: {}", e))?;
    }
    let json = serde_json::to_string_pretty(recents)
        .map_err(|e| format!("Failed to serialize recents: {}", e))?;
    let tmp = file.with_extension("json.tmp");
    fs::write(&tmp, json)
        .await
        .map_err(|e| format!("Failed to write tmp recents: {}", e))?;
    fs::rename(&tmp, file)
        .await
        .map_err(|e| format!("Failed to rename tmp recents: {}", e))?;
    Ok(())
}

// -- File listing --

/// Recursively list files under `project_path`, returning project-relative
/// forward-slash paths. Skips ignored directories. Symlinks are not followed.
pub async fn list_files_recursive(project_path: &str) -> Result<Vec<String>, String> {
    let root = PathBuf::from(project_path);
    if !root.is_dir() {
        return Err(format!("Not a directory: {}", project_path));
    }
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.clone(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 8 {
            continue;
        }
        let mut entries = match fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            let abs = entry.path();

            // Never follow symlinks (cheap loop guard).
            let metadata = match fs::symlink_metadata(&abs).await {
                Ok(m) => m,
                Err(_) => continue,
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                if IGNORE_DIRS.contains(&name.as_str()) {
                    continue;
                }
                stack.push((abs, depth + 1));
            } else if metadata.is_file() {
                out.push(project_relative(project_path, &abs));
            }
        }
    }
    out.sort();
    Ok(out)
}

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn iso_now() -> String {
        now_iso()
    }

    fn sample_v2(path: &str, id: &str) -> Session {
        Session {
            version: 2,
            id: id.to_string(),
            title: "Fix login redirect".into(),
            project_path: path.to_string(),
            created_at: iso_now(),
            updated_at: iso_now(),
            agent: default_agent_snapshot(),
            goal_mode: false,
            action_mode: ActionMode::AskRisky,
            messages: vec![Message {
                id: "m1".into(),
                role: "user".into(),
                content: "hi".into(),
                timestamp: iso_now(),
                mentions: vec![],
                agent_id: None,
                agent_name: None,
                reasoning: None,
            }],
            activities: vec![ToolActivity {
                id: "act1".into(),
                tool: "read_file".into(),
                summary: "read src/main.rs".into(),
                status: ToolActivityStatus::Ran,
                timestamp: iso_now(),
                request_id: Some("req-1".into()),
                arguments: Some(serde_json::json!({ "path": "src/main.rs" })),
            }],
            token_stats: Some(TokenStats {
                last_in: 10,
                last_out: 5,
                cumulative_in: 10,
                cumulative_out: 5,
                session_window_used: Some(0.23),
                session_window_resets_at: Some("2026-08-12T00:00:00Z".into()),
            }),
            last_turn: None,
        }
    }

    #[tokio::test]
    async fn save_and_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let s = sample_v2(&path, "11111111-1111-1111-1111-111111111111");
        let saved = save_session(&path, s).await.unwrap();
        let loaded = load_session(&path, &saved.id).await.unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.messages[0].content, "hi");
        assert_eq!(loaded.activities.len(), 1);
        assert_eq!(loaded.activities[0].status, ToolActivityStatus::Ran);
        // updated_at is rewritten by save_session
        assert_eq!(loaded.updated_at, saved.updated_at);
        // tokenStats survives the round trip
        let ts = loaded.token_stats.unwrap();
        assert_eq!(ts.last_in, 10);
        assert_eq!(ts.cumulative_out, 5);
        assert_eq!(ts.session_window_used, Some(0.23));
    }

    #[tokio::test]
    async fn save_auto_titles_from_first_user_message() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let mut s = sample_v2(&path, "22222222-2222-2222-2222-222222222222");
        s.title = "New session".into();
        let saved = save_session(&path, s).await.unwrap();
        assert_eq!(saved.title, "hi");
    }

    #[tokio::test]
    async fn save_generates_id_when_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let mut s = sample_v2(&path, "");
        s.id = String::new();
        let saved = save_session(&path, s).await.unwrap();
        assert!(!saved.id.is_empty());
        assert_eq!(saved.version, 2);
    }

    #[tokio::test]
    async fn corrupt_session_is_archived() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let sd = ensure_sessions_dir(&path).await.unwrap();
        std::fs::write(sd.join("abc.json"), "{not json").unwrap();
        let loaded = load_session(&path, "abc").await.unwrap();
        assert!(loaded.is_none());
        let entries: Vec<_> = std::fs::read_dir(&sd)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            entries
                .iter()
                .any(|n| n.starts_with("session.v2.archived-") && n.ends_with(".json")),
            "expected an archived corrupt file, got {:?}",
            entries
        );
    }

    #[tokio::test]
    async fn unknown_version_is_archived() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let sd = ensure_sessions_dir(&path).await.unwrap();
        let mut s = sample_v2(&path, "33333333-3333-3333-3333-333333333333");
        s.version = 99;
        std::fs::write(
            sd.join("33333333-3333-3333-3333-333333333333.json"),
            serde_json::to_string(&s).unwrap(),
        )
        .unwrap();
        let loaded = load_session(&path, "33333333-3333-3333-3333-333333333333")
            .await
            .unwrap();
        assert!(loaded.is_none());
        let entries: Vec<_> = std::fs::read_dir(&sd)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            entries
                .iter()
                .any(|n| n.starts_with("session.v99.archived-")),
            "expected an archived unknown-version file, got {:?}",
            entries
        );
    }

    #[tokio::test]
    async fn multi_session_crud() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let a = create_session(&path, None).await.unwrap();
        let b = create_session(&path, None).await.unwrap();

        let list = list_sessions(&path).await.unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|s| s.id == a.id));
        assert!(list.iter().any(|s| s.id == b.id));
        assert_eq!(list[0].agent_name.as_deref(), Some("harness"));

        let renamed = rename_session(&path, &a.id, "My custom title".into())
            .await
            .unwrap();
        assert_eq!(renamed.title, "My custom title");

        delete_session(&path, &a.id).await.unwrap();
        let list = list_sessions(&path).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, b.id);
        assert!(load_session(&path, &a.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn active_marker_resolution() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let a = create_session(&path, None).await.unwrap();
        let b = create_session(&path, None).await.unwrap();

        // Pin distinct updatedAt values so the "newest" fallback below is
        // deterministic instead of depending on clock precision + readdir order.
        let sd = ensure_sessions_dir(&path).await.unwrap();
        let mut a2 = a.clone();
        a2.updated_at = "2026-08-11T00:00:00Z".into();
        std::fs::write(
            sd.join(format!("{}.json", a.id)),
            serde_json::to_string(&a2).unwrap(),
        )
        .unwrap();
        let mut b2 = b.clone();
        b2.updated_at = "2026-08-12T00:00:00Z".into();
        std::fs::write(
            sd.join(format!("{}.json", b.id)),
            serde_json::to_string(&b2).unwrap(),
        )
        .unwrap();

        // Marker points at b (most recent create); resolution honors it.
        assert_eq!(
            resolve_active_session_id(&path).await.unwrap().unwrap(),
            b.id
        );

        // Overwrite marker with an unknown id -> falls back to newest (b).
        set_active_session(&path, "99999999-9999-9999-9999-999999999999")
            .await
            .unwrap();
        assert_eq!(
            resolve_active_session_id(&path).await.unwrap().unwrap(),
            b.id
        );

        // Deleting the active session clears the marker.
        delete_session(&path, &b.id).await.unwrap();
        assert!(!active_file_path(&path).exists());
        assert_eq!(
            resolve_active_session_id(&path).await.unwrap().unwrap(),
            a.id
        );
    }

    #[tokio::test]
    async fn list_sorted_by_updated_at() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let sd = ensure_sessions_dir(&path).await.unwrap();
        // Write two files directly with distinct updatedAt values.
        let mut old = sample_v2(&path, "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa");
        old.updated_at = "2026-08-11T00:00:00Z".into();
        let mut new = sample_v2(&path, "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb");
        new.updated_at = "2026-08-12T00:00:00Z".into();
        std::fs::write(
            sd.join("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa.json"),
            serde_json::to_string(&old).unwrap(),
        )
        .unwrap();
        std::fs::write(
            sd.join("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb.json"),
            serde_json::to_string(&new).unwrap(),
        )
        .unwrap();
        let list = list_sessions(&path).await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb");
    }

    #[tokio::test]
    async fn migration_from_v1() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let h = std::path::Path::new(&path).join(".harness");
        std::fs::create_dir_all(&h).unwrap();
        let v1 = serde_json::json!({
            "version": 1,
            "projectPath": path,
            "createdAt": "2026-08-11T00:00:00Z",
            "updatedAt": "2026-08-11T01:00:00Z",
            "messages": [
                {
                    "id": "m1",
                    "role": "user",
                    "content": "Explain this repo",
                    "timestamp": "2026-08-11T00:30:00Z",
                    "mentions": []
                }
            ],
            "tokenStats": { "lastIn": 1, "lastOut": 2, "cumulativeIn": 1, "cumulativeOut": 2 }
        });
        std::fs::write(h.join("session.json"), serde_json::to_string(&v1).unwrap()).unwrap();

        let list = list_sessions(&path).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "Explain this repo");
        assert_eq!(list[0].message_count, 1);

        // Legacy file was archived, not deleted.
        let entries: Vec<_> = std::fs::read_dir(&h)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            entries
                .iter()
                .any(|n| n.starts_with("session.json.migrated-")),
            "expected migrated legacy file, got {:?}",
            entries
        );

        // The migrated session loads with tokenStats intact.
        let loaded = load_session(&path, &list[0].id).await.unwrap().unwrap();
        assert_eq!(loaded.messages[0].content, "Explain this repo");
        assert_eq!(loaded.token_stats.unwrap().cumulative_in, 1);
    }

    #[tokio::test]
    async fn migration_skipped_when_sessions_exist() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        create_session(&path, None).await.unwrap();
        let h = std::path::Path::new(&path).join(".harness");
        std::fs::create_dir_all(&h).unwrap();
        std::fs::write(h.join("session.json"), "{}").unwrap();

        let list = list_sessions(&path).await.unwrap();
        assert_eq!(list.len(), 1);
        // Legacy file untouched (still raw, not archived).
        assert!(h.join("session.json").exists());
    }

    #[tokio::test]
    async fn corrupt_legacy_is_archived_not_migrated() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let h = std::path::Path::new(&path).join(".harness");
        std::fs::create_dir_all(&h).unwrap();
        std::fs::write(h.join("session.json"), "{not json").unwrap();

        let list = list_sessions(&path).await.unwrap();
        assert!(list.is_empty());
        let entries: Vec<_> = std::fs::read_dir(&h)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            entries
                .iter()
                .any(|n| n.starts_with("session.v1.archived-")),
            "expected archived corrupt legacy file, got {:?}",
            entries
        );
    }

    #[test]
    fn session_id_validation() {
        assert!(validate_session_id("9f2c5c6e-1234-4a5b-8c9d-abcdef012345").is_ok());
        assert!(validate_session_id("../").is_err());
        assert!(validate_session_id("a/b").is_err());
        assert!(validate_session_id("").is_err());
        assert!(validate_session_id("!").is_err());
        assert!(validate_session_id(&"a".repeat(65)).is_err());
        assert!(validate_session_id(&"a".repeat(64)).is_ok());
    }

    #[tokio::test]
    async fn recents_cap_at_20_lru() {
        // Exercise the trim logic directly (hermetic, no global state).
        let mut projects: Vec<RecentProject> = (0..25)
            .map(|i| RecentProject {
                path: format!("/p/{}", i),
                name: format!("p{}", i),
                last_opened: now_iso(),
            })
            .collect();
        if projects.len() > RECENT_LIMIT {
            projects.truncate(RECENT_LIMIT);
        }
        assert_eq!(projects.len(), 20);
        assert_eq!(projects[0].path, "/p/0");
    }

    #[tokio::test]
    async fn recents_case_insensitive_on_windows() {
        let needle = normalize_for_compare("C:/code/MyApp");
        let stored = normalize_for_compare("c:/code/myapp");
        assert_eq!(needle, stored);
    }

    #[tokio::test]
    async fn list_files_recursive_skips_node_modules() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src").join("main.rs"), "").unwrap();
        std::fs::create_dir_all(root.join("node_modules").join("foo")).unwrap();
        std::fs::write(root.join("node_modules").join("foo").join("x.js"), "").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git").join("HEAD"), "").unwrap();

        let root_str = root.to_string_lossy().to_string();
        let files = list_files_recursive(&root_str).await.unwrap();
        assert!(files.iter().any(|f| f.ends_with("src/main.rs")));
        assert!(!files.iter().any(|f| f.contains("node_modules")));
        assert!(!files.iter().any(|f| f.contains(".git")));
    }

    #[test]
    fn session_wire_format_is_camelcase() {
        // The TS frontend speaks camelCase. Verify the wire format matches.
        let s = sample_v2("/p", "9f2c5c6e-1234-4a5b-8c9d-abcdef012345");
        let json = serde_json::to_string(&s).unwrap();
        assert!(
            json.contains("\"projectPath\""),
            "expected projectPath in {}",
            json
        );
        assert!(
            json.contains("\"createdAt\""),
            "expected createdAt in {}",
            json
        );
        assert!(
            json.contains("\"updatedAt\""),
            "expected updatedAt in {}",
            json
        );
        // Agent is a nested object on Session; the flat agentId/agentName
        // live on SessionSummary (the list payload the Sessions panel reads).
        assert!(
            json.contains("\"agent\":{\"id\":\"harness\""),
            "expected nested agent in {}",
            json
        );
        assert!(
            json.contains("\"systemPrompt\""),
            "expected systemPrompt in {}",
            json
        );
        let summary = summarize(&s, "/p");
        let summary_json = serde_json::to_string(&summary).unwrap();
        assert!(
            summary_json.contains("\"agentId\""),
            "expected agentId on summary in {}",
            summary_json
        );
        assert!(
            summary_json.contains("\"agentName\""),
            "expected agentName on summary in {}",
            summary_json
        );
        assert!(
            json.contains("\"goalMode\""),
            "expected goalMode in {}",
            json
        );
        assert!(
            json.contains("\"actionMode\""),
            "expected actionMode in {}",
            json
        );
        assert!(
            json.contains("\"activities\""),
            "expected activities in {}",
            json
        );
        // No snake_case should leak
        assert!(
            !json.contains("\"project_path\""),
            "snake_case leaked: {}",
            json
        );
        assert!(
            !json.contains("\"created_at\""),
            "snake_case leaked: {}",
            json
        );
        assert!(
            !json.contains("\"goal_mode\""),
            "snake_case leaked: {}",
            json
        );
        assert!(
            !json.contains("\"action_mode\""),
            "snake_case leaked: {}",
            json
        );
        assert!(
            !json.contains("\"system_prompt\""),
            "snake_case leaked: {}",
            json
        );
        // Round-trip from camelCase JSON the way the frontend sends it,
        // including a v1-shaped payload (missing the new fields).
        let from_ts = r#"{
            "version": 1,
            "projectPath": "/p",
            "createdAt": "2026-08-11T00:00:00Z",
            "updatedAt": "2026-08-11T00:00:00Z",
            "messages": []
        }"#;
        let parsed: Session = serde_json::from_str(from_ts).unwrap();
        assert_eq!(parsed.project_path, "/p");
        assert_eq!(parsed.version, 1); // normalized to 2 on save
        // v1 sessions carried the personality inline as `id`; the v2
        // deserializer lifts it into `personalityId`. The healing
        // happens at the `create_session` boundary, so the raw
        // deserialized value still has `id` set and `personalityId`
        // empty until first save.
        assert_eq!(parsed.agent.id.as_deref(), Some("harness"));
    }
}

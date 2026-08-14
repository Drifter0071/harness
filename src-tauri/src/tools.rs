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

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::agent::AgentRole;
use crate::agent_loop::AgentEventSink;
use crate::llm_client::ToolSpec;

/// Output cap for tool `content` (model context budget).
const CONTENT_MAX: usize = 32 * 1024;
/// Per-stream cap for command stdout/stderr.
const STREAM_MAX: usize = 16 * 1024;
/// Hard timeout for `run_command` (the child is killed on expiry).
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const SEARCH_MAX_DEPTH: usize = 8;

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
        }
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
                description: "Search the web via DuckDuckGo and return the top result titles, URLs and snippets.".into(),
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
        ]
    }
}

/// Execute one tool call. The `ctx` carries everything a tool might need:
/// the workspace root, the policy/approval verdict (`auto_allowed`), the
/// caller's role (so role-gated tools like `bip` can refuse), and an event
/// sink for state broadcasts (`bip-state`).
pub async fn execute_tool(
    ctx: &ToolContext,
    name: &str,
    args: &Value,
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
    let results = crate::web_search::search_ddg(&query)
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
    Ok(ToolResult {
        ok: true,
        summary: format!("web search for {:?} ({} results)", query, results.len()),
        content: Some(content),
    })
}

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

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentRole;
    use crate::agent_loop::AgentEventSink;
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
}

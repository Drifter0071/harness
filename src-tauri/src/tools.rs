//! Workspace-guarded tool registry for the agent loop.
//!
//! Every tool the agent can call is declared here with a JSON schema
//! (`ToolSpec::all`) and an executor (`execute_tool`). All filesystem
//! targets are forced inside the session workspace via `resolve_within`,
//! commands run with a hard timeout, and output is capped so the model
//! context never explodes.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

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
        ]
    }
}

/// Execute one tool call. `auto_allowed` is the policy/approval verdict the
/// loop already made: `run_command` is defense-in-depth gated on it (the
/// loop passes `true` after policy clearance or user approval), every other
/// tool executes and reports.
pub async fn execute_tool(
    workspace: &str,
    name: &str,
    args: &Value,
    auto_allowed: bool,
) -> Result<ToolResult, String> {
    match name {
        "read_file" => tool_read_file(workspace, args).await,
        "list_dir" => tool_list_dir(workspace, args).await,
        "search_files" => tool_search_files(workspace, args).await,
        "write_file" => tool_write_file(workspace, args).await,
        "edit_file" => tool_edit_file(workspace, args).await,
        "run_command" => {
            if !auto_allowed {
                return Err("run_command requires approval".into());
            }
            tool_run_command(workspace, args, COMMAND_TIMEOUT).await
        }
        "web_search" => tool_web_search(args).await,
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
    use tempfile::TempDir;

    fn ws(dir: &TempDir) -> String {
        dir.path().to_string_lossy().to_string()
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
            &w,
            "edit_file",
            &json!({"path": "src/app.rs", "old": "TODO: fix", "new": "fixed"}),
            true,
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
        let res = execute_tool(
            &w,
            "edit_file",
            &json!({"path": "a.txt", "old": "missing", "new": "x"}),
            true,
        )
        .await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("not found"));

        std::fs::write(root.join("b.txt"), "dup dup").unwrap();
        let res = execute_tool(
            &w,
            "edit_file",
            &json!({"path": "b.txt", "old": "dup", "new": "x"}),
            true,
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
            &w,
            "write_file",
            &json!({"path": "deep/nested/file.txt", "content": "hello"}),
            true,
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

        let read = execute_tool(&w, "read_file", &json!({"path": "src/main.rs"}), true)
            .await
            .unwrap();
        assert!(read.ok);
        assert_eq!(read.content.as_deref(), Some("fn main(){}"));

        let list = execute_tool(&w, "list_dir", &json!({"path": "."}), true)
            .await
            .unwrap();
        assert!(list.ok);
        assert!(list.content.unwrap().contains("README.md"));

        let search = execute_tool(
            &w,
            "search_files",
            &json!({"root": ".", "pattern": "main"}),
            true,
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
        let res = execute_tool(
            &w,
            "run_command",
            &json!({"command": "echo", "args": ["hi"]}),
            false,
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
            &w,
            "run_command",
            &json!({
                "command": "powershell",
                "args": ["-NoProfile", "-Command", "Write-Output ('x' * 50000)"]
            }),
            true,
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
            &w,
            "run_command",
            &json!({"command": "sh", "args": ["-c", "yes x | head -c 100000"]}),
            true,
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
        let res = execute_tool("", "web_search", &json!({}), true).await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("query"));

        let res = execute_tool("", "web_search", &json!({"query": 42}), true).await;
        assert!(res.is_err());
    }

    // -- unknown tool --

    #[tokio::test]
    async fn unknown_tool_is_rejected() {
        let dir = TempDir::new().unwrap();
        let w = ws(&dir);
        let res = execute_tool(&w, "nope", &json!({}), true).await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("Unknown tool"));
    }
}

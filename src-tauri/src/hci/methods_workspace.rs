//! Read-tier handlers for `workspace.*`.

use serde_json::{json, Value};
use std::path::Path;

use crate::commands;
use crate::hci::router::{ConnectionCtx, MethodHandler, MethodTier, RpcContext, RouterError};
use crate::tools;

const READ_MAX_BYTES: u64 = 5 * 1024 * 1024;
const LIST_MAX_DEPTH: usize = 8;
const SEARCH_MAX_RESULTS: usize = 200;

pub struct ListHandler;
#[async_trait::async_trait]
impl MethodHandler for ListHandler {
    fn name(&self) -> &'static str { "workspace.list" }
    fn tier(&self) -> MethodTier { MethodTier::Read }
    async fn handle(
        &self,
        params: Value,
        _ctx: &RpcContext,
        _conn: &ConnectionCtx,
    ) -> Result<Value, RouterError> {
        let workspace = params.get("workspace").and_then(|v| v.as_str())
            .ok_or_else(|| RouterError::Internal("missing 'workspace' param".into()))?;
        let depth = params.get("depth").and_then(|v| v.as_u64()).unwrap_or(4) as usize;
        let depth = depth.min(LIST_MAX_DEPTH);

        let all = commands::list_files_recursive(workspace.to_string()).await
            .map_err(RouterError::Internal)?;
        let total = all.len();

        let root = Path::new(workspace);
        let mut entries: Vec<Value> = Vec::new();
        for p in all {
            if let Ok(rel) = Path::new(&p).strip_prefix(root) {
                let components = rel.components().count();
                if components <= depth {
                    entries.push(json!({ "path": p, "depth": components }));
                }
            }
        }
        Ok(json!({ "entries": entries, "truncated": entries.len() < total }))
    }
}

pub struct ReadHandler;
#[async_trait::async_trait]
impl MethodHandler for ReadHandler {
    fn name(&self) -> &'static str { "workspace.read" }
    fn tier(&self) -> MethodTier { MethodTier::Read }
    async fn handle(
        &self,
        params: Value,
        _ctx: &RpcContext,
        _conn: &ConnectionCtx,
    ) -> Result<Value, RouterError> {
        let workspace = params.get("workspace").and_then(|v| v.as_str())
            .ok_or_else(|| RouterError::Internal("missing 'workspace' param".into()))?;
        let path = params.get("path").and_then(|v| v.as_str())
            .ok_or_else(|| RouterError::Internal("missing 'path' param".into()))?;
        let resolved = tools::resolve_within(workspace, path)
            .map_err(RouterError::Internal)?;
        let meta = std::fs::metadata(&resolved)
            .map_err(|e| RouterError::Internal(format!("stat failed: {e}")))?;
        if meta.len() > READ_MAX_BYTES {
            return Err(RouterError::Internal(format!(
                "file too large: {} bytes (max {})", meta.len(), READ_MAX_BYTES
            )));
        }
        let content = commands::read_file(resolved.to_string_lossy().into_owned()).await
            .map_err(RouterError::Internal)?;
        Ok(json!({ "path": path, "size": meta.len(), "content": content }))
    }
}

pub struct StatHandler;
#[async_trait::async_trait]
impl MethodHandler for StatHandler {
    fn name(&self) -> &'static str { "workspace.stat" }
    fn tier(&self) -> MethodTier { MethodTier::Read }
    async fn handle(
        &self,
        params: Value,
        _ctx: &RpcContext,
        _conn: &ConnectionCtx,
    ) -> Result<Value, RouterError> {
        let workspace = params.get("workspace").and_then(|v| v.as_str())
            .ok_or_else(|| RouterError::Internal("missing 'workspace' param".into()))?;
        let path = params.get("path").and_then(|v| v.as_str())
            .ok_or_else(|| RouterError::Internal("missing 'path' param".into()))?;
        let resolved = tools::resolve_within(workspace, path)
            .map_err(RouterError::Internal)?;
        let meta = std::fs::metadata(&resolved)
            .map_err(|e| RouterError::Internal(format!("stat failed: {e}")))?;
        Ok(json!({
            "path": path,
            "size": meta.len(),
            "is_dir": meta.is_dir(),
            "is_file": meta.is_file(),
            "modified": meta.modified().ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs()),
        }))
    }
}

pub struct SearchHandler;
#[async_trait::async_trait]
impl MethodHandler for SearchHandler {
    fn name(&self) -> &'static str { "workspace.search" }
    fn tier(&self) -> MethodTier { MethodTier::Read }
    async fn handle(
        &self,
        params: Value,
        _ctx: &RpcContext,
        _conn: &ConnectionCtx,
    ) -> Result<Value, RouterError> {
        let path = params.get("path").and_then(|v| v.as_str())
            .ok_or_else(|| RouterError::Internal("missing 'path' param".into()))?;
        let pattern = params.get("pattern").and_then(|v| v.as_str())
            .ok_or_else(|| RouterError::Internal("missing 'pattern' param".into()))?;
        let max_results = params.get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(SEARCH_MAX_RESULTS as u64) as usize;
        let max_results = max_results.min(SEARCH_MAX_RESULTS);
        let results = commands::search_files(path.to_string(), pattern.to_string()).await
            .map_err(RouterError::Internal)?;
        let truncated = results.len() > max_results;
        let capped: Vec<String> = results.into_iter().take(max_results).collect();
        Ok(json!({ "matches": capped, "truncated": truncated }))
    }
}

pub fn register(router: crate::hci::router::Router) -> crate::hci::router::Router {
    router
        .register(ListHandler)
        .register(ReadHandler)
        .register(StatHandler)
        .register(SearchHandler)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hci::auth::AuthState;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn ctx_with_app(app: Arc<crate::commands::AppState>) -> RpcContext {
        let auth = AuthState::new("boot".into());
        let _ = auth.consume_boot_token("boot");
        RpcContext {
            writes_enabled: false,
            auth: Some(auth),
            started_at: chrono::Utc::now(),
            app: Some(app),
            canvas_bus: Some(crate::hci::CanvasEventBus::new()),
        }
    }

    #[tokio::test]
    async fn list_caps_depth() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b/c")).unwrap();
        std::fs::write(dir.path().join("a/file.txt"), "x").unwrap();
        std::fs::write(dir.path().join("a/b/c/deep.txt"), "x").unwrap();

        let app = Arc::new(crate::commands::AppState::new(Arc::new(crate::secrets::InMemoryStore::new())));
        let ctx = ctx_with_app(app);
        let conn = ConnectionCtx { session_token: "x".into() };

        let r = ListHandler.handle(
            json!({ "workspace": dir.path().to_string_lossy(), "depth": 2 }),
            &ctx, &conn,
        ).await.unwrap();
        let entries = r["entries"].as_array().unwrap();
        assert!(entries.iter().all(|e| e["depth"].as_u64().unwrap() <= 2));
    }

    #[tokio::test]
    async fn read_small_file() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("ok.txt");
        std::fs::write(&p, "hello").unwrap();
        let app = Arc::new(crate::commands::AppState::new(Arc::new(crate::secrets::InMemoryStore::new())));
        let ctx = ctx_with_app(app);
        let conn = ConnectionCtx { session_token: "x".into() };

        let r = ReadHandler.handle(
            json!({ "workspace": dir.path().to_string_lossy(), "path": "ok.txt" }),
            &ctx, &conn,
        ).await.unwrap();
        assert_eq!(r["content"], "hello");
    }

    #[tokio::test]
    async fn stat_returns_metadata() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, "abc").unwrap();
        let app = Arc::new(crate::commands::AppState::new(Arc::new(crate::secrets::InMemoryStore::new())));
        let ctx = ctx_with_app(app);
        let conn = ConnectionCtx { session_token: "x".into() };

        let r = StatHandler.handle(
            json!({ "workspace": dir.path().to_string_lossy(), "path": "f.txt" }),
            &ctx, &conn,
        ).await.unwrap();
        assert_eq!(r["size"], 3);
        assert_eq!(r["is_file"], true);
        assert_eq!(r["is_dir"], false);
    }
}

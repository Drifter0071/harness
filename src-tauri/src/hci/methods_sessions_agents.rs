//! Read-tier handlers for `session.*` and `agent.*`.
//!
//! `session.list` and `agent.list` are the workhorses: the outer
//! agent loop (and the Health Agent) query these every tick to see
//! what's happening. The full `session.get` and `agent.get` are
//! heavier and exist for debugging / deep introspection.
//!
//! Note: `session.*` methods go through the existing Tauri commands
//! (which only need project paths and don't need `State<'_, AppState>`).
//! `agent.*` methods read `AppState::agents` directly because the
//! underlying Tauri commands take a `State<>` we can't easily mock
//! outside a Tauri runtime.

use serde_json::{json, Value};

use crate::commands;
use crate::hci::router::{ConnectionCtx, MethodHandler, MethodTier, RpcContext, RouterError};

// ============================= session.* =============================

pub struct SessionListHandler;
impl MethodHandler for SessionListHandler {
    fn name(&self) -> &'static str { "session.list" }
    fn tier(&self) -> MethodTier { MethodTier::Read }
    fn handle(
        &self,
        params: Value,
        _ctx: &RpcContext,
        _conn: &ConnectionCtx,
    ) -> Result<Value, RouterError> {
        let project = params.get("project").and_then(|v| v.as_str())
            .ok_or_else(|| RouterError::Internal("missing 'project' param".into()))?;
        let summaries = tokio::runtime::Handle::current()
            .block_on(commands::list_sessions(project.to_string()))
            .map_err(RouterError::Internal)?;
        let json_str = serde_json::to_string(&summaries)
            .map_err(|e| RouterError::Internal(e.to_string()))?;
        let parsed: Value = serde_json::from_str(&json_str)
            .map_err(|e| RouterError::Internal(e.to_string()))?;
        Ok(json!({ "sessions": parsed }))
    }
}

pub struct SessionGetHandler;
impl MethodHandler for SessionGetHandler {
    fn name(&self) -> &'static str { "session.get" }
    fn tier(&self) -> MethodTier { MethodTier::Read }
    fn handle(
        &self,
        params: Value,
        _ctx: &RpcContext,
        _conn: &ConnectionCtx,
    ) -> Result<Value, RouterError> {
        let project = params.get("project").and_then(|v| v.as_str())
            .ok_or_else(|| RouterError::Internal("missing 'project' param".into()))?;
        let id = params.get("id").and_then(|v| v.as_str())
            .ok_or_else(|| RouterError::Internal("missing 'id' param".into()))?;
        let session = tokio::runtime::Handle::current()
            .block_on(commands::load_session(project.to_string(), id.to_string()))
            .map_err(RouterError::Internal)?;
        let json_str = serde_json::to_string(&session)
            .map_err(|e| RouterError::Internal(e.to_string()))?;
        serde_json::from_str(&json_str)
            .map_err(|e| RouterError::Internal(e.to_string()))
    }
}

// ============================= agent.* =============================

pub struct AgentListHandler;
impl MethodHandler for AgentListHandler {
    fn name(&self) -> &'static str { "agent.list" }
    fn tier(&self) -> MethodTier { MethodTier::Read }
    fn handle(
        &self,
        _params: Value,
        ctx: &RpcContext,
        _conn: &ConnectionCtx,
    ) -> Result<Value, RouterError> {
        let app = ctx.app.as_ref().ok_or_else(|| {
            RouterError::Internal("app state unavailable".into())
        })?;
        let agents = app.agents.blocking_lock();
        let out: Vec<Value> = agents.values().map(|a| {
            json!({
                "id": a.id,
                "name": a.name,
                "role": format!("{:?}", a.role),
                "status": format!("{:?}", a.status),
                "messageCount": a.messages.len(),
                "lastActivity": a.messages.last().map(|m| m.timestamp.clone()),
            })
        }).collect();
        Ok(json!({ "agents": out }))
    }
}

pub struct AgentGetHandler;
impl MethodHandler for AgentGetHandler {
    fn name(&self) -> &'static str { "agent.get" }
    fn tier(&self) -> MethodTier { MethodTier::Read }
    fn handle(
        &self,
        params: Value,
        ctx: &RpcContext,
        _conn: &ConnectionCtx,
    ) -> Result<Value, RouterError> {
        let id = params.get("id").and_then(|v| v.as_str())
            .ok_or_else(|| RouterError::Internal("missing 'id' param".into()))?;
        let app = ctx.app.as_ref().ok_or_else(|| {
            RouterError::Internal("app state unavailable".into())
        })?;
        let agents = app.agents.blocking_lock();
        let agent = agents.get(id).ok_or_else(|| {
            RouterError::Internal(format!("agent not found: {id}"))
        })?;
        Ok(json!({
            "id": agent.id,
            "name": agent.name,
            "role": format!("{:?}", agent.role),
            "status": format!("{:?}", agent.status),
            "messages": agent.messages,
        }))
    }
}

pub fn register(router: crate::hci::router::Router) -> crate::hci::router::Router {
    router
        .register(SessionListHandler)
        .register(SessionGetHandler)
        .register(AgentListHandler)
        .register(AgentGetHandler)
}

//! Read-tier handlers for `session.*` and `agent.*`.

use serde_json::{json, Value};

use crate::commands;
use crate::hci::router::{ConnectionCtx, MethodHandler, MethodTier, RouterError, RpcContext};

// ============================= session.* =============================

pub struct SessionListHandler;
#[async_trait::async_trait]
impl MethodHandler for SessionListHandler {
    fn name(&self) -> &'static str {
        "session.list"
    }
    fn tier(&self) -> MethodTier {
        MethodTier::Read
    }
    async fn handle(
        &self,
        params: Value,
        _ctx: &RpcContext,
        _conn: &ConnectionCtx,
    ) -> Result<Value, RouterError> {
        let project = params
            .get("project")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RouterError::Internal("missing 'project' param".into()))?;
        let summaries = commands::list_sessions(project.to_string())
            .await
            .map_err(RouterError::Internal)?;
        let json_str =
            serde_json::to_string(&summaries).map_err(|e| RouterError::Internal(e.to_string()))?;
        let parsed: Value =
            serde_json::from_str(&json_str).map_err(|e| RouterError::Internal(e.to_string()))?;
        Ok(json!({ "sessions": parsed }))
    }
}

pub struct SessionGetHandler;
#[async_trait::async_trait]
impl MethodHandler for SessionGetHandler {
    fn name(&self) -> &'static str {
        "session.get"
    }
    fn tier(&self) -> MethodTier {
        MethodTier::Read
    }
    async fn handle(
        &self,
        params: Value,
        _ctx: &RpcContext,
        _conn: &ConnectionCtx,
    ) -> Result<Value, RouterError> {
        let project = params
            .get("project")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RouterError::Internal("missing 'project' param".into()))?;
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RouterError::Internal("missing 'id' param".into()))?;
        let session = commands::load_session(project.to_string(), id.to_string())
            .await
            .map_err(RouterError::Internal)?;
        let json_str =
            serde_json::to_string(&session).map_err(|e| RouterError::Internal(e.to_string()))?;
        serde_json::from_str(&json_str).map_err(|e| RouterError::Internal(e.to_string()))
    }
}

// ============================= agent.* =============================

pub struct AgentListHandler;
#[async_trait::async_trait]
impl MethodHandler for AgentListHandler {
    fn name(&self) -> &'static str {
        "agent.list"
    }
    fn tier(&self) -> MethodTier {
        MethodTier::Read
    }
    async fn handle(
        &self,
        _params: Value,
        ctx: &RpcContext,
        _conn: &ConnectionCtx,
    ) -> Result<Value, RouterError> {
        let app = ctx
            .app
            .as_ref()
            .ok_or_else(|| RouterError::Internal("app state unavailable".into()))?;
        let agents = app.agents.lock().await;
        let out: Vec<Value> = agents
            .values()
            .map(|a| {
                json!({
                    "id": a.id,
                    "name": a.name,
                    "role": format!("{:?}", a.role),
                    "status": format!("{:?}", a.status),
                    "messageCount": a.messages.len(),
                    "lastActivity": a.messages.last().map(|m| m.timestamp.clone()),
                })
            })
            .collect();
        Ok(json!({ "agents": out }))
    }
}

pub struct AgentGetHandler;
#[async_trait::async_trait]
impl MethodHandler for AgentGetHandler {
    fn name(&self) -> &'static str {
        "agent.get"
    }
    fn tier(&self) -> MethodTier {
        MethodTier::Read
    }
    async fn handle(
        &self,
        params: Value,
        ctx: &RpcContext,
        _conn: &ConnectionCtx,
    ) -> Result<Value, RouterError> {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RouterError::Internal("missing 'id' param".into()))?;
        let app = ctx
            .app
            .as_ref()
            .ok_or_else(|| RouterError::Internal("app state unavailable".into()))?;
        let agents = app.agents.lock().await;
        let agent = agents
            .get(id)
            .ok_or_else(|| RouterError::Internal(format!("agent not found: {id}")))?;
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

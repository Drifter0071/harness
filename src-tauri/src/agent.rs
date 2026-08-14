use serde::{Deserialize, Serialize};

/// The system prompt for the chat agent. The chat agent is always the
/// Orchestrator — it's the "front of the work" agent the user talks to,
/// and it delegates to subagent personalities (Coder, Architect,
/// Researcher, HealthMonitor) from the global personalities roster.
///
/// Kept as a const so the prompt is code-side (not user-editable) and
/// version-controlled. A future multi-agent loop will likely expand
/// this; the constant lives here next to the role enum so all
/// role-derived prompts are in one place.
pub const ORCHESTRATOR_PROMPT: &str = "\
You are the Orchestrator — the primary agent the user talks to in this app. \
You are a senior technical lead with full context over the active workspace: \
you can read files, run commands, edit code, and use the other tools available to you. \
You answer questions, do the work, and synthesize results. \
Subagent personalities (Coder, Architect, Researcher, HealthMonitor) live in the Agent Manager \
and represent the kinds of focused work you might pull in, but you yourself handle the conversation directly. \
Be direct, specific, and useful. Show your work in the chat. When a task is complete, stop.";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentRole {
    /// The chat agent. Always present, never a user-editable personality
    /// in the global roster. The session runs as the Orchestrator; the
    /// orchestrator prompt is a code constant (see `ORCHESTRATOR_PROMPT`).
    Orchestrator,
    Coder,
    Architect,
    Researcher,
    HealthMonitor,
}

impl AgentRole {
    pub fn default_prompt(&self) -> &str {
        match self {
            AgentRole::Orchestrator => ORCHESTRATOR_PROMPT,
            AgentRole::Coder => "You are a senior software engineer. You write clean, efficient code and follow best practices.",
            AgentRole::Architect => "You are a system architect. You design systems, evaluate trade-offs, and create technical specifications.",
            AgentRole::Researcher => "You are a technical researcher. You gather information, analyze options, and provide well-sourced recommendations.",
            AgentRole::HealthMonitor => "You are a health monitor. You watch for stalled agents, hallucinated outputs, and loops. You flag issues and suggest corrections.",
        }
    }

    /// All role variants, in the canonical order used by the Agent
    /// Manager UI. New variants must be appended so existing saved
    /// personalities keep their numeric index.
    pub fn all() -> [AgentRole; 5] {
        [
            AgentRole::Orchestrator,
            AgentRole::Coder,
            AgentRole::Architect,
            AgentRole::Researcher,
            AgentRole::HealthMonitor,
        ]
    }

    /// Roles that can back a user-editable personality in the global
    /// roster. The Orchestrator is the chat agent and is NOT a
    /// personality — it's hardcoded into the app. The other four are
    /// subagent roles the user can rename, re-prompt, and bind to saved
    /// models.
    pub fn subagent_roles() -> [AgentRole; 4] {
        [
            AgentRole::Coder,
            AgentRole::Architect,
            AgentRole::Researcher,
            AgentRole::HealthMonitor,
        ]
    }

    /// Is this role exposed to the user as a subagent personality? The
    /// Orchestrator returns `false`; the four subagent roles return
    /// `true`. Used by the AgentManager UI and the personalities
    /// roster's `enforce_invariants` to filter the default seed set.
    pub fn is_subagent(&self) -> bool {
        !matches!(self, AgentRole::Orchestrator)
    }

    /// Default display name for a builtin personality carrying this
    /// role. The user can rename the personality in the Agent Manager;
    /// this is the seed value.
    pub fn display_name(&self) -> &str {
        match self {
            AgentRole::Orchestrator => "Orchestrator",
            AgentRole::Coder => "Coder",
            AgentRole::Architect => "Architect",
            AgentRole::Researcher => "Researcher",
            AgentRole::HealthMonitor => "Health Monitor",
        }
    }

    /// Resolve a role from a user-supplied slug (case- and separator-
    /// insensitive). Accepts "coder", "architect", "researcher",
    /// "health-monitor" / "health_monitor" / "healthmonitor", and
    /// "orchestrator". Used by the `delegate_subagent` tool so the
    /// orchestrator can target a role by name instead of a roster id.
    pub fn from_slug(slug: &str) -> Option<AgentRole> {
        let norm = slug
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>();
        match norm.as_str() {
            "orchestrator" => Some(AgentRole::Orchestrator),
            "coder" => Some(AgentRole::Coder),
            "architect" => Some(AgentRole::Architect),
            "researcher" => Some(AgentRole::Researcher),
            "healthmonitor" => Some(AgentRole::HealthMonitor),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentStatus {
    Idle,
    Working,
    WaitingForInput,
    Error(String),
    Stopped,
}

/// Lifecycle status of a dispatched subagent turn. `Error` carries the
/// failure detail in the record's `error` field (kept separate so the
/// enum stays `Eq`-friendly on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubagentStatus {
    Working,
    Done,
    Cancelled,
    Error,
}

/// Registry entry for one dispatched subagent turn. Lives in
/// `AppState.subagents`, keyed by the turn's `sub-<uuid>` request id.
/// The canvas subscribes to the streamed `agent-*` events for live
/// content; this record is the introspection surface (`list_subagents`)
/// and the WebView-reconnect resync source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentRecord {
    pub request_id: String,
    /// Roster personality id the subagent ran as.
    pub personality_id: String,
    /// Human-readable name (personality name, may be user-renamed).
    pub name: String,
    pub role: AgentRole,
    /// The delegation task text the orchestrator handed over.
    pub task: String,
    pub status: SubagentStatus,
    /// Saved-model id the subagent turn ran against. Surfaced in the
    /// window header so the user can see which model the agent used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// requestId of the orchestrator turn that dispatched this subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_request_id: Option<String>,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// Final answer text on success (capped to `CONTENT_MAX`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_content: Option<String>,
    /// Failure detail on error / cancellation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SubagentRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn working(
        request_id: &str,
        personality_id: &str,
        name: &str,
        role: AgentRole,
        task: &str,
        model_id: Option<&str>,
        parent_request_id: Option<&str>,
    ) -> Self {
        Self {
            request_id: request_id.to_string(),
            personality_id: personality_id.to_string(),
            name: name.to_string(),
            role,
            task: task.to_string(),
            status: SubagentStatus::Working,
            model_id: model_id.map(|s| s.to_string()),
            parent_request_id: parent_request_id.map(|s| s.to_string()),
            started_at: chrono::Utc::now().to_rfc3339(),
            finished_at: None,
            final_content: None,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

pub struct Agent {
    pub id: String,
    pub name: String,
    pub role: AgentRole,
    /// Part of the agent data model; consumed once the multi-agent loop
    /// uses sub-agent prompts.
    #[allow(dead_code)]
    pub system_prompt: String,
    pub status: AgentStatus,
    pub messages: Vec<AgentMessage>,
    /// Multi-agent hierarchy — reserved for the orchestrator loop.
    #[allow(dead_code)]
    pub sub_agents: Vec<String>,
    #[allow(dead_code)]
    pub parent_id: Option<String>,
}

impl Agent {
    pub fn new(id: &str, name: &str, role: AgentRole, system_prompt: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            role: role,
            system_prompt: if system_prompt.is_empty() {
                role.default_prompt().to_string()
            } else {
                system_prompt.to_string()
            },
            status: AgentStatus::Idle,
            messages: Vec::new(),
            sub_agents: Vec::new(),
            parent_id: None,
        }
    }

    pub async fn process_message(&mut self, input: &str) -> AgentMessage {
        self.status = AgentStatus::Working;

        // Record user message
        let user_msg = AgentMessage {
            role: "user".to_string(),
            content: input.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        self.messages.push(user_msg);

        // In a real implementation, this calls the LLM API via the stored API key
        // For now, return an acknowledgment
        let response = AgentMessage {
            role: "assistant".to_string(),
            content: format!("[{}] Received. Processing task: {}", self.name, input),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        self.messages.push(response.clone());

        self.status = AgentStatus::Idle;
        response
    }
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentRole {
    Orchestrator,
    Coder,
    Architect,
    Researcher,
    HealthMonitor,
}

impl AgentRole {
    pub fn default_prompt(&self) -> &str {
        match self {
            AgentRole::Orchestrator => "You are the orchestrator. You coordinate sub-agents, break down goals into tasks, and synthesize results.",
            AgentRole::Coder => "You are a senior software engineer. You write clean, efficient code and follow best practices.",
            AgentRole::Architect => "You are a system architect. You design systems, evaluate trade-offs, and create technical specifications.",
            AgentRole::Researcher => "You are a technical researcher. You gather information, analyze options, and provide well-sourced recommendations.",
            AgentRole::HealthMonitor => "You are a health monitor. You watch for stalled agents, hallucinated outputs, and loops. You flag issues and suggest corrections.",
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
            role: role.clone(),
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

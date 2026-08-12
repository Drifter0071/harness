//! Per-project agent roster (`<project>/.harness/agents.json`).
//!
//! The roster holds the agent personalities the user manages in the Agent
//! Manager window plus the max-agents-per-session limit. This sprint runs a
//! single capable agent per session (snapshotted on the `Session`), but the
//! roster shape is the seam the Orchestrator → sub-agent dispatch will use
//! next sprint. Builtin personas are editable but cannot be deleted; that
//! rule is enforced at save time so a stray frontend payload can't remove
//! them.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;

use crate::agent::AgentRole;
use crate::session::harness_dir;

const ROSTER_VERSION: u32 = 1;
/// Clamp floor so a session can never be configured to run zero agents.
const MIN_AGENTS_PER_SESSION: u32 = 1;
/// Builtin persona id (single capable agent this sprint).
pub const BUILTIN_AGENT_ID: &str = "harness";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPersonality {
    pub id: String,
    pub name: String,
    pub role: AgentRole,
    pub system_prompt: String,
    /// Builtin personas are editable but cannot be deleted.
    #[serde(default)]
    pub builtin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRoster {
    pub version: u32,
    #[serde(default = "default_max_agents")]
    pub max_agents_per_session: u32,
    pub default_agent_id: String,
    pub agents: Vec<AgentPersonality>,
}

fn default_max_agents() -> u32 {
    4
}

fn builtin_orchestrator() -> AgentPersonality {
    AgentPersonality {
        id: BUILTIN_AGENT_ID.into(),
        name: BUILTIN_AGENT_ID.into(),
        role: AgentRole::Orchestrator,
        system_prompt: AgentRole::Orchestrator.default_prompt().to_string(),
        builtin: true,
    }
}

impl Default for AgentRoster {
    fn default() -> Self {
        let orchestrator = builtin_orchestrator();
        Self {
            version: ROSTER_VERSION,
            max_agents_per_session: default_max_agents(),
            default_agent_id: orchestrator.id.clone(),
            agents: vec![orchestrator],
        }
    }
}

fn roster_path(project_path: &str) -> PathBuf {
    harness_dir(project_path).join("agents.json")
}

/// Load the per-project roster. On first contact (or after a corrupt file
/// has been archived) this creates and persists the default roster.
pub async fn load_agent_roster(project_path: &str) -> Result<AgentRoster, String> {
    let path = roster_path(project_path);
    if !path.exists() {
        let roster = AgentRoster::default();
        save_agent_roster(project_path, &roster).await?;
        return Ok(roster);
    }
    let raw = match fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) => return Err(format!("Failed to read agent roster: {}", e)),
    };
    match serde_json::from_str::<AgentRoster>(&raw) {
        Ok(mut roster) => {
            enforce_invariants(&mut roster);
            Ok(roster)
        }
        Err(e) => {
            eprintln!("[harness] corrupt agent roster ({}); archiving", e);
            let ts = chrono::Utc::now().timestamp();
            let archived =
                path.with_file_name(format!("agents.v{}.archived-{}.json", ROSTER_VERSION, ts));
            if let Err(rename_err) = fs::rename(&path, &archived).await {
                eprintln!("[harness] failed to archive corrupt roster: {}", rename_err);
            }
            let roster = AgentRoster::default();
            save_agent_roster(project_path, &roster).await?;
            Ok(roster)
        }
    }
}

/// Persist the roster atomically (tmp + rename) and return the normalized
/// roster. Enforces: version pinned, max >= 1, a default that exists, and
/// builtin agents restored if a payload tried to remove them.
pub async fn save_agent_roster(
    project_path: &str,
    roster: &AgentRoster,
) -> Result<AgentRoster, String> {
    let mut roster = roster.clone();
    enforce_invariants(&mut roster);
    if roster.agents.is_empty() {
        return Err("Roster must contain at least one agent".into());
    }
    let dir = harness_dir(project_path);
    fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("Failed to create harness dir: {}", e))?;
    let json = serde_json::to_string_pretty(&roster)
        .map_err(|e| format!("Failed to serialize agent roster: {}", e))?;
    let final_path = dir.join("agents.json");
    let tmp_path = dir.join("agents.json.tmp");
    fs::write(&tmp_path, json)
        .await
        .map_err(|e| format!("Failed to write tmp roster file: {}", e))?;
    fs::rename(&tmp_path, &final_path)
        .await
        .map_err(|e| format!("Failed to rename tmp roster file: {}", e))?;
    Ok(roster)
}

/// Normalize a roster in place: pin version, clamp max >= 1, ensure the
/// default id points at an existing agent, and re-insert builtins that were
/// dropped (edits to a builtin are preserved; the agent itself is not
/// deletable).
fn enforce_invariants(roster: &mut AgentRoster) {
    roster.version = ROSTER_VERSION;
    roster.max_agents_per_session = roster.max_agents_per_session.max(MIN_AGENTS_PER_SESSION);
    if roster.agents.is_empty() {
        roster.agents.push(builtin_orchestrator());
    }
    // Restore builtin agents that a save payload tried to delete.
    if !roster.agents.iter().any(|a| a.id == BUILTIN_AGENT_ID) {
        roster.agents.insert(0, builtin_orchestrator());
    }
    if !roster
        .agents
        .iter()
        .any(|a| a.id == roster.default_agent_id)
    {
        roster.default_agent_id = roster.agents[0].id.clone();
    }
}

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn default_roster_has_builtin_orchestrator() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let roster = load_agent_roster(&path).await.unwrap();
        assert_eq!(roster.version, 1);
        assert_eq!(roster.max_agents_per_session, 4);
        assert_eq!(roster.default_agent_id, "harness");
        assert_eq!(roster.agents.len(), 1);
        assert!(roster.agents[0].builtin);
        assert_eq!(roster.agents[0].role, AgentRole::Orchestrator);
        // The file was persisted on first contact.
        assert!(roster_path(&path).exists());
    }

    #[tokio::test]
    async fn roster_roundtrip_preserves_user_agents() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let mut roster = AgentRoster::default();
        roster.agents.push(AgentPersonality {
            id: "coder1".into(),
            name: "Coder One".into(),
            role: AgentRole::Coder,
            system_prompt: "custom coder".into(),
            builtin: false,
        });
        roster.default_agent_id = "coder1".into();
        roster.max_agents_per_session = 3;
        let saved = save_agent_roster(&path, &roster).await.unwrap();
        assert_eq!(saved.agents.len(), 2);

        let loaded = load_agent_roster(&path).await.unwrap();
        assert_eq!(loaded.agents.len(), 2);
        assert_eq!(loaded.default_agent_id, "coder1");
        assert_eq!(loaded.max_agents_per_session, 3);
        let coder = loaded.agents.iter().find(|a| a.id == "coder1").unwrap();
        assert_eq!(coder.system_prompt, "custom coder");
        assert!(!coder.builtin);
        let builtin = loaded.agents.iter().find(|a| a.id == "harness").unwrap();
        assert!(builtin.builtin);
    }

    #[tokio::test]
    async fn corrupt_roster_is_archived_and_replaced() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let rp = roster_path(&path);
        std::fs::create_dir_all(rp.parent().unwrap()).unwrap();
        std::fs::write(&rp, "{ not json !!!").unwrap();

        let roster = load_agent_roster(&path).await.unwrap();
        assert_eq!(roster.agents.len(), 1);
        assert_eq!(roster.agents[0].id, "harness");
        // Corrupt file was archived, not silently destroyed.
        let archived = std::fs::read_dir(rp.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains("archived"));
        assert!(archived);
        // Fresh default was persisted.
        assert!(roster_path(&path).exists());
    }

    #[tokio::test]
    async fn max_agents_clamped_to_at_least_one() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let mut roster = AgentRoster::default();
        roster.max_agents_per_session = 0;
        let saved = save_agent_roster(&path, &roster).await.unwrap();
        assert_eq!(saved.max_agents_per_session, 1);

        let loaded = load_agent_roster(&path).await.unwrap();
        assert_eq!(loaded.max_agents_per_session, 1);
    }

    #[tokio::test]
    async fn builtin_not_deletable_enforced_at_save() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();

        // A payload that drops the builtin entirely.
        let mut roster = AgentRoster::default();
        roster.agents = vec![AgentPersonality {
            id: "only-coder".into(),
            name: "Only Coder".into(),
            role: AgentRole::Coder,
            system_prompt: "coder".into(),
            builtin: false,
        }];
        roster.default_agent_id = "only-coder".into();
        let saved = save_agent_roster(&path, &roster).await.unwrap();

        // Builtin is restored; edits to it (none here) would be preserved.
        assert!(saved.agents.iter().any(|a| a.id == "harness" && a.builtin));
        assert_eq!(saved.agents.len(), 2);
        assert_eq!(saved.default_agent_id, "only-coder");

        // Edits to the builtin survive a save.
        let mut again = saved.clone();
        let builtin = again.agents.iter_mut().find(|a| a.id == "harness").unwrap();
        builtin.name = "Harness (custom)".into();
        let saved2 = save_agent_roster(&path, &again).await.unwrap();
        let builtin2 = saved2.agents.iter().find(|a| a.id == "harness").unwrap();
        assert_eq!(builtin2.name, "Harness (custom)");
    }

    #[tokio::test]
    async fn default_agent_id_repaired_when_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let mut roster = AgentRoster::default();
        roster.default_agent_id = "does-not-exist".into();
        let saved = save_agent_roster(&path, &roster).await.unwrap();
        assert!(saved.agents.iter().any(|a| a.id == saved.default_agent_id));
    }
}

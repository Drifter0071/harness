//! Global agent personalities (`<data_dir>/Harness/personalities.json`).
//!
//! The roster is **global** — it lives in the OS data directory and is
//! shared across every project the user opens. Each personality carries:
//!
//! - a custom display name (e.g. "Bip", "Senior Coder")
//! - a role (one of the four subagent roles from `AgentRole`)
//! - a system prompt (editable; defaults to `AgentRole::default_prompt`)
//! - a saved-model id (the `SavedModel` to use when the personality runs;
//!   `None` means "fall back to the active chat model")
//!
//! Builtin personalities are seeded on first contact: one per subagent
//! role (Coder, Architect, Researcher, HealthMonitor), each with a
//! stable id and the role's default prompt. The **Orchestrator** is NOT
//! in the roster — it's the hardcoded chat agent the user talks to
//! directly. Builtin subagents are editable but cannot be deleted;
//! that's enforced at save time so a stray frontend payload can't drop
//! them.
//!
//! Future direction: the orchestrator loop will read this roster live
//! to dispatch subagents. For v0 the roster exists so the user can
//! rename / re-prompt / bind models to each subagent persona ahead of
//! dispatch being wired up.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;

use crate::agent::AgentRole;

const ROSTER_VERSION: u32 = 2;
const MIN_PERSONALITIES: usize = 1;
const BUILTIN_PREFIX: &str = "builtin-";

/// Builtin personality id for a given subagent role. Stable across
/// versions. The Orchestrator role is NOT valid here — it's the chat
/// agent and has no roster row.
pub fn builtin_personality_id(role: AgentRole) -> String {
    assert!(
        role.is_subagent(),
        "builtin_personality_id called for non-subagent role {:?}",
        role
    );
    let slug = match role {
        AgentRole::Coder => "coder",
        AgentRole::Architect => "architect",
        AgentRole::Researcher => "researcher",
        AgentRole::HealthMonitor => "health-monitor",
        AgentRole::Orchestrator => unreachable!("guarded by assert above"),
    };
    format!("{BUILTIN_PREFIX}{slug}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPersonality {
    pub id: String,
    pub name: String,
    pub role: AgentRole,
    pub system_prompt: String,
    /// Saved-model id (`SavedModel.id`) to use when this personality runs.
    /// `None` means "use the active chat model".
    #[serde(default)]
    pub model_id: Option<String>,
    /// Builtin personalities are editable but cannot be deleted.
    #[serde(default)]
    pub builtin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalityRoster {
    pub version: u32,
    /// Stable id of the personality that the Agent Manager treats as
    /// the default. The chat agent itself (the Orchestrator) is not
    /// in the roster, so this is the default for the Agent Manager
    /// UX, not for sessions. Repaired on save if it points at a
    /// missing personality.
    pub default_personality_id: String,
    pub personalities: Vec<AgentPersonality>,
}

fn default_personalities() -> Vec<AgentPersonality> {
    // Only subagent roles — the Orchestrator is the chat agent and is
    // not a user-editable personality.
    AgentRole::subagent_roles()
        .iter()
        .map(|&role| AgentPersonality {
            id: builtin_personality_id(role),
            name: role.display_name().to_string(),
            role,
            system_prompt: role.default_prompt().to_string(),
            model_id: None,
            builtin: true,
        })
        .collect()
}

impl Default for PersonalityRoster {
    fn default() -> Self {
        let personalities = default_personalities();
        Self {
            version: ROSTER_VERSION,
            default_personality_id: personalities[0].id.clone(),
            personalities,
        }
    }
}

fn personalities_path() -> Result<PathBuf, String> {
    // `HARNESS_DATA_DIR_OVERRIDE` is a test-only escape hatch that
    // redirects the global config file into a tempdir. Production reads
    // from the OS data dir.
    if let Ok(p) = std::env::var("HARNESS_DATA_DIR_OVERRIDE") {
        return Ok(PathBuf::from(p).join("Harness").join("personalities.json"));
    }
    let base = dirs::data_dir().ok_or_else(|| "no data_dir on this platform".to_string())?;
    let dir = base.join("Harness");
    Ok(dir.join("personalities.json"))
}

async fn ensure_dir(path: &PathBuf) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create Harness data dir: {}", e))?;
    }
    Ok(())
}

/// Load the global personality roster. On first contact (or after a
/// corrupt file has been archived) this creates and persists the default
/// roster.
pub async fn load_personalities() -> Result<PersonalityRoster, String> {
    let path = personalities_path()?;
    if !path.exists() {
        let roster = PersonalityRoster::default();
        save_personalities(&roster).await?;
        return Ok(roster);
    }
    let raw = match fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) => return Err(format!("Failed to read personalities: {}", e)),
    };
    match serde_json::from_str::<PersonalityRoster>(&raw) {
        Ok(mut roster) => {
            enforce_invariants(&mut roster);
            Ok(roster)
        }
        Err(e) => {
            eprintln!("[harness] corrupt personalities file ({}); archiving", e);
            let ts = chrono::Utc::now().timestamp();
            let archived = path.with_file_name(format!(
                "personalities.v{}.archived-{}.json",
                ROSTER_VERSION, ts
            ));
            if let Err(rename_err) = fs::rename(&path, &archived).await {
                eprintln!("[harness] failed to archive corrupt personalities: {}", rename_err);
            }
            let roster = PersonalityRoster::default();
            save_personalities(&roster).await?;
            Ok(roster)
        }
    }
}

/// Persist the roster atomically (tmp + rename) and return the normalized
/// roster.
pub async fn save_personalities(roster: &PersonalityRoster) -> Result<PersonalityRoster, String> {
    let mut roster = roster.clone();
    enforce_invariants(&mut roster);
    if roster.personalities.is_empty() {
        return Err("Roster must contain at least one personality".into());
    }
    let final_path = personalities_path()?;
    ensure_dir(&final_path).await?;
    let json = serde_json::to_string_pretty(&roster)
        .map_err(|e| format!("Failed to serialize personalities: {}", e))?;
    let tmp_path = final_path.with_extension("json.tmp");
    fs::write(&tmp_path, json)
        .await
        .map_err(|e| format!("Failed to write tmp personalities file: {}", e))?;
    fs::rename(&tmp_path, &final_path)
        .await
        .map_err(|e| format!("Failed to rename tmp personalities file: {}", e))?;
    Ok(roster)
}

/// Look up one personality by id. Returns `None` if the id is unknown
/// (the caller decides what to do — usually fall back to the default
/// personality and emit a warning).
pub async fn get_personality(id: &str) -> Result<Option<AgentPersonality>, String> {
    let roster = load_personalities().await?;
    Ok(roster.personalities.into_iter().find(|p| p.id == id))
}

/// Normalize a roster in place: pin version, ensure every builtin role
/// is present, ensure the default id points at an existing personality,
/// repair any empty `system_prompt` on a builtin to its role default
/// (user edits are preserved).
fn enforce_invariants(roster: &mut PersonalityRoster) {
    roster.version = ROSTER_VERSION;
    // Make sure every builtin role has a row. Insert missing ones at the
    // top so the user sees them first.
    for builtin in default_personalities() {
        if !roster.personalities.iter().any(|p| p.id == builtin.id) {
            roster.personalities.insert(0, builtin);
        }
    }
    // Repair empty system prompts on builtins to the role default.
    // (User-set builtins with content are preserved verbatim.)
    for p in roster.personalities.iter_mut() {
        if p.builtin && p.system_prompt.trim().is_empty() {
            p.system_prompt = p.role.default_prompt().to_string();
        }
    }
    if roster.personalities.len() < MIN_PERSONALITIES {
        // Should be impossible after the loop above, but stay defensive.
        let extra = default_personalities()
            .into_iter()
            .find(|p| !roster.personalities.iter().any(|q| q.id == p.id));
        if let Some(p) = extra {
            roster.personalities.push(p);
        }
    }
    if !roster
        .personalities
        .iter()
        .any(|p| p.id == roster.default_personality_id)
    {
        roster.default_personality_id = roster.personalities[0].id.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Override the data dir for the duration of a test so the
    /// personalities file doesn't leak into the user's real data dir.
    /// Returns a guard that restores the previous value on drop.
    struct DataDirGuard {
        prev: Option<std::path::PathBuf>,
        _tmp: TempDir,
    }
    impl DataDirGuard {
        fn new() -> Self {
            let tmp = TempDir::new().unwrap();
            let prev = dirs::data_dir();
            // SAFETY: tests are single-threaded for this guard's lifetime.
            unsafe {
                std::env::set_var("HARNESS_DATA_DIR_OVERRIDE", tmp.path());
            }
            Self { prev, _tmp: tmp }
        }
    }
    impl Drop for DataDirGuard {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var("HARNESS_DATA_DIR_OVERRIDE");
            }
            let _ = self.prev.as_ref();
        }
    }

    /// `personalities_path` honours the `HARNESS_DATA_DIR_OVERRIDE` env
    /// var when set; tests use it to redirect the file to a tempdir.
    fn overridden_path() -> Result<PathBuf, String> {
        if let Ok(p) = std::env::var("HARNESS_DATA_DIR_OVERRIDE") {
            return Ok(PathBuf::from(p).join("Harness").join("personalities.json"));
        }
        personalities_path()
    }

    #[tokio::test]
    async fn default_roster_seeds_all_four_subagent_roles() {
        let _g = DataDirGuard::new();
        let roster = load_personalities().await.unwrap();
        assert_eq!(roster.version, 2);
        // Every subagent role is present.
        for role in AgentRole::subagent_roles() {
            assert!(
                roster.personalities.iter().any(|p| p.role == role),
                "default roster missing {:?}",
                role
            );
        }
        // The Orchestrator is NOT in the roster — it's the chat agent.
        assert!(
            !roster
                .personalities
                .iter()
                .any(|p| p.role == AgentRole::Orchestrator),
            "Orchestrator must not be in the roster"
        );
        // Default points at the Coder builtin (the first subagent).
        assert_eq!(
            roster.default_personality_id,
            builtin_personality_id(AgentRole::Coder)
        );
        // The file landed somewhere (env-overridden or real data_dir).
        assert!(overridden_path().unwrap().exists() || personalities_path().unwrap().exists());
    }

    #[tokio::test]
    async fn orchestrator_role_is_rejected_from_builtin_id() {
        // Defensive: the Orchestrator must never get a roster id.
        let result = std::panic::catch_unwind(|| {
            builtin_personality_id(AgentRole::Orchestrator)
        });
        assert!(result.is_err(), "expected panic for non-subagent role");
    }

    #[tokio::test]
    async fn user_personalities_roundtrip_with_model_id() {
        let _g = DataDirGuard::new();
        let mut roster = PersonalityRoster::default();
        roster.personalities.push(AgentPersonality {
            id: "user-coder".into(),
            name: "Senior Coder".into(),
            role: AgentRole::Coder,
            system_prompt: "Custom coder prompt.".into(),
            model_id: Some("model-abc".into()),
            builtin: false,
        });
        roster.default_personality_id = "user-coder".into();
        let saved = save_personalities(&roster).await.unwrap();
        assert_eq!(saved.personalities.len(), 5); // 4 builtins + 1 user
        let user = saved
            .personalities
            .iter()
            .find(|p| p.id == "user-coder")
            .unwrap();
        assert_eq!(user.model_id.as_deref(), Some("model-abc"));

        let loaded = load_personalities().await.unwrap();
        let user2 = loaded
            .personalities
            .iter()
            .find(|p| p.id == "user-coder")
            .unwrap();
        assert_eq!(user2.model_id.as_deref(), Some("model-abc"));
    }

    #[tokio::test]
    async fn empty_builtin_prompt_is_repaired() {
        let _g = DataDirGuard::new();
        let mut roster = PersonalityRoster::default();
        let coder = roster
            .personalities
            .iter_mut()
            .find(|p| p.role == AgentRole::Coder)
            .unwrap();
        coder.system_prompt = "   ".into(); // whitespace-only
        let saved = save_personalities(&roster).await.unwrap();
        let coder = saved
            .personalities
            .iter()
            .find(|p| p.role == AgentRole::Coder)
            .unwrap();
        assert_eq!(coder.system_prompt, AgentRole::Coder.default_prompt());
    }

    #[tokio::test]
    async fn corrupt_file_is_archived_and_replaced() {
        let _g = DataDirGuard::new();
        let p = overridden_path().unwrap();
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, "{ not json !!!").unwrap();

        let roster = load_personalities().await.unwrap();
        assert!(!roster.personalities.is_empty());
        // Corrupt file was archived, not silently destroyed.
        let archived = std::fs::read_dir(p.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains("archived"));
        assert!(archived);
    }

    #[tokio::test]
    async fn default_id_repaired_when_missing() {
        let _g = DataDirGuard::new();
        let mut roster = PersonalityRoster::default();
        roster.default_personality_id = "does-not-exist".into();
        let saved = save_personalities(&roster).await.unwrap();
        assert!(saved
            .personalities
            .iter()
            .any(|p| p.id == saved.default_personality_id));
    }

    #[tokio::test]
    async fn user_cannot_remove_a_builtin_via_save() {
        // Removing builtins from the payload is silently repaired: the
        // builtin is re-inserted on load. The user's edits to the
        // remaining builtins are preserved.
        let _g = DataDirGuard::new();
        let mut roster = PersonalityRoster::default();
        let coder = roster
            .personalities
            .iter()
            .position(|p| p.role == AgentRole::Coder)
            .unwrap();
        roster.personalities[coder].name = "Coder (custom)".into();
        // Drop a subagent builtin (HealthMonitor in this case) from the
        // payload. `enforce_invariants` re-inserts it.
        roster
            .personalities
            .retain(|p| p.role != AgentRole::HealthMonitor);
        let saved = save_personalities(&roster).await.unwrap();
        assert!(saved
            .personalities
            .iter()
            .any(|p| p.role == AgentRole::HealthMonitor && p.builtin));
        let coder = saved
            .personalities
            .iter()
            .find(|p| p.role == AgentRole::Coder)
            .unwrap();
        assert_eq!(coder.name, "Coder (custom)");
    }

    #[tokio::test]
    async fn get_personality_returns_none_for_unknown_id() {
        let _g = DataDirGuard::new();
        // Ensure a roster exists.
        let _ = load_personalities().await.unwrap();
        let result = get_personality("definitely-not-a-personality").await.unwrap();
        assert!(result.is_none());
    }
}

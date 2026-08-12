//! Saved models catalog + per-model preferences.
//!
//! Saved models are persisted at `<data_dir>/Harness/models.json` and
//! contain only metadata — no API keys. Keys live in the OS keyring
//! under the `Harness` service. The frontend's `hasKey` flag is
//! computed at read time via `SecretStore::has`.
//!
//! Per-model preferences (`ModelPrefs`: thinking on/off, reasoning
//! effort) live in a sibling file `<data_dir>/Harness/model_prefs.json`,
//! keyed by model id. This is a TS-side ergonomic concern — the Rust
//! side exposes typed read/write helpers.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::secrets::{SecretError, SecretStore};

// ===== Wire types =====

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedModel {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub format: String,
    pub base_url: String,
    pub model_name: String,
    pub has_key: bool,
    /// `true` when the keyring returned `Err` while checking this row
    /// (e.g. the OS credential store is locked). Lets the UI distinguish
    /// "no key" from "key unreachable" (spec §7.4).
    #[serde(default)]
    pub key_unreachable: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Input shape from the frontend. Same as `SavedModel` but without
/// `hasKey` (the server computes that) and without timestamps (the
/// server manages them).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedModelInput {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub format: String,
    pub base_url: String,
    pub model_name: String,
    /// `Some(true)` to mark the model as having a key without changing
    /// it (we trust the keyring). `Some(false)` to mark it as having
    /// no key (used when deleting the key on save). The default
    /// (`None`) leaves the keyring entry as-is.
    pub has_key: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SavedModelsFile {
    pub version: u32,
    pub models: Vec<SavedModelRecord>,
}

/// Persisted record (no `hasKey`; that's derived from the keyring).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedModelRecord {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub format: String,
    pub base_url: String,
    pub model_name: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPrefs {
    pub thinking_on: bool,
    pub reasoning_effort: ReasoningEffort,
}

impl Default for ModelPrefs {
    fn default() -> Self {
        Self {
            thinking_on: false,
            reasoning_effort: ReasoningEffort::Medium,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    /// Only used by tests today; kept for the frontend wire boundary.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn as_str(&self) -> &'static str {
        match self {
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelPrefsFile {
    pub version: u32,
    pub prefs: HashMap<String, ModelPrefs>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub ok: bool,
    pub kind: String,
    pub message: String,
}

// ===== Path helpers =====

fn data_dir() -> Result<PathBuf, String> {
    let base = dirs::data_dir().ok_or_else(|| "no data_dir on this platform".to_string())?;
    Ok(base.join("Harness"))
}

fn models_file() -> Result<PathBuf, String> {
    Ok(data_dir()?.join("models.json"))
}

fn prefs_file() -> Result<PathBuf, String> {
    Ok(data_dir()?.join("model_prefs.json"))
}

fn active_model_file() -> Result<PathBuf, String> {
    Ok(data_dir()?.join("active_model.txt"))
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

async fn ensure_data_dir() -> Result<PathBuf, String> {
    let dir = data_dir()?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("Failed to create {}: {}", dir.display(), e))?;
    Ok(dir)
}

// ===== Saved models I/O =====

pub async fn read_models_file() -> Result<SavedModelsFile, String> {
    let path = models_file()?;
    if !path.exists() {
        return Ok(SavedModelsFile {
            version: 1,
            models: Vec::new(),
        });
    }
    let raw = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[harness] read models.json error: {}", e);
            return Ok(SavedModelsFile {
                version: 1,
                models: Vec::new(),
            });
        }
    };
    match serde_json::from_str::<SavedModelsFile>(&raw) {
        Ok(f) => Ok(f),
        Err(e) => {
            eprintln!("[harness] corrupt models.json ({}); ignoring", e);
            Ok(SavedModelsFile {
                version: 1,
                models: Vec::new(),
            })
        }
    }
}

async fn write_models_file(file: &SavedModelsFile) -> Result<(), String> {
    let path = models_file()?;
    ensure_data_dir().await?;
    let json = serde_json::to_string_pretty(file)
        .map_err(|e| format!("Failed to serialize models.json: {}", e))?;
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, json)
        .await
        .map_err(|e| format!("Failed to write tmp models.json: {}", e))?;
    tokio::fs::rename(&tmp, &path)
        .await
        .map_err(|e| format!("Failed to rename tmp models.json: {}", e))?;
    Ok(())
}

pub async fn list_saved_models(secrets: &dyn SecretStore) -> Result<Vec<SavedModel>, String> {
    let file = read_models_file().await?;
    let mut out = Vec::with_capacity(file.models.len());
    for r in file.models {
        // Three-state key status: Ok(true/false) is a legitimate
        // presence/absence; Err means the keyring is unreachable and is
        // surfaced as `key_unreachable` so the UI can offer "Reset
        // secret store" instead of pretending the key is missing.
        let (has_key, key_unreachable) = match secrets.has(&r.id) {
            Ok(v) => (v, false),
            Err(_) => (false, true),
        };
        out.push(SavedModel {
            id: r.id,
            label: r.label,
            provider: r.provider,
            format: r.format,
            base_url: r.base_url,
            model_name: r.model_name,
            has_key,
            key_unreachable,
            created_at: r.created_at,
            updated_at: r.updated_at,
        });
    }
    Ok(out)
}

pub async fn save_saved_model(
    secrets: &dyn SecretStore,
    input: SavedModelInput,
    api_key: Option<&str>,
) -> Result<SavedModel, String> {
    let mut file = read_models_file().await?;
    let now = now_iso();

    // Find existing record by id (upsert).
    let mut existing_idx: Option<usize> = None;
    for (i, m) in file.models.iter().enumerate() {
        if m.id == input.id {
            existing_idx = Some(i);
            break;
        }
    }

    let record = match existing_idx {
        Some(i) => {
            let prev = &file.models[i];
            SavedModelRecord {
                id: input.id.clone(),
                label: if input.label.is_empty() {
                    prev.label.clone()
                } else {
                    input.label.clone()
                },
                provider: if input.provider.is_empty() {
                    prev.provider.clone()
                } else {
                    input.provider.clone()
                },
                format: if input.format.is_empty() {
                    prev.format.clone()
                } else {
                    input.format.clone()
                },
                base_url: if input.base_url.is_empty() {
                    prev.base_url.clone()
                } else {
                    input.base_url.clone()
                },
                model_name: if input.model_name.is_empty() {
                    prev.model_name.clone()
                } else {
                    input.model_name.clone()
                },
                created_at: prev.created_at.clone(),
                updated_at: now.clone(),
            }
        }
        None => SavedModelRecord {
            id: input.id.clone(),
            label: input.label.clone(),
            provider: input.provider.clone(),
            format: input.format.clone(),
            base_url: input.base_url.clone(),
            model_name: input.model_name.clone(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    };

    // Apply to file.
    match existing_idx {
        Some(i) => file.models[i] = record.clone(),
        None => file.models.push(record.clone()),
    }
    write_models_file(&file).await?;

    // Now the keyring. Three states from the frontend:
    //   - `api_key` Some(non-empty) → overwrite
    //   - `api_key` Some("")        → no change (keep existing)
    //   - `api_key` None            → no change (TS omitted the field)
    if let Some(k) = api_key {
        if !k.is_empty() {
            secrets
                .set(&input.id, k)
                .map_err(|e| format!("Failed to write key: {}", e))?;
        }
    }

    // hasKey from the keyring
    let has_key = secrets
        .has(&input.id)
        .map_err(|e| format!("Keyring unreachable: {}", e))?;

    Ok(SavedModel {
        id: record.id,
        label: record.label,
        provider: record.provider,
        format: record.format,
        base_url: record.base_url,
        model_name: record.model_name,
        has_key,
        key_unreachable: false,
        created_at: record.created_at,
        updated_at: record.updated_at,
    })
}

pub async fn delete_saved_model(secrets: &dyn SecretStore, id: &str) -> Result<(), String> {
    let mut file = read_models_file().await?;
    file.models.retain(|m| m.id != id);
    write_models_file(&file).await?;
    // Keyring delete is idempotent; ignore NotFound.
    let _ = secrets.delete(id);
    Ok(())
}

// ===== Active model (single value, localStorage-equivalent) =====

pub fn read_active_model() -> Option<String> {
    let path = active_model_file().ok()?;
    std::fs::read_to_string(&path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn write_active_model(id: Option<&str>) {
    if let Ok(dir) = data_dir() {
        let _ = std::fs::create_dir_all(&dir);
    }
    if let Ok(path) = active_model_file() {
        match id {
            Some(s) => {
                let _ = std::fs::write(&path, s);
            }
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

// ===== Per-model prefs =====

pub fn read_model_prefs(id: &str) -> ModelPrefs {
    let path = match prefs_file() {
        Ok(p) => p,
        Err(_) => return ModelPrefs::default(),
    };
    if !path.exists() {
        return ModelPrefs::default();
    }
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return ModelPrefs::default(),
    };
    let file: ModelPrefsFile = match serde_json::from_str(&raw) {
        Ok(f) => f,
        Err(_) => return ModelPrefs::default(),
    };
    file.prefs.get(id).cloned().unwrap_or_default()
}

pub fn write_model_prefs(id: &str, prefs: &ModelPrefs) {
    if let Ok(dir) = data_dir() {
        let _ = std::fs::create_dir_all(&dir);
    }
    let path = match prefs_file() {
        Ok(p) => p,
        Err(_) => return,
    };
    let mut file: ModelPrefsFile = if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        ModelPrefsFile::default()
    };
    if file.prefs.is_empty() && file.version == 0 {
        file.version = 1;
    }
    file.prefs.insert(id.to_string(), prefs.clone());
    if let Ok(json) = serde_json::to_string_pretty(&file) {
        let _ = std::fs::write(&path, json);
    }
}

// ===== Test connection =====

pub async fn test_saved_model(secrets: &dyn SecretStore, id: &str) -> Result<TestResult, String> {
    let file = read_models_file().await?;
    let record = file
        .models
        .iter()
        .find(|m| m.id == id)
        .ok_or_else(|| format!("Model {} not found", id))?
        .clone();

    let api_key = match secrets.get(&record.id) {
        Ok(Some(k)) => k,
        Ok(None) => {
            return Ok(TestResult {
                ok: false,
                kind: "no_key".to_string(),
                message: "No API key saved for this model".to_string(),
            });
        }
        Err(SecretError::Unreachable(s)) => {
            return Ok(TestResult {
                ok: false,
                kind: "unreachable".to_string(),
                message: format!("Secret store unreachable: {}", s),
            });
        }
        Err(e) => {
            return Ok(TestResult {
                ok: false,
                kind: "unreachable".to_string(),
                message: e.to_string(),
            });
        }
    };

    crate::llm_client::test_connection(&record, &api_key).await
}

// ===== Tests =====

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::InMemoryStore;

    fn fake_record(id: &str, label: &str) -> SavedModelRecord {
        SavedModelRecord {
            id: id.to_string(),
            label: label.to_string(),
            provider: "anthropic".to_string(),
            format: "anthropic-messages".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            model_name: "claude-sonnet-4-20250514".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn model_prefs_default_is_medium_off() {
        let p = ModelPrefs::default();
        assert!(!p.thinking_on);
        assert_eq!(p.reasoning_effort, ReasoningEffort::Medium);
    }

    #[test]
    fn reasoning_effort_string_roundtrip() {
        for (val, expected) in [
            (ReasoningEffort::Low, "low"),
            (ReasoningEffort::Medium, "medium"),
            (ReasoningEffort::High, "high"),
        ] {
            assert_eq!(val.as_str(), expected);
            let json = serde_json::to_string(&val).unwrap();
            assert_eq!(json, format!("\"{}\"", expected));
        }
    }

    #[test]
    fn saved_model_input_round_trip() {
        // Verify camelCase → snake_case at the wire boundary.
        let input = SavedModelInput {
            id: "abc".to_string(),
            label: "Sonnet 4".to_string(),
            provider: "anthropic".to_string(),
            format: "anthropic-messages".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            model_name: "claude-sonnet-4-20250514".to_string(),
            has_key: Some(true),
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("\"baseUrl\""));
        assert!(json.contains("\"modelName\""));
        assert!(json.contains("\"hasKey\""));

        let from_ts = r#"{
            "id": "abc",
            "label": "Sonnet 4",
            "provider": "anthropic",
            "format": "anthropic-messages",
            "baseUrl": "https://api.anthropic.com",
            "modelName": "claude-sonnet-4-20250514"
        }"#;
        let parsed: SavedModelInput = serde_json::from_str(from_ts).unwrap();
        assert_eq!(parsed.id, "abc");
        assert!(parsed.has_key.is_none());
    }

    #[test]
    fn secret_error_display() {
        let _ = InMemoryStore::new();
        let s = format!("{}", SecretError::NotFound);
        assert!(s.contains("not found"));
        let s = format!("{}", SecretError::Unreachable("denied".into()));
        assert!(s.contains("denied"));
    }

    #[test]
    fn fake_record_helper() {
        let r = fake_record("id1", "label1");
        assert_eq!(r.id, "id1");
        assert_eq!(r.label, "label1");
    }
}

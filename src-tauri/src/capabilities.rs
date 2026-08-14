//! Capability probes for saved models.
//!
//! The goal is to discover (or sanity-check) each model's context
//! window and reasoning support without forcing the user to maintain
//! a catalog that goes stale every week. Two strategies:
//!
//! 1. **Anthropic Models API** — when the saved model uses the
//!    `anthropic-messages` format, hit `GET {base_url}/v1/models/{id}`
//!    and parse the `capabilities` tree (max_input_tokens, thinking,
//!    effort, etc.). This is the real source of truth from Anthropic
//!    and avoids the catalog.
//! 2. **OpenAI-compat smoke probe** — for `openai-chat-completions`,
//!    the upstream `/v1/models` endpoint does NOT expose capability
//!    flags, so we instead send a tiny `chat/completions` request with
//!    `reasoning_effort: "low"` and observe whether the model rejects
//!    it. The verdict is then persisted on the saved model record so
//!    we don't keep re-probing.
//!
//! Both probes are explicit (the frontend triggers them via a button);
//! the user always has the last word via the manual override in
//! Settings.

use serde::Deserialize;
use serde_json::Value;

use crate::models::{CapabilityProbe, ReasoningCapability, ReasoningEffort};

/// Result of a capability probe. `Ok` carries a fully-formed
/// `CapabilityProbe`; `Err` carries a human-readable reason that
/// Settings surfaces inline.
pub type ProbeResult = Result<CapabilityProbe, String>;

/// Probe the capabilities of a saved model. Selects the strategy
/// based on the format string (`anthropic-messages` vs
/// `openai-chat-completions`); unknown formats return
/// `Err("unsupported format")`.
///
/// `model_name` is the wire-side model id (e.g. `claude-sonnet-4-6`).
/// `base_url` is the provider base URL with no trailing slash.
/// `api_key` may be empty for some gateways (most OpenAI-compat
/// providers still expect one).
pub async fn probe_capabilities(
    format: &str,
    base_url: &str,
    model_name: &str,
    api_key: &str,
) -> ProbeResult {
    match format {
        "anthropic-messages" => probe_anthropic(base_url, model_name, api_key).await,
        "openai-chat-completions" => probe_openai_smoke(base_url, model_name, api_key).await,
        other => Err(format!("unsupported format: {other}")),
    }
}

// ===== Anthropic Models API =====

/// Shape of a `GET /v1/models/{id}` response from Anthropic.
/// We only deserialize the fields we care about; the rest is ignored.
#[derive(Debug, Deserialize)]
struct AnthropicModelEnvelope {
    #[serde(default)]
    max_input_tokens: Option<u64>,
    #[serde(default)]
    capabilities: Option<Value>,
}

async fn probe_anthropic(base_url: &str, model_name: &str, api_key: &str) -> ProbeResult {
    let url = format!(
        "{}/v1/models/{}",
        base_url.trim_end_matches('/'),
        urlencode_path_segment(model_name)
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("probe client: {e}"))?;

    let mut req = client.get(&url);
    if !api_key.is_empty() {
        req = req
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01");
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("probe request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("Anthropic Models API returned {status}"));
    }
    let envelope: AnthropicModelEnvelope = resp
        .json()
        .await
        .map_err(|e| format!("probe response parse: {e}"))?;

    let (reasoning, reasoning_levels) =
        parse_anthropic_capabilities(envelope.capabilities.as_ref());

    let probed_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    Ok(CapabilityProbe {
        probed_at,
        source: "anthropic-models-api".to_string(),
        context_window: envelope.max_input_tokens.map(|n| n as u32),
        reasoning: Some(reasoning),
        reasoning_levels,
        note: None,
    }
    .normalize())
}

/// Parse the Anthropic `capabilities` JSON tree into our internal
/// enum. The tree is nested; we use bracket access to walk it because
/// the leaves are untyped `{"supported": bool}` dicts.
///
/// Layout (verified against Anthropic docs 2026):
/// ```json
/// {
///   "thinking": {"supported": true, "types": {"enabled": {"supported": false}, "adaptive": {"supported": true}}},
///   "effort": {"supported": true, "low": {"supported": true}, "medium": {"supported": true}, "high": {"supported": true}, "max": {"supported": true}}
/// }
/// ```
fn parse_anthropic_capabilities(
    caps: Option<&Value>,
) -> (ReasoningCapability, Option<Vec<ReasoningEffort>>) {
    let Some(caps) = caps else {
        return (ReasoningCapability::None, None);
    };

    // Thinking branch: Anthropic's extended/adaptive thinking.
    let thinking_supported = caps
        .pointer("/thinking/supported")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if thinking_supported {
        return (ReasoningCapability::Thinking, None);
    }

    // Effort branch: levels live under caps.effort.<level>.supported.
    let effort_root = caps.pointer("/effort");
    if let Some(effort) = effort_root {
        let mut levels = Vec::new();
        for (name, target) in [
            ("low", ReasoningEffort::Low),
            ("medium", ReasoningEffort::Medium),
            ("high", ReasoningEffort::High),
        ] {
            if effort
                .pointer(&format!("/{name}/supported"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                levels.push(target);
            }
        }
        if !levels.is_empty() {
            return (ReasoningCapability::Effort, Some(levels));
        }
    }

    (ReasoningCapability::None, None)
}

// ===== OpenAI-compat smoke probe =====

/// Send a 1-token request with `reasoning_effort: "low"` and see if
/// the model accepts it. We treat a 2xx response as "supports effort"
/// and a 400 mentioning `reasoning_effort` as "doesn't support it".
/// Everything else (auth, network, other 4xx/5xx) bubbles up as an
/// error so the UI can show the actual reason.
async fn probe_openai_smoke(base_url: &str, model_name: &str, api_key: &str) -> ProbeResult {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("probe client: {e}"))?;

    let body = serde_json::json!({
        "model": model_name,
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 1,
        "stream": false,
        "reasoning_effort": "low",
    });

    let mut req = client.post(&url).json(&body);
    if !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("probe request failed: {e}"))?;

    let status = resp.status();
    let body_text = resp
        .text()
        .await
        .map_err(|e| format!("probe read body: {e}"))?;

    let (reasoning, levels) = if status.is_success() {
        // 2xx — the model accepted reasoning_effort. We don't know
        // the full level set without further probing, so the UI
        // defaults to low/medium/high (canonical).
        (ReasoningCapability::Effort, None)
    } else if status.as_u16() == 400 && body_text.to_lowercase().contains("reasoning_effort") {
        // 400 specifically about reasoning_effort → model doesn't
        // accept the param. Treat as plain chat.
        (ReasoningCapability::None, None)
    } else {
        return Err(format!(
            "probe returned {status}: {}",
            truncate(&body_text, 200)
        ));
    };

    let probed_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let note = if matches!(reasoning, ReasoningCapability::Effort) {
        Some("Smoke probe confirmed reasoning_effort is accepted.".to_string())
    } else {
        Some("Smoke probe: model rejected reasoning_effort.".to_string())
    };

    Ok(CapabilityProbe {
        probed_at,
        source: "smoke-test".to_string(),
        context_window: None, // The smoke probe can't discover this; left for a future /v1/models call.
        reasoning: Some(reasoning),
        reasoning_levels: levels,
        note,
    }
    .normalize())
}

// ===== Small utilities =====

impl CapabilityProbe {
    /// Drop empty `None`s and clamp the level set to canonical values.
    /// We keep this in a method so callers can chain `.normalize()`.
    fn normalize(mut self) -> Self {
        if let Some(levels) = self.reasoning_levels.as_mut() {
            levels.sort_by_key(|l| match l {
                ReasoningEffort::Low => 0,
                ReasoningEffort::Medium => 1,
                ReasoningEffort::High => 2,
            });
            levels.dedup();
        }
        self
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

fn urlencode_path_segment(s: &str) -> String {
    // Minimal path-segment encoder: we only need to escape the few
    // characters that are illegal in URL path segments and that
    // could conceivably appear in a model name.
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            other => {
                let mut buf = [0u8; 4];
                let bytes = other.encode_utf8(&mut buf).as_bytes();
                for b in bytes {
                    out.push_str(&format!("%{:02X}", b));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_anthropic_thinking_only() {
        let caps = serde_json::json!({
            "thinking": {"supported": true, "types": {"adaptive": {"supported": true}}},
            "effort": {"supported": false}
        });
        let (r, levels) = parse_anthropic_capabilities(Some(&caps));
        assert_eq!(r, ReasoningCapability::Thinking);
        assert!(levels.is_none());
    }

    #[test]
    fn parses_anthropic_effort_with_levels() {
        let caps = serde_json::json!({
            "thinking": {"supported": false},
            "effort": {
                "supported": true,
                "low": {"supported": true},
                "medium": {"supported": true},
                "high": {"supported": false}
            }
        });
        let (r, levels) = parse_anthropic_capabilities(Some(&caps));
        assert_eq!(r, ReasoningCapability::Effort);
        assert_eq!(
            levels,
            Some(vec![ReasoningEffort::Low, ReasoningEffort::Medium])
        );
    }

    #[test]
    fn parses_anthropic_no_reasoning() {
        let caps = serde_json::json!({
            "thinking": {"supported": false},
            "effort": {"supported": false}
        });
        let (r, levels) = parse_anthropic_capabilities(Some(&caps));
        assert_eq!(r, ReasoningCapability::None);
        assert!(levels.is_none());
    }

    #[test]
    fn parses_anthropic_missing_caps() {
        let (r, levels) = parse_anthropic_capabilities(None);
        assert_eq!(r, ReasoningCapability::None);
        assert!(levels.is_none());
    }

    #[test]
    fn urlencode_passes_safe_chars() {
        assert_eq!(
            urlencode_path_segment("claude-sonnet-4-6"),
            "claude-sonnet-4-6"
        );
        assert_eq!(urlencode_path_segment("a/b"), "a%2Fb");
        assert_eq!(urlencode_path_segment("foo bar"), "foo%20bar");
    }

    #[test]
    fn truncate_short() {
        assert_eq!(truncate("hi", 10), "hi");
        assert_eq!(truncate("hello world", 5), "hello…");
    }
}

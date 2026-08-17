//! Chat compacting (spec §6).
//!
//! Folds a session's message history into a single summary message using a
//! one-shot LLM call, then persists the updated session and emits a
//! `chat-compacted` Tauri event so the frontend can surface the result.

use serde::Serialize;
use std::sync::Arc;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::agent_loop::{AgentEventSink, TurnCompleter};
use crate::llm_client::AgentTurnError;
use crate::session::{save_session, CompactMeta, CompactState, Message, Session};

/// Summary of a completed compaction. Returned so the caller (the IPC handler)
/// can surface a meaningful toast.
#[derive(Debug, Serialize)]
pub struct CompactResult {
    /// Number of messages that were folded into the summary.
    pub original_count: usize,
    /// Rough token estimate (chars/4) before compaction.
    pub token_estimate_before: usize,
    /// Rough token estimate (chars/4) after compaction.
    pub token_estimate_after: usize,
}

/// Why compaction could not be performed.
#[derive(Debug)]
pub enum CompactError {
    /// The foldable message count was 0 or 1 — nothing meaningful to compact.
    NothingToCompact,
    /// LLM returned a refusal or malformed response.
    BadSummary,
    /// LLM or transport error.
    Llm(AgentTurnError),
}

/// Rough token estimate: chars / 4 (spec §6.6).
fn token_estimate(messages: &[Message]) -> usize {
    messages.iter().map(|m| m.content.len()).sum::<usize>() / 4
}

const SUMMARY_SYSTEM_PROMPT: &str = "You are a precise technical summarizer. \
Summarize the following conversation into 1–2k tokens preserving: user intent, \
decisions made, files modified, tool outcomes, and open questions. \
Discard file contents and command stdout unless the content itself is \
what the user asked about.";

/// Build the user prompt text from a list of messages (spec §6.3 step 4).
fn build_summary_user_prompt(foldable: &[&Message]) -> String {
    let mut text = String::from("Summarize the following conversation:\n\n");
    for m in foldable {
        let role = if m.role == "user" {
            "User"
        } else {
            "Assistant"
        };
        text.push_str(&format!("[{}]\n{}\n\n", role, m.content));
    }
    text
}

/// Refusal patterns that indicate the LLM declined to summarize (spec §6.3 step 5.2).
fn is_refusal(content: &str) -> bool {
    let lower = content.to_lowercase();
    lower.starts_with("i can't")
        || lower.starts_with("i cannot")
        || lower.starts_with("i'm unable")
        || lower.starts_with("i am unable")
}

/// Whether the LLM response looks like a valid summary (spec §6.3 step 5.2).
fn is_valid_summary(content: &str) -> bool {
    let trimmed = content.trim();
    !trimmed.is_empty() && trimmed.len() <= 5 * 1024 && !is_refusal(trimmed)
}

/// The core compaction operation. Folds `session.messages` into a single
/// summary message and updates `session.compact` + `session.last_turn`.
///
/// Returns `Ok(CompactResult)` on success. On failure (LLM error, malformed
/// response, or nothing to compact), returns `Err` and leaves the session
/// **unchanged** (spec §6.3 step 5).
///
/// Does NOT persist — the caller is responsible for persisting after a
/// successful call.
pub async fn compact_chat(
    session: &mut Session,
    model_id: &str,
    completer: Arc<dyn TurnCompleter>,
    cancel_rx: oneshot::Receiver<()>,
) -> Result<CompactResult, CompactError> {
    // Step 2: collect foldable messages (exclude existing summaries).
    let foldable: Vec<&Message> = session
        .messages
        .iter()
        .filter(|m| m.kind.as_deref() != Some("summary"))
        .collect();

    // Step 3: need at least 2 messages to compact.
    if foldable.len() < 2 {
        return Err(CompactError::NothingToCompact);
    }

    let token_before = token_estimate(&session.messages);

    // Step 4: build prompts and call LLM.
    let user_prompt = build_summary_user_prompt(&foldable);
    let system_prompt = SUMMARY_SYSTEM_PROMPT.to_string();

    let summary = completer
        .complete_with_system_prompt(&system_prompt, &user_prompt, model_id, cancel_rx)
        .await
        .map_err(CompactError::Llm)?;

    // Step 5.2: validate the response.
    if !is_valid_summary(&summary) {
        return Err(CompactError::BadSummary);
    }

    // Step 6: generate IDs and metadata.
    let summary_id = format!("sum-{}", Uuid::new_v4());
    let folded_ids: Vec<String> = foldable.iter().map(|m| m.id.clone()).collect();
    let original_count = foldable.len();
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let token_after = (summary.len() / 4).max(1);

    // Build the summary message.
    let summary_msg = Message {
        id: summary_id.clone(),
        role: "system".to_string(),
        content: summary.clone(),
        timestamp: now.clone(),
        mentions: Vec::new(),
        agent_id: None,
        agent_name: None,
        reasoning: None,
        kind: Some("summary".to_string()),
        compact_meta: Some(CompactMeta {
            original_count,
            token_estimate: token_after,
            folded_message_ids: folded_ids.clone(),
            compacted_at: now.clone(),
        }),
    };

    // Replace foldable range with the single summary message.
    let new_messages: Vec<Message> = session
        .messages
        .iter()
        .filter(|m| m.kind.as_deref() == Some("summary"))
        .cloned()
        .chain(std::iter::once(summary_msg))
        .collect();

    session.messages = new_messages;

    // Step 8: update compact state.
    session.compact = Some(CompactState {
        compacted_at: now.clone(),
        summary_message_id: summary_id,
        original_count,
        token_estimate_before: token_before,
        token_estimate_after: token_after,
    });

    // Step 9: clear last_turn.
    session.last_turn = None;

    Ok(CompactResult {
        original_count,
        token_estimate_before: token_before,
        token_estimate_after: token_after,
    })
}

/// Persist the session and emit a `chat-compacted` event. Call this after a
/// successful `compact_chat`.
pub async fn persist_compact(
    session: &Session,
    result: &CompactResult,
    sink: &Arc<dyn AgentEventSink>,
) {
    if let Err(e) = save_session(&session.project_path, session.clone()).await {
        eprintln!("compact: failed to persist session {}: {}", session.id, e);
    }
    sink.emit(
        "chat-compacted",
        serde_json::json!({
            "sessionId": session.id,
            "originalCount": result.original_count,
            "tokenEstimateBefore": result.token_estimate_before,
            "tokenEstimateAfter": result.token_estimate_after,
        }),
    );
}

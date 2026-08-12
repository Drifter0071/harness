//! LLM streaming client.
//!
//! Two providers, one trait-shaped surface:
//!
//! - Anthropic Messages (`POST {base_url}/v1/messages`) - SSE event types:
//!   `message_start`, `content_block_delta`, `message_delta`, `message_stop`, `error`.
//! - OpenAI Chat Completions (`POST {base_url}/chat/completions`) - SSE
//!   data lines: `{"choices":[{"delta":{"content":"..."}}]}` and
//!   `{"choices":[], "usage":{...}}` and the terminator `[DONE]`.
//!
//! The chat-streaming machinery below is WIP: it is not wired into
//! `chat_complete` yet (see the placeholder in `commands.rs`). Only
//! `test_connection` is live today, so the rest is intentionally dead
//! code until the real implementation lands.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use dashmap::DashMap;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

use crate::models::{self, ReasoningEffort, SavedModelRecord};
use crate::secrets::SecretStore;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    pub request_id: String,
    pub model_id: String,
    pub messages: Vec<ChatMessage>,
    pub thinking_on: bool,
    pub reasoning_effort: Option<ReasoningEffort>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatChunk {
    pub request_id: String,
    pub delta: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatUsage {
    pub input: u64,
    pub output: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSessionWindow {
    pub used_fraction: f64,
    pub resets_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatDone {
    pub request_id: String,
    pub content: String,
    pub usage: ChatUsage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_window: Option<ChatSessionWindow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatError {
    pub request_id: String,
    pub kind: String,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
}

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))
}

fn strip_trailing_slash(s: &str) -> String {
    s.trim_end_matches('/').to_string()
}

struct ResolvedModel {
    record: SavedModelRecord,
    api_key: String,
    base_url: String,
}

async fn resolve(secrets: &dyn SecretStore, model_id: &str) -> Result<ResolvedModel, String> {
    let file = models::read_models_file().await?;
    let record = file
        .models
        .iter()
        .find(|m| m.id == model_id)
        .ok_or_else(|| format!("Model {} not found", model_id))?
        .clone();
    let api_key = secrets
        .get(&record.id)
        .map_err(|e| format!("Keyring error: {}", e))?
        .ok_or_else(|| format!("No API key saved for model {}", record.id))?;
    let base_url = strip_trailing_slash(&record.base_url);
    Ok(ResolvedModel {
        record,
        api_key,
        base_url,
    })
}

fn map_status_to_kind(status: u16) -> &'static str {
    match status {
        401 | 403 => "auth",
        429 => "rate_limit",
        400 => "bad_request",
        500..=599 => "provider",
        _ => "bad_request",
    }
}

fn parse_retry_after(headers: &HeaderMap) -> Option<u64> {
    let v = headers.get(RETRY_AFTER)?.to_str().ok()?;
    v.trim().parse::<u64>().ok()
}

fn body_excerpt(body: &str) -> String {
    if body.len() > 200 {
        format!("{}...", &body[..200])
    } else {
        body.to_string()
    }
}

pub async fn run_chat(
    request: ChatRequest,
    secrets: Arc<dyn SecretStore>,
    in_flight: Arc<DashMap<String, oneshot::Sender<()>>>,
    app: AppHandle,
) -> Result<(), String> {
    let request_id = request.request_id.clone();
    let model_id = request.model_id.clone();

    let resolved = match resolve(&*secrets, &model_id).await {
        Ok(r) => r,
        Err(e) => {
            let kind = if e.contains("No API key") {
                "auth"
            } else {
                "internal"
            };
            emit_error(&app, &request_id, kind, e, None);
            return Ok(());
        }
    };

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    in_flight.insert(request_id.clone(), cancel_tx);

    let outcome = match resolved.record.format.as_str() {
        "anthropic-messages" => run_anthropic(&request, &resolved, &app, cancel_rx).await,
        "openai-chat-completions" => run_openai(&request, &resolved, &app, cancel_rx).await,
        other => {
            emit_error(
                &app,
                &request_id,
                "bad_request",
                format!("Unsupported format: {}", other),
                None,
            );
            return Ok(());
        }
    };

    in_flight.remove(&request_id);

    if let Err(e) = outcome {
        emit_error(&app, &request_id, "internal", e, None);
    }

    Ok(())
}

fn emit_error(
    app: &AppHandle,
    request_id: &str,
    kind: &str,
    error: impl Into<String>,
    retry_after: Option<u64>,
) {
    let _ = app.emit(
        "chat-error",
        ChatError {
            request_id: request_id.to_string(),
            kind: kind.to_string(),
            error: error.into(),
            retry_after_seconds: retry_after,
        },
    );
}

fn emit_done(
    app: &AppHandle,
    request_id: &str,
    content: &str,
    usage: ChatUsage,
    session_window: Option<ChatSessionWindow>,
) {
    let _ = app.emit(
        "chat-done",
        ChatDone {
            request_id: request_id.to_string(),
            content: content.to_string(),
            usage,
            session_window,
        },
    );
}

async fn run_anthropic(
    request: &ChatRequest,
    resolved: &ResolvedModel,
    app: &AppHandle,
    mut cancel_rx: oneshot::Receiver<()>,
) -> Result<(), String> {
    let client = build_client()?;
    let url = format!("{}/v1/messages", resolved.base_url);

    let mut body = serde_json::json!({
        "model": resolved.record.model_name,
        "max_tokens": 4096,
        "messages": format_anthropic_messages(&request.messages),
        "stream": true,
    });
    if request.thinking_on {
        body["thinking"] = serde_json::json!({"type": "enabled", "budget_tokens": 4096});
    }

    let resp = client
        .post(&url)
        .header("x-api-key", &resolved.api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await;

    let resp = match resp {
        Ok(r) => r,
        Err(e) => return Err(emit_chat_error_from_reqwest(app, &request.request_id, e)),
    };

    let status = resp.status();
    if !status.is_success() {
        let status_code = status.as_u16();
        let headers = resp.headers().clone();
        let body_text = resp.text().await.unwrap_or_default();
        let kind = map_status_to_kind(status_code);
        let retry = if kind == "rate_limit" {
            parse_retry_after(&headers)
        } else {
            None
        };
        emit_error(
            app,
            &request.request_id,
            kind,
            format!("HTTP {}: {}", status_code, body_excerpt(&body_text)),
            retry,
        );
        return Ok(());
    }

    let headers = resp.headers().clone();
    let session_window_from_headers = extract_anthropic_session_window(&headers);

    let mut stream = resp.bytes_stream();
    let mut buffer = String::new();
    let mut full_content = String::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;

    loop {
        let chunk_fut = stream.next();
        tokio::select! {
            biased;
            _ = cancel_rx_async(&mut cancel_rx) => {
                emit_error(app, &request.request_id, "cancelled", "cancelled by user", None);
                return Ok(());
            }
            maybe = chunk_fut => {
                let Some(chunk_res) = maybe else { break };
                let chunk: Bytes = match chunk_res {
                    Ok(c) => c,
                    Err(e) => { emit_error(app, &request.request_id, "network", format!("stream error: {}", e), None); return Ok(()); }
                };
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some((event, _consumed)) = take_sse_event(&mut buffer) {
                    match event {
                        AnthropicSseEvent::MessageStart { usage } => { input_tokens = usage.input_tokens; }
                        AnthropicSseEvent::ContentBlockDelta { text: Some(text), .. } => {
                            if !text.is_empty() {
                                full_content.push_str(&text);
                                let _ = app.emit("chat-chunk", ChatChunk {
                                    request_id: request.request_id.clone(),
                                    delta: text,
                                });
                            }
                        }
                        AnthropicSseEvent::ContentBlockDelta { .. } => {}
                        AnthropicSseEvent::MessageDelta { usage, .. } => { output_tokens = usage.output_tokens; }
                        AnthropicSseEvent::MessageStop => {
                            emit_done(app, &request.request_id, &full_content, ChatUsage { input: input_tokens, output: output_tokens }, session_window_from_headers.clone());
                            return Ok(());
                        }
                        AnthropicSseEvent::Error { message } => { emit_error(app, &request.request_id, "provider", message, None); return Ok(()); }
                        AnthropicSseEvent::Ignore => {}
                        // Tool-calling blocks are consumed by complete_with_tools.
                        AnthropicSseEvent::ContentBlockStart { .. } | AnthropicSseEvent::ContentBlockStop { .. } => {}
                    }
                }
            }
        }
    }

    emit_done(
        app,
        &request.request_id,
        &full_content,
        ChatUsage {
            input: input_tokens,
            output: output_tokens,
        },
        session_window_from_headers,
    );
    Ok(())
}

async fn cancel_rx_async(rx: &mut oneshot::Receiver<()>) -> Option<()> {
    rx.await.ok()
}

#[derive(Debug, Clone)]
enum AnthropicSseEvent {
    MessageStart {
        usage: AnthropicUsage,
    },
    ContentBlockStart {
        index: usize,
        tool_use: Option<(String, String)>,
    },
    ContentBlockDelta {
        index: usize,
        text: Option<String>,
        partial_json: Option<String>,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageDelta {
        usage: AnthropicUsage,
        stop_reason: Option<String>,
    },
    MessageStop,
    Error {
        message: String,
    },
    Ignore,
}

#[derive(Debug, Clone, Default)]
struct AnthropicUsage {
    input_tokens: u64,
    output_tokens: u64,
}

fn take_sse_event(buffer: &mut String) -> Option<(AnthropicSseEvent, usize)> {
    let event_end = find_event_boundary(buffer)?;
    let raw: String = buffer.drain(..event_end).collect();
    // Drop any blank lines left between events.
    *buffer = buffer.trim_start_matches(['\r', '\n']).to_string();

    let mut event_type: Option<String> = None;
    let mut data_lines: Vec<String> = Vec::new();
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            event_type = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim().to_string());
        }
    }
    let data = data_lines.join("\n");
    let event_type = event_type.unwrap_or_default();

    let event = match event_type.as_str() {
        "message_start" => match serde_json::from_str::<Value>(&data) {
            Ok(v) => {
                let usage = v.pointer("/message/usage").cloned().unwrap_or(Value::Null);
                AnthropicSseEvent::MessageStart {
                    usage: parse_anthropic_usage(&usage),
                }
            }
            Err(_) => AnthropicSseEvent::Ignore,
        },
        "content_block_start" => match serde_json::from_str::<Value>(&data) {
            Ok(v) => {
                let index = v.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                let tool_use = v
                    .pointer("/content_block")
                    .filter(|cb| cb.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                    .and_then(|cb| {
                        let id = cb.get("id").and_then(|i| i.as_str())?;
                        let name = cb.get("name").and_then(|n| n.as_str())?;
                        Some((id.to_string(), name.to_string()))
                    });
                AnthropicSseEvent::ContentBlockStart { index, tool_use }
            }
            Err(_) => AnthropicSseEvent::Ignore,
        },
        "content_block_delta" => match serde_json::from_str::<Value>(&data) {
            Ok(v) => {
                let index = v.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                let text = v
                    .pointer("/delta/text")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string());
                let partial_json = v
                    .pointer("/delta/partial_json")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string());
                AnthropicSseEvent::ContentBlockDelta {
                    index,
                    text,
                    partial_json,
                }
            }
            Err(_) => AnthropicSseEvent::Ignore,
        },
        "content_block_stop" => match serde_json::from_str::<Value>(&data) {
            Ok(v) => {
                let index = v.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                AnthropicSseEvent::ContentBlockStop { index }
            }
            Err(_) => AnthropicSseEvent::Ignore,
        },
        "message_delta" => match serde_json::from_str::<Value>(&data) {
            Ok(v) => {
                let usage = v.pointer("/usage").cloned().unwrap_or(Value::Null);
                let stop_reason = v
                    .pointer("/delta/stop_reason")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
                AnthropicSseEvent::MessageDelta {
                    usage: parse_anthropic_usage(&usage),
                    stop_reason,
                }
            }
            Err(_) => AnthropicSseEvent::Ignore,
        },
        "message_stop" => AnthropicSseEvent::MessageStop,
        "error" => {
            let message = serde_json::from_str::<Value>(&data)
                .ok()
                .and_then(|v| {
                    v.get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| data.clone());
            AnthropicSseEvent::Error { message }
        }
        _ => AnthropicSseEvent::Ignore,
    };

    Some((event, event_end))
}

fn find_event_boundary(buffer: &str) -> Option<usize> {
    if let Some(i) = buffer.find("\n\n") {
        return Some(i + 2);
    }
    if let Some(i) = buffer.find("\r\n\r\n") {
        return Some(i + 4);
    }
    None
}

fn parse_anthropic_usage(v: &Value) -> AnthropicUsage {
    AnthropicUsage {
        input_tokens: v.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
        output_tokens: v.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
    }
}

fn extract_anthropic_session_window(headers: &HeaderMap) -> Option<ChatSessionWindow> {
    // Try the tokens pair first, fall back to the requests pair. A missing
    // or malformed header only skips that pair, it does not abort.
    for (remaining, reset) in [
        (
            "anthropic-ratelimit-tokens-remaining",
            "anthropic-ratelimit-tokens-reset",
        ),
        (
            "anthropic-ratelimit-requests-remaining",
            "anthropic-ratelimit-requests-reset",
        ),
    ] {
        let remaining: u64 = match headers
            .get(remaining)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok())
        {
            Some(v) => v,
            None => continue,
        };
        let reset = match headers.get(reset).and_then(|v| v.to_str().ok()) {
            Some(v) => v,
            None => continue,
        };
        let resets_at = match parse_anthropic_reset(reset) {
            Some(v) => v,
            None => continue,
        };
        let used_fraction = ((100u64.saturating_sub(remaining)) as f64 / 100.0).clamp(0.0, 1.0);
        return Some(ChatSessionWindow {
            used_fraction,
            resets_at,
        });
    }
    None
}

fn parse_anthropic_reset(s: &str) -> Option<String> {
    use chrono::{DateTime, Duration, Utc};
    let mut total_secs: i64 = 0;
    let mut cur_num = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            cur_num.push(c);
        } else {
            let n: i64 = cur_num.parse().ok()?;
            cur_num.clear();
            match c {
                's' => total_secs += n,
                'm' => total_secs += n * 60,
                'h' => total_secs += n * 3600,
                'd' => total_secs += n * 86400,
                _ => return None,
            }
        }
    }
    if !cur_num.is_empty() {
        let n: i64 = cur_num.parse().ok()?;
        total_secs += n;
    }
    if total_secs == 0 {
        return None;
    }
    let target: DateTime<Utc> = Utc::now() + Duration::seconds(total_secs);
    Some(target.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

async fn run_openai(
    request: &ChatRequest,
    resolved: &ResolvedModel,
    app: &AppHandle,
    mut cancel_rx: oneshot::Receiver<()>,
) -> Result<(), String> {
    let client = build_client()?;
    let url = format!("{}/chat/completions", resolved.base_url);

    let mut include_reasoning = request.reasoning_effort.is_some();

    loop {
        let mut body = serde_json::json!({
            "model": resolved.record.model_name,
            "messages": format_openai_messages(&request.messages),
            "stream": true,
            "stream_options": { "include_usage": true }
        });
        if include_reasoning {
            if let Some(effort) = request.reasoning_effort {
                body["reasoning_effort"] = serde_json::json!(effort.as_str());
            }
        }

        let resp = match client
            .post(&url)
            .header("authorization", format!("Bearer {}", resolved.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                emit_error(
                    app,
                    &request.request_id,
                    "network",
                    format!("request failed: {}", e),
                    None,
                );
                return Ok(());
            }
        };

        let status = resp.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            let headers = resp.headers().clone();
            let body_text = resp.text().await.unwrap_or_default();

            if status_code == 400
                && include_reasoning
                && body_text.to_lowercase().contains("reasoning_effort")
            {
                include_reasoning = false;
                continue;
            }

            let kind = map_status_to_kind(status_code);
            let retry = if kind == "rate_limit" {
                parse_retry_after(&headers)
            } else {
                None
            };
            emit_error(
                app,
                &request.request_id,
                kind,
                format!("HTTP {}: {}", status_code, body_excerpt(&body_text)),
                retry,
            );
            return Ok(());
        }

        let headers = resp.headers().clone();
        let session_window_from_headers = extract_openai_session_window(&headers);

        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut full_content = String::new();
        let mut input_tokens: u64 = 0;
        let mut output_tokens: u64 = 0;

        let mut done = false;
        let mut cancelled = false;
        let mut provider_err: Option<String> = None;
        let mut stream_err: Option<String> = None;
        loop {
            let chunk_fut = stream.next();
            tokio::select! {
                biased;
                _ = (&mut cancel_rx) => { cancelled = true; break; }
                maybe = chunk_fut => {
                    let Some(chunk_res) = maybe else { break };
                    let chunk: Bytes = match chunk_res {
                        Ok(c) => c,
                        Err(e) => { stream_err = Some(e.to_string()); break; }
                    };
                    buffer.push_str(&String::from_utf8_lossy(&chunk));
                    while let Some(line) = take_sse_data_line(&mut buffer) {
                        if line == "[DONE]" { done = true; break; }
                        let parsed: Value = match serde_json::from_str(&line) { Ok(v) => v, Err(_) => continue };
                        if let Some(err) = parsed.get("error") {
                            let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("provider error").to_string();
                            provider_err = Some(msg);
                            break;
                        }
                        if let Some(choice) = parsed.pointer("/choices/0") {
                            if let Some(text) = choice.pointer("/delta/content").and_then(|c| c.as_str()) {
                                if !text.is_empty() {
                                    full_content.push_str(text);
                                    let _ = app.emit("chat-chunk", ChatChunk {
                                        request_id: request.request_id.clone(),
                                        delta: text.to_string(),
                                    });
                                }
                            }
                        }
                        if let Some(usage) = parsed.get("usage") {
                            if let Some(n) = usage.get("prompt_tokens").and_then(|v| v.as_u64()) { input_tokens = n; }
                            if let Some(n) = usage.get("completion_tokens").and_then(|v| v.as_u64()) { output_tokens = n; }
                        }
                    }
                    if done || provider_err.is_some() { break; }
                }
            }
        }

        if cancelled {
            emit_error(
                app,
                &request.request_id,
                "cancelled",
                "cancelled by user",
                None,
            );
            return Ok(());
        }
        if let Some(e) = stream_err {
            emit_error(
                app,
                &request.request_id,
                "network",
                format!("stream error: {}", e),
                None,
            );
            return Ok(());
        }
        if let Some(msg) = provider_err {
            emit_error(app, &request.request_id, "provider", msg, None);
            return Ok(());
        }
        emit_done(
            app,
            &request.request_id,
            &full_content,
            ChatUsage {
                input: input_tokens,
                output: output_tokens,
            },
            session_window_from_headers,
        );
        return Ok(());
    }
}

fn take_sse_data_line(buffer: &mut String) -> Option<String> {
    let line_end = buffer.find('\n')?;
    let line: String = buffer.drain(..=line_end).collect();
    let line = line.trim_end_matches('\n').trim_end_matches('\r').trim();
    if let Some(rest) = line.strip_prefix("data:") {
        Some(rest.trim().to_string())
    } else {
        take_sse_data_line(buffer)
    }
}

fn extract_openai_session_window(headers: &HeaderMap) -> Option<ChatSessionWindow> {
    let pairs = [
        (
            "x-ratelimit-remaining-requests",
            "x-ratelimit-reset-requests",
        ),
        ("x-ratelimit-remaining-tokens", "x-ratelimit-reset-tokens"),
        ("x-quota-remaining", "x-quota-reset"),
    ];
    for (rem, reset) in pairs {
        if let (Some(rem_v), Some(reset_v)) = (
            headers.get(rem).and_then(|v| v.to_str().ok()),
            headers.get(reset).and_then(|v| v.to_str().ok()),
        ) {
            if let (Ok(remaining), Ok(reset_secs)) = (rem_v.parse::<u64>(), reset_v.parse::<i64>())
            {
                let used_fraction =
                    ((100u64.saturating_sub(remaining)) as f64 / 100.0).clamp(0.0, 1.0);
                let target = chrono::Utc::now() + chrono::Duration::seconds(reset_secs);
                let resets_at = target.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                return Some(ChatSessionWindow {
                    used_fraction,
                    resets_at,
                });
            }
        }
    }
    None
}

fn format_anthropic_messages(msgs: &[ChatMessage]) -> Vec<Value> {
    msgs.iter()
        .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
        .collect()
}

fn format_openai_messages(msgs: &[ChatMessage]) -> Vec<Value> {
    msgs.iter()
        .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
        .collect()
}

fn emit_chat_error_from_reqwest(app: &AppHandle, request_id: &str, e: reqwest::Error) -> String {
    let kind = if e.is_timeout() || e.is_connect() || e.is_request() {
        "network"
    } else {
        "internal"
    };
    emit_error(
        app,
        request_id,
        kind,
        format!("request failed: {}", e),
        None,
    );
    format!("emit chat-error (network): {}", e)
}

pub async fn test_connection(
    record: &SavedModelRecord,
    api_key: &str,
) -> Result<models::TestResult, String> {
    let client = build_client()?;
    let base = strip_trailing_slash(&record.base_url);

    match record.format.as_str() {
        "anthropic-messages" => {
            let url = format!("{}/v1/messages", base);
            let body = serde_json::json!({"model": record.model_name, "max_tokens": 1, "messages": [{"role": "user", "content": "hi"}]});
            let resp = client
                .post(&url)
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await;
            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    return Ok(models::TestResult {
                        ok: false,
                        kind: "network".to_string(),
                        message: format!("Could not reach Anthropic: {}", e),
                    })
                }
            };
            let status = resp.status();
            if status.is_success() {
                Ok(models::TestResult {
                    ok: true,
                    kind: "ok".to_string(),
                    message: "OK".to_string(),
                })
            } else {
                let body = resp.text().await.unwrap_or_default();
                Ok(models::TestResult {
                    ok: false,
                    kind: map_status_to_kind(status.as_u16()).to_string(),
                    message: body_excerpt(&body),
                })
            }
        }
        "openai-chat-completions" => {
            let models_url = format!("{}/models", base);
            let probe = client
                .get(&models_url)
                .header("authorization", format!("Bearer {}", api_key))
                .send()
                .await;
            if let Ok(r) = probe {
                let s = r.status();
                if s.is_success() || s.as_u16() == 404 {
                    return Ok(models::TestResult {
                        ok: true,
                        kind: "ok".to_string(),
                        message: "OK".to_string(),
                    });
                }
                if s.as_u16() == 401 || s.as_u16() == 403 {
                    return Ok(models::TestResult {
                        ok: false,
                        kind: "auth".to_string(),
                        message: "Invalid API key".to_string(),
                    });
                }
            }
            let url = format!("{}/chat/completions", base);
            let body = serde_json::json!({"model": record.model_name, "max_tokens": 1, "messages": [{"role": "user", "content": "hi"}]});
            let resp = client
                .post(&url)
                .header("authorization", format!("Bearer {}", api_key))
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await;
            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    return Ok(models::TestResult {
                        ok: false,
                        kind: "network".to_string(),
                        message: format!("Could not reach provider: {}", e),
                    })
                }
            };
            let status = resp.status();
            if status.is_success() {
                Ok(models::TestResult {
                    ok: true,
                    kind: "ok".to_string(),
                    message: "OK".to_string(),
                })
            } else {
                let body = resp.text().await.unwrap_or_default();
                Ok(models::TestResult {
                    ok: false,
                    kind: map_status_to_kind(status.as_u16()).to_string(),
                    message: body_excerpt(&body),
                })
            }
        }
        other => Ok(models::TestResult {
            ok: false,
            kind: "bad_request".to_string(),
            message: format!("Unsupported format: {}", other),
        }),
    }
}

// -- Tool calling (agent loop, OpenAI Chat Completions) --
//
// `complete_with_tools` drives a single provider turn with native tool
// calling. It is the LLM seam the agent loop (WP5) calls; text deltas are
// forwarded through `text_cb`, and the outcome carries the aggregated tool
// calls for the loop to execute. Anthropic tool use is not implemented yet
// and fails with an explicit error instead of misbehaving.

/// JSON schema for one tool the model may call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub parameters: Value,
}

/// One tool call the model requested.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// Result of one provider turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnOutcome {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: ChatUsage,
}

/// Provider-agnostic message for tool-capable turns. `role` is one of
/// `system` / `user` / `assistant` / `tool`; `tool_call_id` pairs a `tool`
/// result with the call it answers; `tool_calls` echoes assistant calls
/// back (OpenAI requires them in the history for multi-turn tool use).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolChatMessage {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone)]
pub struct ToolChatRequest {
    pub model_id: String,
    pub system_prompt: String,
    pub messages: Vec<ToolChatMessage>,
}

/// Agent-loop error vocabulary; the loop maps these to `agent-error` events.
#[derive(Debug, Clone)]
pub enum AgentTurnError {
    Auth(String),
    RateLimited(Option<u64>),
    Network(String),
    Provider(String),
    BadRequest(String),
    Cancelled,
    Internal(String),
}

/// Run one tool-capable turn. `text_cb` receives content deltas as they
/// stream; cancellation races the stream and returns `Cancelled`.
pub async fn complete_with_tools(
    request: &ToolChatRequest,
    tools: &[ToolSpec],
    secrets: &dyn SecretStore,
    text_cb: impl Fn(&str),
    cancel_rx: oneshot::Receiver<()>,
) -> Result<TurnOutcome, AgentTurnError> {
    let resolved = resolve(secrets, &request.model_id).await.map_err(|e| {
        if e.contains("No API key") {
            AgentTurnError::Auth(e)
        } else {
            AgentTurnError::Internal(e)
        }
    })?;
    match resolved.record.format.as_str() {
        "openai-chat-completions" => {
            run_openai_tools(request, tools, &resolved, text_cb, cancel_rx).await
        }
        other => Err(AgentTurnError::BadRequest(format!(
            "Tool calling not implemented for format: {}",
            other
        ))),
    }
}

async fn run_openai_tools(
    request: &ToolChatRequest,
    tools: &[ToolSpec],
    resolved: &ResolvedModel,
    text_cb: impl Fn(&str),
    mut cancel_rx: oneshot::Receiver<()>,
) -> Result<TurnOutcome, AgentTurnError> {
    let client = build_client().map_err(AgentTurnError::Internal)?;
    let url = format!("{}/chat/completions", resolved.base_url);

    let body = serde_json::json!({
        "model": resolved.record.model_name,
        "messages": build_openai_tool_messages(request),
        "tools": tools.iter().map(tool_spec_to_openai).collect::<Vec<_>>(),
        "stream": true,
        "stream_options": { "include_usage": true }
    });

    let resp = client
        .post(&url)
        .header("authorization", format!("Bearer {}", resolved.api_key))
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| agent_network_error(&e))?;

    let status = resp.status();
    if !status.is_success() {
        let status_code = status.as_u16();
        let headers = resp.headers().clone();
        let body_text = resp.text().await.unwrap_or_default();
        return Err(status_to_agent_error(status_code, &body_text, &headers));
    }

    let mut stream = resp.bytes_stream();
    let mut buffer = String::new();
    let mut full_text = String::new();
    let mut acc = OpenAiToolCallAccumulator::default();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut tool_calls_finish = false;

    'stream: loop {
        let chunk_fut = stream.next();
        tokio::select! {
            biased;
            _ = (&mut cancel_rx) => return Err(AgentTurnError::Cancelled),
            maybe = chunk_fut => {
                let Some(chunk_res) = maybe else { break };
                let chunk: Bytes = chunk_res
                    .map_err(|e| AgentTurnError::Network(format!("stream error: {}", e)))?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(line) = take_sse_data_line(&mut buffer) {
                    if line == "[DONE]" {
                        break 'stream;
                    }
                    let parsed: Value = match serde_json::from_str(&line) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if let Some(err) = parsed.get("error") {
                        let msg = err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("provider error")
                            .to_string();
                        return Err(AgentTurnError::Provider(msg));
                    }
                    if let Some(choice) = parsed.pointer("/choices/0") {
                        if let Some(text) = choice
                            .pointer("/delta/content")
                            .and_then(|c| c.as_str())
                        {
                            if !text.is_empty() {
                                full_text.push_str(text);
                                text_cb(text);
                            }
                        }
                        if let Some(delta) = choice.get("delta") {
                            acc.apply_delta(delta);
                        }
                        if let Some(reason) = choice
                            .pointer("/finish_reason")
                            .and_then(|r| r.as_str())
                        {
                            if reason == "tool_calls" {
                                tool_calls_finish = true;
                            }
                        }
                    }
                    if let Some(usage) = parsed.get("usage") {
                        if let Some(n) = usage.get("prompt_tokens").and_then(|v| v.as_u64()) {
                            input_tokens = n;
                        }
                        if let Some(n) = usage.get("completion_tokens").and_then(|v| v.as_u64()) {
                            output_tokens = n;
                        }
                    }
                }
            }
        }
    }

    let usage = ChatUsage {
        input: input_tokens,
        output: output_tokens,
    };
    let tool_calls = acc.finish();
    if tool_calls_finish && !tool_calls.is_empty() {
        return Ok(TurnOutcome {
            text: full_text,
            tool_calls,
            usage,
        });
    }
    Ok(TurnOutcome {
        text: full_text,
        tool_calls: Vec::new(),
        usage,
    })
}

/// Aggregates OpenAI `delta.tool_calls` fragments (index-keyed) into
/// complete `ToolCall`s. The provider streams `id` and `function.name`
/// once per call and `function.arguments` as split JSON fragments across
/// chunks, so fragments are concatenated per index and parsed at the end.
#[derive(Debug, Default, PartialEq)]
pub struct OpenAiToolCallAccumulator {
    /// Per tool-call index: (id, name, concatenated arguments fragments).
    by_index: Vec<(String, String, String)>,
}

impl OpenAiToolCallAccumulator {
    /// Feed one `choices[0].delta` object from a streamed chunk.
    pub fn apply_delta(&mut self, delta: &Value) {
        let Some(arr) = delta.get("tool_calls").and_then(|v| v.as_array()) else {
            return;
        };
        for item in arr {
            let Some(index) = item.get("index").and_then(|v| v.as_u64()) else {
                continue;
            };
            let index = index as usize;
            while self.by_index.len() <= index {
                self.by_index
                    .push((String::new(), String::new(), String::new()));
            }
            let entry = &mut self.by_index[index];
            if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                entry.0 = id.to_string();
            }
            if let Some(name) = item.pointer("/function/name").and_then(|v| v.as_str()) {
                entry.1 = name.to_string();
            }
            if let Some(args) = item.pointer("/function/arguments").and_then(|v| v.as_str()) {
                entry.2.push_str(args);
            }
        }
    }

    pub fn finish(&self) -> Vec<ToolCall> {
        self.by_index
            .iter()
            .filter(|(id, name, _)| !id.is_empty() && !name.is_empty())
            .map(|(id, name, args)| ToolCall {
                id: id.clone(),
                name: name.clone(),
                arguments: parse_arguments(args),
            })
            .collect()
    }
}

/// Parse a concatenated arguments string; fall back to a string value when
/// the provider closed the JSON mid-flight.
fn parse_arguments(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

fn tool_spec_to_openai(spec: &ToolSpec) -> Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": spec.name,
            "description": spec.description,
            "parameters": spec.parameters,
        }
    })
}

/// Build the OpenAI `messages` array. The system prompt becomes a `system`
/// message; `tool` results carry `tool_call_id`; assistant messages echo
/// their tool calls so the provider can pair results.
fn build_openai_tool_messages(request: &ToolChatRequest) -> Vec<Value> {
    let mut out = Vec::new();
    if !request.system_prompt.is_empty() {
        out.push(serde_json::json!({
            "role": "system",
            "content": request.system_prompt,
        }));
    }
    for m in &request.messages {
        match m.role.as_str() {
            "tool" => {
                out.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
                    "content": m.content,
                }));
            }
            "assistant" if !m.tool_calls.is_empty() => {
                let calls: Vec<Value> = m
                    .tool_calls
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "id": c.id,
                            "type": "function",
                            "function": {
                                "name": c.name,
                                "arguments": serde_json::to_string(&c.arguments)
                                    .unwrap_or_else(|_| "{}".into()),
                            }
                        })
                    })
                    .collect();
                out.push(serde_json::json!({
                    "role": "assistant",
                    "content": if m.content.is_empty() {
                        Value::Null
                    } else {
                        Value::String(m.content.clone())
                    },
                    "tool_calls": calls,
                }));
            }
            _ => {
                out.push(serde_json::json!({"role": m.role, "content": m.content}));
            }
        }
    }
    out
}

fn status_to_agent_error(status: u16, body: &str, headers: &HeaderMap) -> AgentTurnError {
    let msg = format!("HTTP {}: {}", status, body_excerpt(body));
    match map_status_to_kind(status) {
        "auth" => AgentTurnError::Auth(msg),
        "rate_limit" => AgentTurnError::RateLimited(parse_retry_after(headers)),
        "provider" => AgentTurnError::Provider(msg),
        _ => AgentTurnError::BadRequest(msg),
    }
}

fn agent_network_error(e: &reqwest::Error) -> AgentTurnError {
    if e.is_timeout() || e.is_connect() || e.is_request() {
        AgentTurnError::Network(format!("request failed: {}", e))
    } else {
        AgentTurnError::Internal(format!("request failed: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request() -> ToolChatRequest {
        ToolChatRequest {
            model_id: "m1".into(),
            system_prompt: "You are the harness agent.".into(),
            messages: vec![ToolChatMessage {
                role: "user".into(),
                content: "fix the bug".into(),
                tool_call_id: None,
                tool_calls: Vec::new(),
            }],
        }
    }

    #[test]
    fn openai_messages_map_system_tool_and_assistant_calls() {
        let mut req = sample_request();
        req.messages = vec![
            ToolChatMessage {
                role: "assistant".into(),
                content: "".into(),
                tool_call_id: None,
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "src/main.rs"}),
                }],
            },
            ToolChatMessage {
                role: "tool".into(),
                content: "file contents".into(),
                tool_call_id: Some("call_1".into()),
                tool_calls: Vec::new(),
            },
        ];

        let msgs = build_openai_tool_messages(&req);
        assert_eq!(msgs.len(), 3);
        // System prompt first.
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "You are the harness agent.");
        // Assistant echoes its tool call with stringified arguments.
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["tool_calls"][0]["id"], "call_1");
        assert_eq!(msgs[1]["tool_calls"][0]["function"]["name"], "read_file");
        assert_eq!(
            msgs[1]["tool_calls"][0]["function"]["arguments"],
            "{\"path\":\"src/main.rs\"}"
        );
        assert!(msgs[1]["content"].is_null());
        // Tool result carries the pairing id.
        assert_eq!(msgs[2]["role"], "tool");
        assert_eq!(msgs[2]["tool_call_id"], "call_1");
        assert_eq!(msgs[2]["content"], "file contents");
    }

    #[test]
    fn openai_accumulator_aggregates_split_arguments() {
        let mut acc = OpenAiToolCallAccumulator::default();

        // Chunk 1: header + first half of the arguments JSON.
        acc.apply_delta(&serde_json::json!({
            "tool_calls": [{
                "index": 0,
                "id": "call_9",
                "function": {"name": "edit_file", "arguments": "{\"path\":\""}
            }]
        }));
        // Chunk 2: rest of the arguments.
        acc.apply_delta(&serde_json::json!({
            "tool_calls": [{"index": 0, "function": {"arguments": "a.rs\",\"old\":\"x\"}"}}]
        }));

        let calls = acc.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_9");
        assert_eq!(calls[0].name, "edit_file");
        assert_eq!(
            calls[0].arguments,
            serde_json::json!({"path": "a.rs", "old": "x"})
        );
    }

    #[test]
    fn openai_accumulator_orders_multiple_indexes() {
        let mut acc = OpenAiToolCallAccumulator::default();
        acc.apply_delta(&serde_json::json!({
            "tool_calls": [
                {"index": 1, "id": "call_b", "function": {"name": "b", "arguments": "{}"}},
                {"index": 0, "id": "call_a", "function": {"name": "a", "arguments": "{\"n\":1}"}}
            ]
        }));
        let calls = acc.finish();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_a");
        assert_eq!(calls[0].name, "a");
        assert_eq!(calls[1].id, "call_b");
        assert_eq!(calls[1].name, "b");
    }

    #[test]
    fn openai_accumulator_skips_unfinished_calls() {
        let mut acc = OpenAiToolCallAccumulator::default();
        // Only arguments arrived, never an id/name header.
        acc.apply_delta(&serde_json::json!({
            "tool_calls": [{"index": 0, "function": {"arguments": "{}"}}]
        }));
        assert!(acc.finish().is_empty());
    }

    #[test]
    fn parse_arguments_falls_back_to_string_on_bad_json() {
        assert_eq!(parse_arguments("{oops"), Value::String("{oops".to_string()));
        assert_eq!(parse_arguments("{\"a\":1}"), serde_json::json!({"a": 1}));
    }

    #[test]
    fn tool_spec_serializes_function_schema() {
        let spec = ToolSpec {
            name: "read_file".into(),
            description: "Read a file".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}}
            }),
        };
        let v = tool_spec_to_openai(&spec);
        assert_eq!(v["type"], "function");
        assert_eq!(v["function"]["name"], "read_file");
        assert_eq!(
            v["function"]["parameters"]["properties"]["path"]["type"],
            "string"
        );
    }
}

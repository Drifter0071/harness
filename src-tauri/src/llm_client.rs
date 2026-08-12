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
                        AnthropicSseEvent::ContentBlockDelta { text } => {
                            if !text.is_empty() {
                                full_content.push_str(&text);
                                let _ = app.emit("chat-chunk", ChatChunk {
                                    request_id: request.request_id.clone(),
                                    delta: text,
                                });
                            }
                        }
                        AnthropicSseEvent::MessageDelta { usage } => { output_tokens = usage.output_tokens; }
                        AnthropicSseEvent::MessageStop => {
                            emit_done(app, &request.request_id, &full_content, ChatUsage { input: input_tokens, output: output_tokens }, session_window_from_headers.clone());
                            return Ok(());
                        }
                        AnthropicSseEvent::Error { message } => { emit_error(app, &request.request_id, "provider", message, None); return Ok(()); }
                        AnthropicSseEvent::Ignore => {}
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
    MessageStart { usage: AnthropicUsage },
    ContentBlockDelta { text: String },
    MessageDelta { usage: AnthropicUsage },
    MessageStop,
    Error { message: String },
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
        "content_block_delta" => match serde_json::from_str::<Value>(&data) {
            Ok(v) => {
                let text = v
                    .pointer("/delta/text")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                AnthropicSseEvent::ContentBlockDelta { text }
            }
            Err(_) => AnthropicSseEvent::Ignore,
        },
        "message_delta" => match serde_json::from_str::<Value>(&data) {
            Ok(v) => {
                let usage = v.pointer("/usage").cloned().unwrap_or(Value::Null);
                AnthropicSseEvent::MessageDelta {
                    usage: parse_anthropic_usage(&usage),
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

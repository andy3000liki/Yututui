//! OpenAI-compatible chat backend for DJ Gem: OpenAI, xAI, OpenCode (via a local
//! OpenAI-compatible proxy), and anything else speaking `POST {base}/chat/completions`
//! (Ollama, LM Studio, OpenRouter, ...).
//!
//! The actor loop, tools, and structured-output parsers all speak the Gemini protocol
//! internally ([`GenerateContentRequest`]/[`GenerateContentResponse`]). This module
//! translates at the boundary so the rest of the assistant never knows which service
//! answered:
//!
//! - Request: `systemInstruction` → `system` message; `user`/`model` turns →
//!   `user`/`assistant` messages; `functionCall` parts → `tool_calls` (ids are
//!   deterministic per request, `call_{msg}_{idx}`); `functionResponse` parts → `tool`
//!   messages whose `tool_call_id` is recovered **positionally** from the most recent
//!   assistant turn's `tool_calls` in the same request (the actor always appends the
//!   tool-result turn immediately after the assistant turn, so this lines up).
//! - Response: assistant text → `text` part; `tool_calls` → `functionCall` parts;
//!   `usage` counters → [`UsageMetadata`]; `finish_reason` mapped (`stop`→`STOP`,
//!   `length`→`MAX_TOKENS`, `tool_calls`→`STOP`, `content_filter`→`SAFETY`).
//! - `generationConfig`: `temperature`/`top_p`/`max_output_tokens` map directly;
//!   `response_mime_type: "application/json"` becomes `response_format: json_object`
//!   (the structured-output parsers read text, so the JSON-schema constraint is
//!   intentionally not forwarded); `thinking_config` has no OpenAI equivalent.
//!
//! Errors reuse [`GeminiError`] so the actor's retry/fallback handling applies
//! unchanged (model fallback is skipped for this backend — there is only one model).

use std::time::Duration;

use reqwest::header::{HeaderValue, RETRY_AFTER};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::sleep;

use super::client::{
    Candidate, Content, FunctionCall, GenerateContentRequest, GenerateContentResponse,
    GeminiClient, GeminiError, Part, UsageMetadata,
};
use super::model::GeminiModel;
use crate::util::{http, sanitize};

/// Per-request transport retries (does not include the first attempt).
const MAX_RETRIES: u32 = 3;
/// Upper bound on a server-sent `Retry-After` sleep (mirrors the Gemini client: a huge
/// value must not wedge the single-threaded AI actor).
const RETRY_AFTER_CAP_SECS: u64 = 60;
/// Cap on error-body text kept in messages/logs.
const ERR_BODY_CAP: usize = 200;
const RESPONSE_BODY_MAX: usize = 4 * 1024 * 1024;
const ERROR_BODY_MAX: usize = 64 * 1024;

/// Which assistant backend DJ Gem uses. Persisted as snake_case in `config.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AiProviderKind {
    /// Google Gemini (`generativelanguage.googleapis.com`). The default.
    #[default]
    Gemini,
    /// Any OpenAI-compatible `/chat/completions` endpoint (see [`OpenAiPreset`]).
    OpenAi,
}

/// Preset endpoints for the OpenAI-compatible backend. A preset is only a default
/// base URL + model + display label — `openai_base_url`/`openai_model` in the config
/// override either, and `Custom` covers every other server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiPreset {
    /// `https://api.openai.com/v1` — OpenAI platform key.
    #[default]
    OpenAi,
    /// `https://api.x.ai/v1` — xAI key.
    Xai,
    /// `http://127.0.0.1:4096/v1` — `opencode serve` itself exposes no
    /// OpenAI-compatible route, so this points at the community proxy default
    /// (e.g. `opencode-openai-proxy` on :4096). Override `openai_base_url` if your
    /// proxy listens elsewhere, and always set `openai_model` to match.
    OpenCode,
    /// Any other server (Ollama default shown; LM Studio, OpenRouter, ... work the
    /// same way). Set `openai_base_url` (and usually `openai_model`).
    Custom,
}

impl OpenAiPreset {
    pub fn default_base_url(self) -> &'static str {
        match self {
            OpenAiPreset::OpenAi => "https://api.openai.com/v1",
            OpenAiPreset::Xai => "https://api.x.ai/v1",
            OpenAiPreset::OpenCode => "http://127.0.0.1:4096/v1",
            OpenAiPreset::Custom => "http://127.0.0.1:11434/v1",
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            OpenAiPreset::OpenAi => "gpt-4o-mini",
            OpenAiPreset::Xai => "grok-4",
            // Proxies/servers with a single or pass-through model: override
            // `openai_model` when the server needs a specific name.
            OpenAiPreset::OpenCode | OpenAiPreset::Custom => "default",
        }
    }

    /// Display label used in user-facing error text.
    pub fn label(self) -> &'static str {
        match self {
            OpenAiPreset::OpenAi => "OpenAI",
            OpenAiPreset::Xai => "xAI",
            OpenAiPreset::OpenCode => "OpenCode",
            OpenAiPreset::Custom => "Custom",
        }
    }
}

/// Resolved (runtime) OpenAI-compatible backend. Built by
/// `Config::effective_ai_provider`; never serialized.
#[derive(Debug, Clone)]
pub enum AiProvider {
    Gemini,
    OpenAi(OpenAiConfig),
}

/// Resolved OpenAI-compatible endpoint + model. Never serialized.
#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    pub base_url: String,
    pub model: String,
    pub service_label: &'static str,
}

/// The DJ Gem backend: either Google Gemini or an OpenAI-compatible endpoint.
/// Both arms speak the internal Gemini protocol (`generate` takes and returns the
/// same request/response types), so the actor loop is identical for every backend.
pub enum AiClient {
    Gemini(GeminiClient),
    OpenAi(OpenAiClient),
}

impl AiClient {
    pub async fn generate(
        &self,
        model: GeminiModel,
        req: &GenerateContentRequest,
    ) -> Result<GenerateContentResponse, GeminiError> {
        match self {
            AiClient::Gemini(client) => client.generate(model, req).await,
            // The OpenAI-compatible backend carries its own model string; the
            // Gemini model selector is inert for it.
            AiClient::OpenAi(client) => client.generate(req).await,
        }
    }

    pub fn service_label(&self) -> &'static str {
        match self {
            AiClient::Gemini(_) => "Gemini",
            AiClient::OpenAi(client) => client.service_label(),
        }
    }
}

// ---------------------------------------------------------------------------
// Chat wire types (OpenAI `POST {base}/chat/completions`)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ChatTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    function: FunctionSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FunctionSpec {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize)]
struct ChatTool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: ToolFunction,
}

#[derive(Debug, Serialize)]
struct ToolFunction {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<Value>,
}

#[derive(Debug, Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    #[serde(default)]
    message: Option<AssistantMessage>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AssistantMessage {
    #[serde(default)]
    content: Option<Value>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
    #[serde(default)]
    total_tokens: u32,
}

// ---------------------------------------------------------------------------
// Request translation (Gemini protocol → OpenAI chat)
// ---------------------------------------------------------------------------

/// Translate one DJ Gem request into an OpenAI chat-completions request.
fn to_chat_request(model: &str, req: &GenerateContentRequest) -> ChatRequest {
    let mut messages = Vec::new();
    if let Some(system) = &req.system_instruction {
        let text = system.joined_text();
        if !text.is_empty() {
            messages.push(ChatMessage {
                role: "system",
                content: Some(text),
                tool_calls: None,
                tool_call_id: None,
            });
        }
    }
    // Tool-call ids of the most recent assistant turn, consumed positionally by the
    // tool-result turn that follows it.
    let mut pending_ids: Vec<String> = Vec::new();
    for (msg_idx, content) in req.contents.iter().enumerate() {
        let is_model = content.role.as_deref() == Some("model");
        let mut text = String::new();
        let mut calls = Vec::new();
        let mut responses = Vec::new();
        for (part_idx, part) in content.parts.iter().enumerate() {
            if let Some(t) = &part.text {
                text.push_str(t);
            }
            if let Some(call) = &part.function_call {
                let id = format!("call_{msg_idx}_{part_idx}");
                let args = if call.args.is_null() {
                    "{}".to_owned()
                } else {
                    serde_json::to_string(&call.args).unwrap_or_else(|_| "{}".to_owned())
                };
                calls.push(ToolCall {
                    id,
                    kind: "function".to_owned(),
                    function: FunctionSpec {
                        name: call.name.clone(),
                        arguments: args,
                    },
                });
            }
            if let Some(resp) = &part.function_response {
                responses.push(resp);
            }
        }
        if !responses.is_empty() {
            // Tool-result turn: one `tool` message per result. Ids come from the
            // preceding assistant turn; if none are known (shouldn't happen through
            // the actor loop), degrade to a plain user message so the request
            // still validates.
            let mut ids = std::mem::take(&mut pending_ids);
            for resp in responses {
                let body = serde_json::to_string(&resp.response).unwrap_or_default();
                if ids.is_empty() {
                    messages.push(ChatMessage {
                        role: "user",
                        content: Some(body),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                } else {
                    messages.push(ChatMessage {
                        role: "tool",
                        content: Some(body),
                        tool_calls: None,
                        tool_call_id: Some(ids.remove(0)),
                    });
                }
            }
            if !text.is_empty() {
                messages.push(ChatMessage {
                    role: if is_model { "assistant" } else { "user" },
                    content: Some(text),
                    tool_calls: None,
                    tool_call_id: None,
                });
            }
            continue;
        }
        // Remember this turn's tool-call ids for the tool-result turn that follows.
        // Any other turn shape clears stale ids.
        pending_ids = if is_model {
            calls.iter().map(|c| c.id.clone()).collect()
        } else {
            Vec::new()
        };
        messages.push(ChatMessage {
            role: if is_model { "assistant" } else { "user" },
            content: if text.is_empty() { None } else { Some(text) },
            tool_calls: if calls.is_empty() { None } else { Some(calls) },
            tool_call_id: None,
        });
    }

    let tools = req.tools.as_ref().map(|tools| {
        tools
            .iter()
            .flat_map(|t| t.function_declarations.iter())
            .filter_map(|decl| {
                let name = decl.get("name")?.as_str()?.to_owned();
                Some(ChatTool {
                    kind: "function",
                    function: ToolFunction {
                        name,
                        description: decl
                            .get("description")
                            .and_then(|d| d.as_str())
                            .map(str::to_owned),
                        parameters: decl.get("parameters").cloned(),
                    },
                })
            })
            .collect::<Vec<_>>()
    });
    let tools = tools.filter(|t: &Vec<ChatTool>| !t.is_empty());

    let cfg = req.generation_config.as_ref();
    ChatRequest {
        model: model.to_owned(),
        messages,
        tools,
        temperature: cfg.and_then(|c| c.temperature),
        max_tokens: cfg.and_then(|c| c.max_output_tokens),
        top_p: cfg.and_then(|c| c.top_p),
        response_format: cfg
            .and_then(|c| c.response_mime_type.as_deref())
            .filter(|m| *m == "application/json")
            .map(|_| ResponseFormat {
                kind: "json_object",
            }),
    }
}

// ---------------------------------------------------------------------------
// Response translation (OpenAI chat → Gemini protocol)
// ---------------------------------------------------------------------------

/// Translate an OpenAI chat-completions response back into the internal protocol.
fn from_chat_response(resp: ChatResponse) -> Result<GenerateContentResponse, GeminiError> {
    let choice = resp.choices.into_iter().next().ok_or_else(|| {
        GeminiError::Decode("empty chat-completions response (no choices)".to_owned())
    })?;
    let message = choice.message.ok_or_else(|| {
        GeminiError::Decode("chat-completions choice has no message".to_owned())
    })?;
    let mut parts = Vec::new();
    let text = message_text(&message.content);
    if !text.is_empty() {
        parts.push(Part::text(text));
    }
    for call in message.tool_calls.unwrap_or_default() {
        let args = serde_json::from_str(&call.function.arguments)
            .unwrap_or(Value::String(call.function.arguments));
        parts.push(Part {
            text: None,
            function_call: Some(FunctionCall {
                name: call.function.name,
                args,
            }),
            function_response: None,
            extra: Default::default(),
        });
    }
    let content = Content {
        role: Some("model".to_owned()),
        parts,
    };
    let finish_reason = choice.finish_reason.as_deref().map(map_finish_reason);
    let usage = resp.usage.unwrap_or_default();
    Ok(GenerateContentResponse {
        candidates: vec![Candidate {
            content: Some(content),
            finish_reason,
        }],
        prompt_feedback: None,
        usage_metadata: Some(UsageMetadata {
            prompt_token_count: usage.prompt_tokens,
            candidates_token_count: usage.completion_tokens,
            total_token_count: usage.total_tokens,
            thoughts_token_count: 0,
            cached_content_token_count: 0,
        }),
    })
}

/// Assistant `content` may be a string, an array of content blocks, or null.
fn message_text(content: &Option<Value>) -> String {
    match content {
        None => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn map_finish_reason(reason: &str) -> String {
    match reason {
        "stop" => "STOP".to_owned(),
        "length" => "MAX_TOKENS".to_owned(),
        "tool_calls" => "STOP".to_owned(),
        "content_filter" => "SAFETY".to_owned(),
        other => other.to_ascii_uppercase(),
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// An OpenAI-compatible chat client. Deliberately does NOT derive `Debug` — the key
/// must never be printed.
pub struct OpenAiClient {
    http: reqwest::Client,
    url: String,
    key: String,
    model: String,
    service_label: &'static str,
}

impl OpenAiClient {
    /// Build a client for `{base_url}/chat/completions`. The key is validated as
    /// HTTP-header-safe here (as a `Bearer` value); an empty model is rejected.
    pub fn new(
        base_url: &str,
        api_key: &str,
        model: String,
        service_label: &'static str,
    ) -> Result<Self, GeminiError> {
        if model.trim().is_empty() {
            return Err(GeminiError::ModelNotFound);
        }
        let bearer = format!("Bearer {api_key}");
        HeaderValue::from_str(&bearer).map_err(|_| GeminiError::Auth)?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| GeminiError::Network(e.to_string()))?;
        Ok(Self {
            http,
            url: format!("{}/chat/completions", base_url.trim_end_matches('/')),
            key: api_key.to_owned(),
            model,
            service_label,
        })
    }

    pub fn service_label(&self) -> &'static str {
        self.service_label
    }

    /// One chat-completions call with transport-level retry (429 / 5xx / network).
    /// Takes the internal request and returns the internal response, so the actor
    /// loop is identical for every backend.
    pub async fn generate(
        &self,
        req: &GenerateContentRequest,
    ) -> Result<GenerateContentResponse, GeminiError> {
        let chat = to_chat_request(&self.model, req);
        let mut attempt = 0u32;
        loop {
            let bearer = format!("Bearer {}", self.key);
            let send = self
                .http
                .post(&self.url)
                .header("Authorization", bearer)
                .json(&chat)
                .send()
                .await;

            match send {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let chat_resp =
                            http::json_limited::<ChatResponse>(resp, RESPONSE_BODY_MAX)
                                .await
                                .map_err(|e| {
                                    GeminiError::Decode(sanitize::sanitize_error_text(
                                        e.to_string(),
                                    ))
                                })?;
                        return from_chat_response(chat_resp);
                    }
                    let code = status.as_u16();
                    let retry_after = resp
                        .headers()
                        .get(RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.trim().parse::<u64>().ok());
                    let body = http::read_response_limited(resp, ERROR_BODY_MAX)
                        .await
                        .ok()
                        .and_then(|b| String::from_utf8(b).ok())
                        .map(sanitize::sanitize_error_text)
                        .unwrap_or_default();
                    match code {
                        401 => return Err(GeminiError::Auth),
                        404 => return Err(GeminiError::ModelNotFound),
                        429 => {
                            if attempt >= MAX_RETRIES {
                                return Err(GeminiError::RateLimited);
                            }
                            let secs = retry_after
                                .unwrap_or_else(|| 1u64 << attempt)
                                .min(RETRY_AFTER_CAP_SECS);
                            sleep(Duration::from_secs(secs)).await;
                        }
                        500..=599 => {
                            if attempt >= MAX_RETRIES {
                                return Err(GeminiError::Server(truncate(&body)));
                            }
                            sleep(server_backoff(attempt)).await;
                        }
                        _ => {
                            return Err(GeminiError::Http(format!(
                                "HTTP {code}: {}",
                                truncate(&body)
                            )));
                        }
                    }
                }
                Err(e) => {
                    if attempt >= MAX_RETRIES {
                        return Err(GeminiError::Network(e.to_string()));
                    }
                    sleep(server_backoff(attempt)).await;
                }
            }
            attempt += 1;
        }
    }
}

/// 5xx/network backoff: 0.6 / 1.2 / 2.4 s.
fn server_backoff(attempt: u32) -> Duration {
    Duration::from_millis(600 * (1u64 << attempt))
}

fn truncate(s: &str) -> String {
    let s = s.trim();
    if s.len() <= ERR_BODY_CAP {
        s.to_owned()
    } else {
        let end = s.floor_char_boundary(ERR_BODY_CAP);
        format!("{}…", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::super::client::{GenerationConfig, Tool};
    use super::*;

    fn sample_request() -> GenerateContentRequest {
        GenerateContentRequest {
            contents: vec![
                Content::user(vec![Part::text("play some jazz")]),
                Content {
                    role: Some("model".to_owned()),
                    parts: vec![Part {
                        text: None,
                        function_call: Some(FunctionCall {
                            name: "search_tracks".to_owned(),
                            args: serde_json::json!({"query": "jazz", "limit": 5}),
                        }),
                        function_response: None,
                        extra: Default::default(),
                    }],
                },
                Content::user(vec![Part::function_response(
                    "search_tracks",
                    serde_json::json!([{"videoId": "abc"}]),
                )]),
            ],
            system_instruction: Some(Content {
                role: None,
                parts: vec![Part::text("be brief")],
            }),
            tools: Some(vec![Tool {
                function_declarations: vec![serde_json::json!({
                    "name": "search_tracks",
                    "description": "Search tracks",
                    "parameters": {"type": "object", "properties": {"query": {"type": "string"}}}
                })],
            }]),
            generation_config: Some(GenerationConfig {
                temperature: Some(0.7),
                max_output_tokens: Some(1024),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn request_maps_system_turns_tools_and_config() {
        let chat = to_chat_request("gpt-4o-mini", &sample_request());
        assert_eq!(chat.model, "gpt-4o-mini");
        assert_eq!(chat.messages[0].role, "system");
        assert_eq!(chat.messages[0].content.as_deref(), Some("be brief"));
        assert_eq!(chat.messages[1].role, "user");
        // Assistant turn carries tool_calls with a deterministic id.
        assert_eq!(chat.messages[2].role, "assistant");
        let calls = chat.messages[2].tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "search_tracks");
        assert_eq!(calls[0].id, "call_1_0");
        assert!(calls[0].function.arguments.contains("\"query\":\"jazz\""));
        // Tool-result turn references that id.
        assert_eq!(chat.messages[3].role, "tool");
        assert_eq!(
            chat.messages[3].tool_call_id.as_deref(),
            Some("call_1_0")
        );
        // Tool schemas pass through as OpenAI functions.
        let tools = chat.tools.unwrap();
        assert_eq!(tools[0].function.name, "search_tracks");
        assert_eq!(
            tools[0].function.parameters.as_ref().unwrap()["type"],
            "object"
        );
        assert_eq!(chat.temperature, Some(0.7));
        assert_eq!(chat.max_tokens, Some(1024));
    }

    #[test]
    fn json_mime_type_requests_json_object() {
        let mut req = sample_request();
        req.generation_config.as_mut().unwrap().response_mime_type =
            Some("application/json".to_owned());
        let chat = to_chat_request("m", &req);
        assert_eq!(chat.response_format.unwrap().kind, "json_object");
    }

    #[test]
    fn response_maps_text_tool_calls_usage_and_finish() {
        let raw = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "on it",
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {"name": "play_music", "arguments": "{\"query\":\"lofi\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 120, "completion_tokens": 30, "total_tokens": 150}
        });
        let resp: ChatResponse = serde_json::from_value(raw).unwrap();
        let out = from_chat_response(resp).unwrap();
        let content = out.content().unwrap();
        assert_eq!(content.joined_text(), "on it");
        let calls = content.function_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "play_music");
        assert_eq!(calls[0].args["query"], "lofi");
        assert_eq!(out.finish_reason(), Some("STOP"));
        let usage = out.usage().unwrap();
        assert_eq!(usage.prompt_token_count, 120);
        assert_eq!(usage.candidates_token_count, 30);
        assert_eq!(usage.total_token_count, 150);
    }

    #[test]
    fn string_content_blocks_and_length_map() {
        // Array content blocks join their text.
        let raw = serde_json::json!({
            "choices": [{"message": {"content": [{"type": "text", "text": "he"}, {"type": "text", "text": "llo"}]}, "finish_reason": "length"}],
            "usage": {}
        });
        let out =
            from_chat_response(serde_json::from_value(raw).unwrap()).unwrap();
        assert_eq!(out.content().unwrap().joined_text(), "hello");
        assert_eq!(out.finish_reason(), Some("MAX_TOKENS"));
    }

    #[test]
    fn empty_choices_is_a_decode_error() {
        let raw = serde_json::json!({"choices": []});
        let out = from_chat_response(serde_json::from_value(raw).unwrap());
        assert!(matches!(out, Err(GeminiError::Decode(_))));
    }

    #[test]
    fn presets_carry_expected_defaults() {
        assert_eq!(
            OpenAiPreset::OpenAi.default_base_url(),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            OpenAiPreset::Xai.default_base_url(),
            "https://api.x.ai/v1"
        );
        assert_eq!(
            OpenAiPreset::OpenCode.default_base_url(),
            "http://127.0.0.1:4096/v1"
        );
        assert_eq!(OpenAiPreset::Xai.label(), "xAI");
    }

    #[test]
    fn client_rejects_empty_model() {
        assert!(OpenAiClient::new("https://x/v1", "k", String::new(), "T").is_err());
    }
}

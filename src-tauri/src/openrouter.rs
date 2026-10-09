use std::sync::OnceLock;
use std::time::Duration;

use serde::Deserialize;

use crate::types::ChatMessage;

const OPENROUTER_BASE: &str = "https://openrouter.ai/api/v1";

pub fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("http client")
    })
}

#[derive(Deserialize)]
struct Choice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
}

/// A model call that has not answered by now is treated as failed (and retried).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_ATTEMPTS: u32 = 3;
const MAX_RETRY_WAIT: Duration = Duration::from_secs(8);

enum Attempt {
    /// Worth trying again: a rate limit or a provider hiccup.
    Retry { message: String, wait: Option<Duration> },
    Fail(String),
}

fn is_retryable(code: u64) -> bool {
    matches!(code, 429 | 500 | 502 | 503 | 504)
}

/// Posts one chat-completions request and returns the assistant message,
/// which may carry `tool_calls` instead of text.
///
/// Providers behind OpenRouter rate limit or fail briefly now and then (the
/// limit is theirs, not the account's), so those answers are retried a couple
/// of times with a short wait before giving up.
pub async fn post_chat(api_key: &str, body: &serde_json::Value) -> Result<ChatMessage, String> {
    let mut last = String::new();
    for attempt in 0..MAX_ATTEMPTS {
        match try_once(api_key, body).await {
            Ok(message) => return Ok(message),
            Err(Attempt::Fail(message)) => return Err(message),
            Err(Attempt::Retry { message, wait }) => {
                last = message;
                if attempt + 1 < MAX_ATTEMPTS {
                    let wait = wait.unwrap_or(Duration::from_secs(1 << attempt)).min(MAX_RETRY_WAIT);
                    tokio::time::sleep(wait).await;
                }
            }
        }
    }
    Err(format!(
        "The model's provider is busy or rate limiting it right now. Try again in a moment, or choose a different model in settings. ({last})"
    ))
}

async fn try_once(api_key: &str, body: &serde_json::Value) -> Result<ChatMessage, Attempt> {
    let started = std::time::Instant::now();
    let result = try_once_inner(api_key, body).await;
    if std::env::var_os("LEO_TRACE").is_some() {
        let outcome = match &result {
            Ok(m) if m.tool_calls.is_some() => "tool call".to_string(),
            Ok(_) => "text".to_string(),
            Err(Attempt::Retry { message, .. }) => format!("RETRY: {}", message.chars().take(70).collect::<String>()),
            Err(Attempt::Fail(message)) => format!("FAIL: {}", message.chars().take(70).collect::<String>()),
        };
        eprintln!(
            "   [model call {:.1}s, {}: {outcome}]",
            started.elapsed().as_secs_f32(),
            body["model"].as_str().unwrap_or("?")
        );
    }
    result
}

async fn try_once_inner(api_key: &str, body: &serde_json::Value) -> Result<ChatMessage, Attempt> {
    let resp = client()
        .post(format!("{OPENROUTER_BASE}/chat/completions"))
        .bearer_auth(api_key)
        .json(body)
        .send()
        .await
        .map_err(|e| Attempt::Retry { message: e.to_string(), wait: None })?;

    let status = resp.status();
    if !status.is_success() {
        let wait = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs);
        let text = resp.text().await.unwrap_or_default();
        let message = format!("OpenRouter error {status}: {text}");
        return Err(if is_retryable(status.as_u16() as u64) {
            Attempt::Retry { message, wait }
        } else {
            Attempt::Fail(message)
        });
    }

    // OpenRouter can answer 200 with an `error` object when the upstream
    // provider fails, so look for that before expecting `choices`.
    let value: serde_json::Value = resp.json().await.map_err(|e| Attempt::Fail(e.to_string()))?;
    if let Some(err) = value.get("error") {
        let message = format!("OpenRouter error: {err}");
        let code = err["code"].as_u64().unwrap_or(0);
        return Err(if is_retryable(code) {
            Attempt::Retry { message, wait: None }
        } else {
            Attempt::Fail(message)
        });
    }
    trace_usage(&value["usage"]);
    let parsed: ChatResponse = serde_json::from_value(value).map_err(|e| Attempt::Fail(e.to_string()))?;
    parsed
        .choices
        .into_iter()
        .next()
        .map(|c| c.message)
        .ok_or_else(|| Attempt::Fail("empty response from OpenRouter".to_string()))
}

/// Like `post_chat`, but asks for a streamed answer and calls `on_delta` with
/// each piece of text as it arrives, so the UI can show a reply while it is
/// still being written. Tool calls are collected and returned whole.
///
/// Only a failure before anything has arrived is retried; once text has been
/// shown, starting over would repeat it.
pub async fn post_chat_stream(
    api_key: &str,
    body: &serde_json::Value,
    on_delta: &mut (dyn FnMut(&str) + Send),
) -> Result<ChatMessage, String> {
    let mut body = body.clone();
    body["stream"] = serde_json::json!(true);
    let mut last = String::new();
    for attempt in 0..MAX_ATTEMPTS {
        match stream_once(api_key, &body, on_delta).await {
            Ok(message) => return Ok(message),
            Err(Attempt::Fail(message)) => return Err(message),
            Err(Attempt::Retry { message, wait }) => {
                last = message;
                if attempt + 1 < MAX_ATTEMPTS {
                    let wait = wait.unwrap_or(Duration::from_secs(1 << attempt)).min(MAX_RETRY_WAIT);
                    tokio::time::sleep(wait).await;
                }
            }
        }
    }
    Err(format!(
        "The model's provider is busy or rate limiting it right now. Try again in a moment, or choose a different model in settings. ({last})"
    ))
}

async fn stream_once(
    api_key: &str,
    body: &serde_json::Value,
    on_delta: &mut (dyn FnMut(&str) + Send),
) -> Result<ChatMessage, Attempt> {
    let started = std::time::Instant::now();
    let mut resp = client()
        .post(format!("{OPENROUTER_BASE}/chat/completions"))
        .bearer_auth(api_key)
        .json(body)
        .send()
        .await
        .map_err(|e| Attempt::Retry { message: e.to_string(), wait: None })?;

    let status = resp.status();
    if !status.is_success() {
        let wait = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs);
        let text = resp.text().await.unwrap_or_default();
        let message = format!("OpenRouter error {status}: {text}");
        return Err(if is_retryable(status.as_u16() as u64) {
            Attempt::Retry { message, wait }
        } else {
            Attempt::Fail(message)
        });
    }

    let mut acc = StreamAccumulator::default();
    let mut first_byte: Option<f32> = None;
    // Bytes are decoded only at line ends, so a character split across two
    // network chunks is never cut in half.
    let mut raw: Vec<u8> = Vec::new();
    'stream: loop {
        let chunk = match resp.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(e) => {
                let message = e.to_string();
                return Err(if acc.started() { Attempt::Fail(message) } else { Attempt::Retry { message, wait: None } });
            }
        };
        first_byte.get_or_insert_with(|| started.elapsed().as_secs_f32());
        raw.extend_from_slice(&chunk);
        while let Some(pos) = raw.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = raw.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line).trim().to_string();
            match acc.feed_line(&line, on_delta) {
                Ok(true) => {}
                Ok(false) => break 'stream,
                Err(err) => {
                    return Err(if acc.started() { Attempt::Fail(err.message) } else { err.into_attempt() });
                }
            }
        }
    }
    if std::env::var_os("LEO_TRACE").is_some() {
        eprintln!(
            "   [model stream {:.1}s (first text {:.1}s), {}]",
            started.elapsed().as_secs_f32(),
            first_byte.unwrap_or(0.0),
            body["model"].as_str().unwrap_or("?")
        );
    }
    Ok(acc.into_message())
}

#[derive(Debug)]
struct StreamError {
    message: String,
    retryable: bool,
}

impl StreamError {
    fn into_attempt(self) -> Attempt {
        if self.retryable {
            Attempt::Retry { message: self.message, wait: None }
        } else {
            Attempt::Fail(self.message)
        }
    }
}

/// Builds the final assistant message out of the streamed pieces.
#[derive(Default)]
struct StreamAccumulator {
    content: String,
    /// Tool calls by their index in the stream: (id, name, arguments).
    calls: Vec<(String, String, String)>,
}

impl StreamAccumulator {
    fn started(&self) -> bool {
        !self.content.is_empty() || !self.calls.is_empty()
    }

    /// Handles one line of the event stream. `Ok(false)` means the stream is over.
    fn feed_line(&mut self, line: &str, on_delta: &mut (dyn FnMut(&str) + Send)) -> Result<bool, StreamError> {
        let Some(data) = line.strip_prefix("data:") else { return Ok(true) };
        let data = data.trim();
        if data == "[DONE]" {
            return Ok(false);
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else { return Ok(true) };
        if let Some(err) = value.get("error") {
            let code = err["code"].as_u64().unwrap_or(0);
            return Err(StreamError { message: format!("OpenRouter error: {err}"), retryable: is_retryable(code) });
        }
        trace_usage(&value["usage"]);
        let delta = &value["choices"][0]["delta"];
        if let Some(text) = delta["content"].as_str() {
            if !text.is_empty() {
                self.content.push_str(text);
                on_delta(text);
            }
        }
        if let Some(calls) = delta["tool_calls"].as_array() {
            for call in calls {
                let index = call["index"].as_u64().unwrap_or(0) as usize;
                while self.calls.len() <= index {
                    self.calls.push((String::new(), String::new(), String::new()));
                }
                let entry = &mut self.calls[index];
                if let Some(id) = call["id"].as_str() {
                    entry.0 = id.to_string();
                }
                if let Some(name) = call["function"]["name"].as_str() {
                    entry.1.push_str(name);
                }
                if let Some(args) = call["function"]["arguments"].as_str() {
                    entry.2.push_str(args);
                }
            }
        }
        Ok(true)
    }

    fn into_message(self) -> ChatMessage {
        let calls: Vec<crate::types::ToolCall> = self
            .calls
            .into_iter()
            .filter(|(_, name, _)| !name.is_empty())
            .enumerate()
            .map(|(i, (id, name, arguments))| crate::types::ToolCall {
                id: if id.is_empty() { format!("call_{i}") } else { id },
                kind: "function".to_string(),
                function: crate::types::FunctionCall { name, arguments },
            })
            .collect();
        ChatMessage {
            role: "assistant".to_string(),
            content: if self.content.is_empty() { None } else { Some(self.content) },
            tool_calls: if calls.is_empty() { None } else { Some(calls) },
            tool_call_id: None,
        }
    }
}

/// Marks the system prompt as cacheable. Qwen models on OpenRouter only cache
/// when asked (`cache_control` on a content block); a cache hit reads that
/// prefix at a fraction of the normal input price and is faster. Models that
/// cache on their own, or not at all, ignore the marker.
pub fn cache_system_prompt(body: &mut serde_json::Value) {
    let Some(first) = body["messages"].get_mut(0) else { return };
    if first["role"] != "system" {
        return;
    }
    let Some(text) = first["content"].as_str().map(str::to_string) else { return };
    first["content"] = serde_json::json!([
        { "type": "text", "text": text, "cache_control": { "type": "ephemeral" } }
    ]);
}

/// With `LEO_TRACE` set, prints how much of the prompt came from the cache.
fn trace_usage(usage: &serde_json::Value) {
    if std::env::var_os("LEO_TRACE").is_none() || !usage.is_object() {
        return;
    }
    eprintln!(
        "   [tokens: prompt {}, cached {}, cache-write {}, out {}]",
        usage["prompt_tokens"],
        usage["prompt_tokens_details"]["cached_tokens"],
        usage["prompt_tokens_details"]["cache_write_tokens"],
        usage["completion_tokens"]
    );
}

/// Pulls the first JSON object out of a model reply, tolerating code fences
/// and chatter around it.
pub fn extract_json(text: &str) -> Result<serde_json::Value, String> {
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else {
        return Err("the model did not return JSON".to_string());
    };
    serde_json::from_str(&text[start..=end]).map_err(|e| format!("bad JSON from the model: {e}"))
}

/// A short, tool-less call whose whole answer must fit in a small budget
/// (routing, planning, checking, the final write-up).
///
/// "Thinking" models spend their token budget on hidden reasoning, which can
/// leave a small budget with no answer at all. So the first try switches
/// reasoning off; models that insist on reasoning get a second try with a
/// much larger budget.
pub async fn quick_chat(api_key: &str, mut body: serde_json::Value) -> Result<ChatMessage, String> {
    let mut fast = body.clone();
    fast["reasoning"] = serde_json::json!({ "enabled": false });
    if let Ok(reply) = post_chat(api_key, &fast).await {
        if reply.content.as_deref().is_some_and(|c| !c.trim().is_empty()) {
            return Ok(reply);
        }
    }

    let tokens = body["max_tokens"].as_u64().unwrap_or(500);
    body["max_tokens"] = serde_json::json!((tokens * 4).max(2000));
    post_chat(api_key, &body).await
}

/// A quick call that must answer in JSON. Used by the orchestrator, planner
/// and verifier.
pub async fn json_call(
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<serde_json::Value, String> {
    let mut body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
        "temperature": 0.1,
        "max_tokens": max_tokens,
    });
    cache_system_prompt(&mut body);
    let reply = quick_chat(api_key, body).await?;
    extract_json(&reply.content.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::{extract_json, StreamAccumulator};

    #[test]
    fn stream_pieces_become_one_message() {
        let mut acc = StreamAccumulator::default();
        let mut seen = String::new();
        let mut on = |t: &str| seen.push_str(t);
        for line in [
            r#"data: {"choices":[{"delta":{"content":"Hel"}}]}"#,
            ": keep-alive",
            r#"data: {"choices":[{"delta":{"content":"lo"}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"web_search","arguments":"{\"q\":"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"x\"}"}}]}}]}"#,
        ] {
            assert!(acc.feed_line(line, &mut on).unwrap());
        }
        assert!(!acc.feed_line("data: [DONE]", &mut on).unwrap());
        let m = acc.into_message();
        assert_eq!(m.content.as_deref(), Some("Hello"));
        let calls = m.tool_calls.unwrap();
        assert_eq!(calls[0].function.name, "web_search");
        assert_eq!(calls[0].function.arguments, r#"{"q":"x"}"#);
        assert_eq!(seen, "Hello");
    }

    #[test]
    fn finds_json_in_chatter_and_fences() {
        let v = extract_json("Sure!\n```json\n{\"route\": \"plan\", \"n\": 2}\n```\nDone.").unwrap();
        assert_eq!(v["route"], "plan");
        assert!(extract_json("no json here").is_err());
        assert!(extract_json("{ broken").is_err());
    }
}

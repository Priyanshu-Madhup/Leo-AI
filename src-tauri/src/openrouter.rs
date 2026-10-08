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
    let parsed: ChatResponse = serde_json::from_value(value).map_err(|e| Attempt::Fail(e.to_string()))?;
    parsed
        .choices
        .into_iter()
        .next()
        .map(|c| c.message)
        .ok_or_else(|| Attempt::Fail("empty response from OpenRouter".to_string()))
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
    let body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
        "temperature": 0.1,
        "max_tokens": max_tokens,
    });
    let reply = quick_chat(api_key, body).await?;
    extract_json(&reply.content.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::extract_json;

    #[test]
    fn finds_json_in_chatter_and_fences() {
        let v = extract_json("Sure!\n```json\n{\"route\": \"plan\", \"n\": 2}\n```\nDone.").unwrap();
        assert_eq!(v["route"], "plan");
        assert!(extract_json("no json here").is_err());
        assert!(extract_json("{ broken").is_err());
    }
}

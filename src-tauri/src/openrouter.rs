use std::sync::OnceLock;
use std::time::Duration;

use serde::Deserialize;

use crate::types::ChatMessage;

const OPENROUTER_BASE: &str = "https://openrouter.ai/api/v1";

pub fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
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

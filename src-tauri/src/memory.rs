//! Long-term memory backed by MemoryLake (https://docs.memorylake.ai).
//!
//! Writing: messages are appended to a conversation; MemoryLake extracts
//! structured facts from them in the background and stores them in the
//! project, so facts outlive any single conversation. Each app session starts
//! its own conversation: the API only lets a message be appended to the
//! current head, and offers no cheap way to look the head up later.
//! Reading: natural-language search over the workspace's facts and documents.
//!
//! The tools in tools.rs (`recall_memory`, `remember`) are thin wrappers over
//! this module, so any agent that is given those tools - the utility agent
//! today, the planner and executors later - can use memory.

use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tauri::async_runtime::Mutex;

use crate::openrouter::client;

const BASE_URL: &str = "https://app.memorylake.ai";
const API_PREFIX: &str = "/openapi/memorylake/api/v3";
const PROJECT_CUSTOM_ID: &str = "leo-memory";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RECALL_FACTS: usize = 8;
const MAX_RECALL_DOCS: usize = 3;
const MAX_SNIPPET_CHARS: usize = 300;
/// MemoryLake needs a little while to extract and index a saved fact, so
/// facts saved in the last few minutes are also served from a local list.
const RECENT_WINDOW: Duration = Duration::from_secs(10 * 60);
const MAX_RECENT: usize = 20;

#[derive(Clone)]
struct Config {
    api_key: String,
}

/// Resolved MemoryLake resources, found or created on first use.
struct Ids {
    workspace: String,
    actor: String,
    project: String,
    /// Created on the first write of a session.
    conversation: Option<String>,
    /// Id of the newest message in the conversation; each new message must
    /// name it as its parent (`None` only for the very first message).
    last_message: Option<String>,
}

#[derive(Default)]
pub struct MemoryState {
    config: Mutex<Option<Config>>,
    ids: Mutex<Option<Ids>>,
    recent: StdMutex<Vec<(String, Instant)>>,
}

impl MemoryState {
    pub async fn is_configured(&self) -> bool {
        self.config.lock().await.is_some()
    }

    pub async fn remember(&self, fact: &str) -> Result<String, String> {
        let cfg = self.config().await?;
        // Held for the whole call so concurrent writes can't fork the
        // message chain.
        let mut ids = self.ids.lock().await;

        for attempt in 0..2 {
            let resolved = match ids.as_mut() {
                Some(r) => r,
                None => ids.insert(bootstrap(&cfg).await?),
            };
            if resolved.conversation.is_none() {
                match create_conversation(&cfg, resolved).await {
                    Ok(id) => {
                        resolved.conversation = Some(id);
                        resolved.last_message = None;
                    }
                    Err(err) => return Err(err),
                }
            }
            let body = json!({
                "custom_id": format!("leo-{}", unique_suffix()),
                "actor_id": resolved.actor,
                "parent_message_id": resolved.last_message,
                "content": [{ "block_type": "TEXT", "text": fact }],
            });
            let path = format!(
                "/conversations/{}/messages",
                resolved.conversation.as_deref().unwrap_or_default()
            );
            match call(&cfg, "POST", &path, Some(body)).await {
                Ok(data) => {
                    resolved.last_message = data["id"].as_str().map(str::to_string);
                    let mut recent = self.recent.lock().unwrap();
                    recent.push((fact.to_string(), Instant::now()));
                    if recent.len() > MAX_RECENT {
                        recent.remove(0);
                    }
                    return Ok("Saved to memory. It becomes searchable in a few seconds.".to_string());
                }
                // Most likely the message chain got out of step; start over
                // with a fresh conversation once.
                Err(_) if attempt == 0 => *ids = None,
                Err(err) => return Err(err),
            }
        }
        unreachable!()
    }

    pub async fn recall(&self, query: &str) -> Result<String, String> {
        let cfg = self.config().await?;
        let workspace = {
            let mut ids = self.ids.lock().await;
            if ids.is_none() {
                *ids = Some(bootstrap(&cfg).await?);
            }
            ids.as_ref().unwrap().workspace.clone()
        };

        let data = call(
            &cfg,
            "POST",
            &format!("/workspaces/{workspace}/memories/search"),
            Some(json!({ "query": query, "top_k": MAX_RECALL_FACTS })),
        )
        .await?;
        let mut text = format_results(&data);

        // Facts saved moments ago may not be searchable yet.
        let fresh: Vec<String> = self
            .recent
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, at)| at.elapsed() < RECENT_WINDOW)
            .map(|(fact, _)| format!("- {fact}"))
            .collect();
        if !fresh.is_empty() {
            if text == "No matching memories." {
                text.clear();
            } else {
                text.push('\n');
            }
            text.push_str("Just saved (still being indexed):\n");
            text.push_str(&fresh.join("\n"));
        }
        Ok(text)
    }

    pub async fn configure(&self, api_key: &str) {
        let key = api_key.trim();
        *self.config.lock().await = if key.is_empty() {
            None
        } else {
            Some(Config { api_key: key.to_string() })
        };
        // A different key may mean a different account.
        *self.ids.lock().await = None;
    }

    async fn config(&self) -> Result<Config, String> {
        self.config
            .lock()
            .await
            .clone()
            .ok_or_else(|| "Memory isn't set up. Add the MemoryLake API key in settings.".to_string())
    }
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn format_results(data: &Value) -> String {
    let mut lines = Vec::new();

    for fact in data["facts"].as_array().into_iter().flatten().take(MAX_RECALL_FACTS) {
        if let Some(text) = fact["fact"].as_str() {
            lines.push(format!("- {text}"));
        }
    }
    for doc in data["documents"].as_array().into_iter().flatten().take(MAX_RECALL_DOCS) {
        let name = doc["document_name"].as_str().unwrap_or("document");
        if let Some(snippet) = doc["items"][0]["text"].as_str() {
            let snippet: String = snippet.chars().take(MAX_SNIPPET_CHARS).collect();
            lines.push(format!("- From \"{name}\": {snippet}"));
        }
    }

    if lines.is_empty() {
        "No matching memories.".to_string()
    } else {
        lines.join("\n")
    }
}

/// Calls the MemoryLake API and returns the `data` payload of a successful
/// response.
async fn call(cfg: &Config, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    let url = format!("{BASE_URL}{API_PREFIX}{path}");
    let mut request = match method {
        "POST" => client().post(url),
        _ => client().get(url),
    }
    .bearer_auth(&cfg.api_key)
    .timeout(REQUEST_TIMEOUT);
    if let Some(body) = body {
        request = request.json(&body);
    }

    let resp = request.send().await.map_err(|e| format!("MemoryLake unreachable: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);

    if status.is_success() && value["success"].as_bool().unwrap_or(false) {
        return Ok(value["data"].clone());
    }
    let reason = value["message"].as_str().unwrap_or(text.as_str());
    Err(format!("MemoryLake error {status}: {reason}"))
}

async fn create_conversation(cfg: &Config, ids: &Ids) -> Result<String, String> {
    let data = call(
        cfg,
        "POST",
        &format!("/workspaces/{}/memories/conversations", ids.workspace),
        Some(json!({
            "custom_id": format!("leo-{}", unique_suffix()),
            "kind": "DIRECT",
            "rw_project_ids": [ids.project],
            "actor_ids": [ids.actor],
            "name": "Leo session",
        })),
    )
    .await?;
    data["id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "Malformed conversation.".to_string())
}

/// Finds (or creates) what memory needs: the account's workspace and own
/// actor, plus a dedicated project for Leo's facts.
async fn bootstrap(cfg: &Config) -> Result<Ids, String> {
    let find = |items: &Value, key: &str, value: &str| -> Option<Value> {
        items["items"]
            .as_array()?
            .iter()
            .find(|i| i[key].as_str() == Some(value))
            .cloned()
    };

    let workspaces = call(cfg, "GET", "/workspaces?page_size=100", None).await?;
    let workspace = find(&workspaces, "custom_id", "_sys_default_workspace")
        .or_else(|| workspaces["items"].get(0).cloned())
        .ok_or("No MemoryLake workspace found for this key.")?;
    let workspace_id = workspace["id"].as_str().ok_or("Malformed workspace.")?.to_string();

    // The key's owner already has a HUMAN actor; messages are attributed to it.
    let actors = call(cfg, "GET", "/actors?page_size=100", None).await?;
    let actor = match find(&actors, "actor_type", "HUMAN").or_else(|| actors["items"].get(0).cloned()) {
        Some(a) => a,
        None => {
            call(
                cfg,
                "POST",
                "/actors",
                Some(json!({ "custom_id": "leo-user", "display_name": "Leo user" })),
            )
            .await?
        }
    };
    let actor_id = actor["id"].as_str().ok_or("Malformed actor.")?.to_string();

    let projects = call(cfg, "GET", &format!("/workspaces/{workspace_id}/projects?page_size=100"), None).await?;
    let project = match find(&projects, "custom_id", PROJECT_CUSTOM_ID) {
        Some(p) => p,
        None => {
            call(
                cfg,
                "POST",
                &format!("/workspaces/{workspace_id}/projects"),
                Some(json!({
                    "custom_id": PROJECT_CUSTOM_ID,
                    "name": "Leo memory",
                    "description": "Long-term memory for the Leo assistant",
                })),
            )
            .await?
        }
    };
    let project_id = project["id"].as_str().ok_or("Malformed project.")?.to_string();

    Ok(Ids {
        workspace: workspace_id,
        actor: actor_id,
        project: project_id,
        conversation: None,
        last_message: None,
    })
}

#[tauri::command]
pub async fn memory_is_configured(state: tauri::State<'_, MemoryState>) -> Result<bool, String> {
    Ok(state.is_configured().await)
}

#[tauri::command]
pub async fn memory_configure(
    state: tauri::State<'_, MemoryState>,
    api_key: String,
) -> Result<(), String> {
    state.configure(&api_key).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live check against the real API; run with MEMORYLAKE_KEY set:
    /// `cargo test live_recall -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_recall() {
        let key = std::env::var("MEMORYLAKE_KEY").expect("set MEMORYLAKE_KEY");
        tauri::async_runtime::block_on(async {
            let state = MemoryState::default();
            state.configure(&key).await;
            let result = state.recall("what is the user's name").await;
            println!("RECALL RESULT: {result:?}");
            assert!(result.is_ok());
        });
    }
}

//! Questions and approvals that need the user.
//!
//! Any agent (the utility agent today; the planner and executors later) can
//! pause and show a card in the chat:
//!
//! - `ask_user` is a tool the model calls when a request is ambiguous or
//!   needs a choice. The card shows the question with option buttons and a
//!   free-text field.
//! - `approve` is run by the agent loop itself before any tool that changes
//!   something. The card shows exactly what would happen (e.g. the full
//!   email) with Allow / Decline.
//!
//! The run waits on a channel until the frontend answers via `agent_answer`,
//! and gives up if the turn is cancelled or the user never answers.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, State};
use tokio::sync::oneshot;
use tokio::time::timeout;

use crate::agent::{emit, AgentEvent};
use crate::tools;
use crate::types::ToolCall;

const ANSWER_TIMEOUT: Duration = Duration::from_secs(600);
const POLL_INTERVAL: Duration = Duration::from_millis(400);
const MAX_OPTIONS: usize = 8;

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;

#[derive(Default)]
pub struct InteractState {
    pending: Pending,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AskOption {
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
}

pub enum Approval {
    Allowed,
    /// The user declined; the string is what to tell the model.
    Declined(String),
}

#[derive(Deserialize)]
struct AskArgs {
    question: String,
    #[serde(default)]
    options: Vec<AskOption>,
}

/// Starts listening for the answer to card `id`. This must happen BEFORE the
/// card is announced: if the answer could arrive first, it would find nobody
/// waiting, be dropped, and the run would wait forever.
fn register(app: &AppHandle, id: &str) -> oneshot::Receiver<Value> {
    let (tx, rx) = oneshot::channel();
    app.state::<InteractState>().pending.lock().unwrap().insert(id.to_string(), tx);
    rx
}

async fn wait_for_answer(
    app: &AppHandle,
    id: &str,
    mut rx: oneshot::Receiver<Value>,
    turn: &Arc<AtomicU64>,
    my_turn: u64,
) -> Result<Value, String> {
    let state = app.state::<InteractState>();

    let deadline = Instant::now() + ANSWER_TIMEOUT;
    loop {
        match timeout(POLL_INTERVAL, &mut rx).await {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(_)) => return Err("The question was dismissed.".to_string()),
            Err(_) => {
                // Woken only to check whether this turn is still wanted.
                if turn.load(Ordering::SeqCst) != my_turn {
                    state.pending.lock().unwrap().remove(id);
                    return Err("cancelled".to_string());
                }
                if Instant::now() > deadline {
                    state.pending.lock().unwrap().remove(id);
                    return Err("The user didn't answer in time.".to_string());
                }
            }
        }
    }
}

/// Shows a question card and returns the user's answer for the model.
pub async fn ask_user(
    app: &AppHandle,
    turn: &Arc<AtomicU64>,
    my_turn: u64,
    call: &ToolCall,
) -> Result<String, String> {
    let args: AskArgs = serde_json::from_str(&call.function.arguments)
        .map_err(|e| format!("Bad arguments: {e}"))?;
    let options: Vec<AskOption> = args.options.into_iter().take(MAX_OPTIONS).collect();

    let id = format!("ask-{}", call.id);
    let rx = register(app, &id);
    emit(
        app,
        AgentEvent::AskUser {
            id: id.clone(),
            question: args.question,
            options,
        },
    );

    let answer = wait_for_answer(app, &id, rx, turn, my_turn).await?;
    let text = answer["text"].as_str().unwrap_or("").trim();
    if text.is_empty() {
        return Err("The user gave no answer.".to_string());
    }
    Ok(format!("The user answered: {text}"))
}

/// Shows an approval card for a tool call and waits for Allow / Decline.
pub async fn approve(
    app: &AppHandle,
    turn: &Arc<AtomicU64>,
    my_turn: u64,
    call: &ToolCall,
) -> Result<Approval, String> {
    let id = format!("approve-{}", call.id);
    let args: Value = serde_json::from_str(&call.function.arguments).unwrap_or(Value::Null);
    let rx = register(app, &id);
    emit(
        app,
        AgentEvent::ApprovalRequest {
            id: id.clone(),
            name: call.function.name.clone(),
            label: tools::describe(&call.function.name, &call.function.arguments),
            args,
        },
    );

    let answer = wait_for_answer(app, &id, rx, turn, my_turn).await?;
    if answer["allow"].as_bool().unwrap_or(false) {
        return Ok(Approval::Allowed);
    }
    let note = answer["note"].as_str().unwrap_or("").trim();
    Ok(Approval::Declined(if note.is_empty() {
        "The user declined this action. Do not retry it; ask what they want instead.".to_string()
    } else {
        format!("The user declined this action and said: {note}")
    }))
}

#[tauri::command]
pub fn agent_answer(state: State<'_, InteractState>, id: String, payload: Value) {
    answer(state.inner(), &id, payload);
}

/// Delivers the user's answer to whoever is waiting on card `id`.
pub fn answer(state: &InteractState, id: &str, payload: Value) {
    if let Some(tx) = state.pending.lock().unwrap().remove(id) {
        let _ = tx.send(payload);
    }
}

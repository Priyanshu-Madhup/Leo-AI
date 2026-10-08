//! The agent engine and the entry point for a user message.
//!
//! `agent_run` is the front door. The orchestrator picks a route:
//!   - utility: the utility agent answers (chat, web, memory, opening things);
//!   - google:  the Google agent handles one clear Google Workspace job;
//!   - plan:    the planner builds a step plan, runs each step on an agent and
//!              has the verifier check it (see planner.rs).
//!
//! `run_agent` is the one tool-calling loop every agent uses: model -> tool
//! calls -> results -> model, until a plain reply. Calling another agent is
//! just a tool call (`ask_gmail`, `ask_utility`, ...) that runs that agent's
//! loop and returns its answer; agents.rs says who may call whom.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::agents::{self, AgentId};
use crate::interact::{self, Approval, AskOption};
use crate::memory::MemoryState;
use crate::openrouter::post_chat;
use crate::tools;
use crate::types::{ChatMessage, ToolCall};
use crate::web::WebState;

const MAX_ITERATIONS: usize = 8;
const MAX_DEPTH: usize = 3;
const MAX_TOOL_RESULT_CHARS: usize = 8000;
const MAX_HISTORY_MESSAGES: usize = 40;
const MAX_LOG_LINE: usize = 240;
/// Room for a long tool call (e.g. a whole document). If a reply is cut off
/// mid-call the arguments are broken JSON, so this must not be too small.
const MAX_OUTPUT_TOKENS: u32 = 4096;

/// One request may run this long, not counting time spent waiting for the user
/// to answer a card. Past it, Leo stops rather than spin forever.
const JOB_LIMIT: Duration = Duration::from_secs(300);
const TOO_SLOW: &str = "That is taking too long, so I stopped. Try again, or choose a faster model in settings.";

const BAD_ARGUMENTS: &str = "Your tool call could not be read: its arguments were not valid JSON, most likely because the message was cut off for being too long. Nothing was done. Try again with less text in the call. For a long document, create it with the first part, then add the rest in several smaller edits.";

#[derive(Default)]
pub struct AgentState {
    /// The conversation so far: the user's messages and Leo's final replies.
    history: Mutex<Vec<ChatMessage>>,
    /// Bumped by every new run and by `agent_cancel`; a run whose id no
    /// longer matches has been superseded and stops at its next checkpoint.
    turn: Arc<AtomicU64>,
}

#[derive(Serialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AgentEvent {
    ToolStart {
        id: String,
        name: String,
        label: String,
        detail: Option<String>,
        brand: tools::Brand,
    },
    ToolResult { id: String, name: String, ok: bool, error: Option<String> },
    /// What Leo is doing right now, in plain words ("Planning the steps…").
    Progress { text: String },
    /// A question card: waits for `agent_answer` with `{ "text": ... }`.
    AskUser { id: String, question: String, options: Vec<AskOption> },
    /// An approval card: waits for `agent_answer` with `{ "allow": bool, "note": ... }`.
    ApprovalRequest { id: String, name: String, label: String, args: serde_json::Value },
}

/// The "started" event for a tool call, described in plain words with its brand.
pub(crate) fn tool_start(call: &ToolCall) -> AgentEvent {
    let p = tools::present(&call.function.name, &call.function.arguments);
    AgentEvent::ToolStart {
        id: call.id.clone(),
        name: call.function.name.clone(),
        label: p.title,
        detail: p.detail,
        brand: p.brand,
    }
}

pub(crate) fn emit(app: &AppHandle, event: AgentEvent) {
    let _ = app.emit("agent://event", event);
}

/// Which model each role uses (planner and verifier default to the main one).
#[derive(Clone)]
pub struct Models {
    pub main: String,
    pub planner: String,
    pub verifier: String,
}

/// Everything one user request shares across all the agents it involves.
#[derive(Clone)]
pub struct Ctx {
    pub app: AppHandle,
    pub api_key: String,
    pub models: Models,
    pub(crate) turn: Arc<AtomicU64>,
    pub(crate) my_turn: u64,
    pub depth: usize,
    /// Set once any tool has returned text written by someone else.
    tainted: Arc<AtomicBool>,
    /// Number of approved, state-changing actions carried out so far.
    writes: Arc<AtomicUsize>,
    /// Plain-language record of tool calls, read by the verifier.
    log: Arc<Mutex<Vec<String>>>,
    /// Facts saved to memory during this request.
    saved: Arc<Mutex<Vec<String>>>,
    /// When the request gives up; moved back while waiting for the user.
    deadline: Arc<Mutex<Instant>>,
}

impl Ctx {
    pub fn cancelled(&self) -> bool {
        self.turn.load(Ordering::SeqCst) != self.my_turn
    }

    /// `Err("cancelled")` once a newer request has taken over.
    pub fn check(&self) -> Result<(), String> {
        if self.cancelled() {
            Err("cancelled".to_string())
        } else if Instant::now() > *self.deadline.lock().unwrap() {
            Err(TOO_SLOW.to_string())
        } else {
            Ok(())
        }
    }

    /// Time spent waiting for the user does not count against the limit.
    fn extend(&self, waited: Duration) {
        *self.deadline.lock().unwrap() += waited;
    }

    /// Tells the UI what is happening now.
    pub fn progress(&self, text: &str) {
        emit(&self.app, AgentEvent::Progress { text: text.to_string() });
    }

    fn child(&self) -> Ctx {
        let mut child = self.clone();
        child.depth += 1;
        child
    }

    pub fn tainted(&self) -> bool {
        self.tainted.load(Ordering::SeqCst)
    }

    pub fn writes(&self) -> usize {
        self.writes.load(Ordering::SeqCst)
    }

    pub fn clear_log(&self) {
        self.log.lock().unwrap().clear();
    }

    pub fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock().unwrap())
    }

    fn saved_facts(&self) -> Vec<String> {
        self.saved.lock().unwrap().clone()
    }
}

fn truncate(mut text: String) -> String {
    if text.chars().count() > MAX_TOOL_RESULT_CHARS {
        text = text.chars().take(MAX_TOOL_RESULT_CHARS).collect();
        text.push_str("\n[truncated]");
    }
    text
}

/// Empty arguments mean "none"; otherwise they must be a JSON object.
fn arguments_are_valid(args: &str) -> bool {
    let trimmed = args.trim();
    trimmed.is_empty() || matches!(serde_json::from_str::<Value>(trimmed), Ok(Value::Object(_)))
}

// ---------------- the agent loop ----------------

/// Runs one agent until it gives a plain reply and returns that text.
pub fn run_agent<'a>(
    ctx: &'a Ctx,
    agent: AgentId,
    history: Vec<ChatMessage>,
) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
    Box::pin(async move {
        let memory_enabled = ctx.app.state::<MemoryState>().is_configured().await;
        let web_enabled = ctx.app.state::<WebState>().key().is_some();
        let system = agents::system_prompt(agent, memory_enabled, web_enabled);
        let mut messages = history;
        let mut empty_retries = 0;

        for _ in 0..MAX_ITERATIONS {
            ctx.check()?;

            let defs = agents::tool_defs(&ctx.app, agent, memory_enabled);
            let mut request: Vec<ChatMessage> = vec![ChatMessage::text("system", system.clone())];
            request.extend(messages.iter().cloned());
            let mut body = json!({
                "model": ctx.models.main,
                "messages": request,
                "temperature": 0.7,
                "max_tokens": MAX_OUTPUT_TOKENS,
            });
            if !defs.is_empty() {
                body["tools"] = Value::Array(defs);
            }

            let reply = post_chat(&ctx.api_key, &body).await?;
            ctx.check()?;

            let calls = reply.tool_calls.clone().unwrap_or_default();
            let text = reply.content.clone().unwrap_or_default();
            messages.push(reply);

            if calls.is_empty() {
                if text.trim().is_empty() {
                    // An empty reply usually means the model had nothing to
                    // call and nothing to say. Nudge once, then answer
                    // honestly rather than showing an error.
                    messages.pop();
                    if empty_retries == 0 {
                        empty_retries += 1;
                        messages.push(ChatMessage::text(
                            "user",
                            "(Your last reply was empty. Reply in plain text, or use a tool if one fits. If you have no tool for this, say so.)",
                        ));
                        continue;
                    }
                    return Ok("I couldn't work out how to do that with the tools I have.".to_string());
                }
                return Ok(text);
            }

            for call in calls {
                ctx.check()?;
                let content = execute_call(ctx, agent, &call).await?;
                messages.push(ChatMessage::tool_result(&call.id, content));
            }
        }

        Err("Stopped after too many steps without finishing.".to_string())
    })
}

/// Runs one tool call for `agent` and returns the text to hand back to the
/// model. Failures become text so the model can recover; the only `Err` is
/// cancellation.
async fn execute_call(ctx: &Ctx, agent: AgentId, call: &ToolCall) -> Result<String, String> {
    let name = call.function.name.as_str();

    let result: Result<String, String> = if !agent.allows(name) {
        Err(format!("Tool not available: {name}"))
    } else if !arguments_are_valid(&call.function.arguments) {
        // Checked before anything is shown to the user, so a cut-off call
        // never reaches an approval card.
        Err(BAD_ARGUMENTS.to_string())
    } else if name == "ask_user" {
        let waiting_since = Instant::now();
        let answer = interact::ask_user(&ctx.app, &ctx.turn, ctx.my_turn, call).await;
        ctx.extend(waiting_since.elapsed());
        answer
    } else if let Some(target) = agents::delegation_target(name) {
        delegate(ctx, target, &call.function.arguments).await
    } else {
        run_tool(ctx, call).await
    };

    if matches!(&result, Err(e) if e == "cancelled") {
        return Err("cancelled".to_string());
    }
    Ok(match result {
        Ok(text) => truncate(text),
        Err(err) => format!("Error: {err}"),
    })
}

/// Hands a job to another agent and returns its final answer.
async fn delegate(ctx: &Ctx, target: AgentId, args: &str) -> Result<String, String> {
    if ctx.depth >= MAX_DEPTH {
        return Err("Jobs are nested too deeply.".to_string());
    }
    let task = serde_json::from_str::<Value>(args)
        .ok()
        .and_then(|v| v["task"].as_str().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Missing `task`.".to_string())?;

    let child = ctx.child();
    run_agent(&child, target, vec![ChatMessage::text("user", task)]).await
}

/// An ordinary tool: approval card if it changes something, then run it.
async fn run_tool(ctx: &Ctx, call: &ToolCall) -> Result<String, String> {
    let app = &ctx.app;
    let name = call.function.name.as_str();
    let args = call.function.arguments.as_str();

    // Anything that changes something needs an approval card: MCP tools not
    // marked read-only, and (once untrusted content has been read) the
    // built-in side-effect tools.
    let needs_approval = app.state::<crate::mcp::McpManager>().requires_approval(name)
        || (ctx.tainted() && tools::is_side_effect(name));

    if needs_approval {
        let waiting_since = Instant::now();
        let verdict = interact::approve(app, &ctx.turn, ctx.my_turn, call).await;
        ctx.extend(waiting_since.elapsed());
        match verdict? {
            Approval::Allowed => {}
            Approval::Declined(message) => return Err(message),
        }
    }

    emit(app, tool_start(call));
    let outcome = tools::execute(app, name, args).await;

    if tools::returns_untrusted_content(app, name) {
        ctx.tainted.store(true, Ordering::SeqCst);
    }
    if needs_approval && outcome.is_ok() {
        ctx.writes.fetch_add(1, Ordering::SeqCst);
    }
    if name == "remember" && outcome.is_ok() {
        if let Some(fact) = serde_json::from_str::<Value>(args)
            .ok()
            .and_then(|v| v["fact"].as_str().map(str::to_string))
        {
            ctx.saved.lock().unwrap().push(fact);
        }
    }

    let title = tools::describe(name, args);
    let line = match &outcome {
        Ok(_) => format!("{title}: done"),
        Err(err) => format!("{title}: failed ({err})"),
    };
    ctx.log.lock().unwrap().push(line.chars().take(MAX_LOG_LINE).collect());

    emit(
        app,
        AgentEvent::ToolResult {
            id: call.id.clone(),
            name: name.to_string(),
            ok: outcome.is_ok(),
            error: outcome.as_ref().err().cloned(),
        },
    );
    outcome
}

// ---------------- entry point ----------------

fn non_empty(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Keeps the history bounded and starting at a user message.
fn trim_history(history: &mut Vec<ChatMessage>) {
    while history.len() > MAX_HISTORY_MESSAGES || history.first().is_some_and(|m| m.role != "user") {
        if history.is_empty() {
            break;
        }
        history.remove(0);
    }
}

#[tauri::command]
pub async fn agent_run(
    app: AppHandle,
    state: State<'_, AgentState>,
    api_key: String,
    model: String,
    planner_model: Option<String>,
    verifier_model: Option<String>,
    text: String,
) -> Result<String, String> {
    run_request(app, state.inner(), api_key, model, planner_model, verifier_model, text).await
}

/// Handles one user message from start to finish (see the module docs).
pub async fn run_request(
    app: AppHandle,
    state: &AgentState,
    api_key: String,
    model: String,
    planner_model: Option<String>,
    verifier_model: Option<String>,
    text: String,
) -> Result<String, String> {
    let main = model.trim().to_string();
    if main.is_empty() {
        return Err("Set an OpenRouter model name in settings first.".to_string());
    }
    let models = Models {
        planner: non_empty(planner_model).unwrap_or_else(|| main.clone()),
        verifier: non_empty(verifier_model).unwrap_or_else(|| main.clone()),
        main,
    };

    let my_turn = state.turn.fetch_add(1, Ordering::SeqCst) + 1;
    let ctx = Ctx {
        app: app.clone(),
        api_key: api_key.clone(),
        models: models.clone(),
        turn: state.turn.clone(),
        my_turn,
        depth: 0,
        tainted: Arc::new(AtomicBool::new(false)),
        writes: Arc::new(AtomicUsize::new(0)),
        log: Arc::new(Mutex::new(Vec::new())),
        saved: Arc::new(Mutex::new(Vec::new())),
        deadline: Arc::new(Mutex::new(Instant::now() + JOB_LIMIT)),
    };

    let mut messages = state.history.lock().unwrap().clone();
    messages.push(ChatMessage::text("user", text.clone()));

    let route = crate::orchestrator::route(&ctx, &messages).await?;
    let reply = match route {
        crate::orchestrator::Route::Utility => run_agent(&ctx, AgentId::Utility, messages.clone()).await?,
        crate::orchestrator::Route::Google => run_agent(&ctx, AgentId::Google, messages.clone()).await?,
        crate::orchestrator::Route::Plan => crate::planner::run_job(&ctx, &messages).await?,
    };

    // After replying, the agent reviews the exchange and decides what to
    // commit to long-term memory. It runs in the background so the user isn't
    // kept waiting, and is told what was already saved.
    if app.state::<MemoryState>().is_configured().await {
        tauri::async_runtime::spawn(crate::reflection::run(
            app.clone(),
            api_key,
            models.main.clone(),
            text,
            reply.clone(),
            ctx.saved_facts(),
            ctx.tainted(),
        ));
    }

    // Only a run that is still current commits to the history, and only when
    // it succeeded, so a failed or superseded request leaves nothing behind.
    if state.turn.load(Ordering::SeqCst) == my_turn {
        messages.push(ChatMessage::text("assistant", reply.clone()));
        trim_history(&mut messages);
        *state.history.lock().unwrap() = messages;
    }
    Ok(reply)
}

#[tauri::command]
pub fn agent_cancel(state: State<'_, AgentState>) {
    state.turn.fetch_add(1, Ordering::SeqCst);
}

#[tauri::command]
pub fn agent_reset(state: State<'_, AgentState>) {
    state.turn.fetch_add(1, Ordering::SeqCst);
    state.history.lock().unwrap().clear();
}

#[cfg(test)]
mod tests {
    use super::arguments_are_valid;

    #[test]
    fn rejects_cut_off_arguments() {
        assert!(arguments_are_valid(""));
        assert!(arguments_are_valid("  "));
        assert!(arguments_are_valid(r#"{"title":"Notes"}"#));
        // A long document cut off mid-string, as in a truncated reply.
        assert!(!arguments_are_valid(r#"{"title":"MPI notes","content":"MPI_Send is blo"#));
        // Valid JSON, but not an object.
        assert!(!arguments_are_valid(r#""just a string""#));
        assert!(!arguments_are_valid("[1,2]"));
    }
}

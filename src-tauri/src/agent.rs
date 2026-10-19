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

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::agents::{self, AgentId};
use crate::history::{self, Db, StoredMessage};
use crate::interact::{self, Approval, AskOption};
use crate::memory::MemoryState;
use crate::openrouter::{post_chat, post_chat_stream};
use crate::tools;
use crate::types::{ChatMessage, ToolCall};
use crate::web::WebState;

const MAX_ITERATIONS: usize = 8;
const MAX_DEPTH: usize = 3;
const MAX_TOOL_RESULT_CHARS: usize = 8000;
const MAX_HISTORY_MESSAGES: usize = 40;
/// Web lookups allowed for one request (all agents and steps together). Past
/// these the tool refuses and the model has to answer with what it has; a
/// prompt alone does not stop a small model from searching on and on.
const MAX_SEARCHES: usize = 5;
const MAX_PAGE_READS: usize = 4;
const MAX_LOG_LINE: usize = 240;
const MAX_STEPS_SUMMARY_LINE: usize = 160;
const MAX_STEPS_SUMMARY_LINES: usize = 30;
/// Room for a long tool call (e.g. a whole document). If a reply is cut off
/// mid-call the arguments are broken JSON, so this must not be too small.
const MAX_OUTPUT_TOKENS: u32 = 4096;

/// One request may run this long, not counting time spent waiting for the user
/// to answer a card. Past it, Leo stops rather than spin forever.
const JOB_LIMIT: Duration = Duration::from_secs(600);
const TOO_SLOW: &str = "That is taking too long, so I stopped. Try again, or choose a faster model in settings.";

const UNFINISHED_TEXT: &str = "The text you sent ends with an ellipsis, so it looks unfinished or cut short. Nothing was done. Write the complete text of this part, with no placeholders or ellipsis, and send it again. If the document needs more sections, add each one in its own following call until the whole document is written.";

/// A tool that keeps failing is not tried a fourth time in one request.
const MAX_TOOL_FAILURES: usize = 3;
/// A tool the user declined twice is not offered again; after this many
/// declines of anything, every approval is stopped.
const MAX_SAME_DECLINES: usize = 2;
const MAX_TOTAL_DECLINES: usize = 3;

const BAD_ARGUMENTS: &str = "Your tool call could not be read: its arguments were not valid JSON, most likely because the message was cut off for being too long. Nothing was done. Try again with less text in the call. For a long document, create it with the first part, then add the rest in several smaller edits.";

#[derive(Default)]
pub struct AgentState {
    /// The conversation so far: the user's messages and Leo's final replies.
    history: Mutex<Vec<ChatMessage>>,
    /// Notes on everything older than `history` (see summary.rs).
    summary: Mutex<String>,
    /// Held while a summary update runs, so the next request waits for it.
    folding: Arc<tokio::sync::Mutex<()>>,
    /// The saved chat this conversation belongs to (see history.rs), created
    /// with the first message. `None` until then, or if saving is unavailable.
    conv: Mutex<Option<i64>>,
    /// How many of the chat's oldest saved messages the summary covers.
    folded: Mutex<usize>,
    /// Bumped by "New chat"; an update started before it is thrown away.
    epoch: AtomicU64,
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
    /// A piece of the reply as the model writes it (only for replies that go
    /// straight to the user).
    Delta { text: String },
    /// A line only the voice speaks, never shown: said when a tool starts and the
    /// model did not say anything itself ("Sure, I'm checking the weather for
    /// Bengaluru."). The tool row on screen already says the same in writing.
    Narrate { text: String },
    /// A short line Leo says before a step of a plan ("Let me check your
    /// calendar first."), shown and spoken as a message of its own.
    Say { text: String },
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
    /// The saved chat this request belongs to, if any.
    chat: Option<i64>,
    /// Stream the reply to the UI as it is written. Only for a single agent
    /// answering the user directly, never for plan steps or sub-agents.
    pub stream: bool,
    /// Set once any tool has returned text written by someone else.
    tainted: Arc<AtomicBool>,
    /// Number of approved, state-changing actions carried out so far.
    writes: Arc<AtomicUsize>,
    /// Every tool call made so far in this request, in plain words. Unlike
    /// `log` it is not cleared between plan steps, so later steps can be told
    /// what has already been done and not repeat it.
    steps_summary: Arc<Mutex<Vec<String>>>,
    /// Web searches and page reads made so far, against the limits above.
    searches: Arc<AtomicUsize>,
    page_reads: Arc<AtomicUsize>,
    /// Plain-language record of tool calls, read by the verifier.
    log: Arc<Mutex<Vec<String>>>,
    /// Facts saved to memory during this request.
    saved: Arc<Mutex<Vec<String>>>,
    /// Google documents the user has already approved writing to in this
    /// request (created or first edited), so follow-up edits to the same
    /// document do not ask again.
    docs_approved: Arc<Mutex<HashSet<String>>>,
    /// Documents created in this request: lowercase title -> document id. A
    /// second create with the same title is refused (the agent edits the first
    /// instead of making copies).
    docs_created: Arc<Mutex<HashMap<String, String>>>,
    /// Tools the user declined in this request (one entry per decline), shared
    /// by every agent, so a "no" is remembered instead of tried another way.
    declines: Arc<Mutex<Vec<String>>>,
    /// How many times each tool has failed in this request.
    failures: Arc<Mutex<HashMap<String, usize>>>,
    /// When the request gives up; moved back while waiting for the user.
    deadline: Arc<Mutex<Instant>>,
}

impl Ctx {
    /// What has already been done in this request, for the next step to read.
    pub fn steps_summary_text(&self) -> String {
        self.steps_summary.lock().unwrap().iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n")
    }

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

    /// True for an edit to a document the user already approved writing to
    /// during this request: the edits that follow one approved document are
    /// one job, so they must not ask again and again.
    fn doc_already_approved(&self, name: &str, args: &str) -> bool {
        doc_id_for_edit(name, args).is_some_and(|id| self.docs_approved.lock().unwrap().contains(&id))
    }

    /// After a Docs write succeeded, remembers which document it was, so
    /// further edits to it go through without a new card.
    fn remember_doc(&self, name: &str, args: &str, result: &str) {
        let id = if is_doc_edit(name) {
            doc_id_for_edit(name, args)
        } else if is_doc_create(name) {
            doc_id_in(result)
        } else {
            None
        };
        if let Some(id) = id {
            if is_doc_create(name) {
                if let Some(title) = doc_title(args) {
                    self.docs_created.lock().unwrap().insert(title, id.clone());
                }
            }
            self.docs_approved.lock().unwrap().insert(id);
        }
    }

    /// The id of a document with this title already created in this request,
    /// when `name` is about to create it again.
    fn duplicate_doc(&self, name: &str, args: &str) -> Option<String> {
        if !is_doc_create(name) {
            return None;
        }
        self.docs_created.lock().unwrap().get(&doc_title(args)?).cloned()
    }

    fn note_decline(&self, tool: &str) {
        self.declines.lock().unwrap().push(tool.to_string());
    }

    fn declined_block(&self, tool: &str) -> Option<String> {
        decline_block(&self.declines.lock().unwrap(), tool)
    }

    fn too_many_failures(&self, tool: &str) -> bool {
        self.failures.lock().unwrap().get(tool).copied().unwrap_or(0) >= MAX_TOOL_FAILURES
    }

    fn note_failure(&self, tool: &str) {
        *self.failures.lock().unwrap().entry(tool.to_string()).or_insert(0) += 1;
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

/// A Google Docs write whose text ends in an ellipsis: the model stopped in
/// the middle of its own document ("... the European ...") and would leave it
/// unfinished.
fn writes_unfinished_text(name: &str, args: &str) -> bool {
    if !is_doc_edit(name) && !is_doc_create(name) {
        return false;
    }
    let Ok(v) = serde_json::from_str::<Value>(args) else {
        return false;
    };
    ["content", "text"].iter().any(|k| {
        v.get(*k)
            .and_then(Value::as_str)
            .map(|t| {
                let t = t.trim_end();
                t.ends_with('\u{2026}') || t.ends_with("...")
            })
            .unwrap_or(false)
    })
}

/// Why a tool that needs approval must not even be offered again, given what
/// the user has already declined in this request.
fn decline_block(declines: &[String], name: &str) -> Option<String> {
    let same = declines.iter().filter(|d| d.as_str() == name).count();
    if same >= MAX_SAME_DECLINES {
        return Some("The user has already declined this action more than once in this request. Do not ask again, and do not look for another way to do the same thing: tell them you did not do it.".to_string());
    }
    if declines.len() >= MAX_TOTAL_DECLINES {
        return Some("The user has declined several actions in this request. Stop trying: do not look for another way to do what they declined. Say briefly what you did not do, and ask what they would like instead.".to_string());
    }
    None
}

fn is_doc_edit(name: &str) -> bool {
    matches!(
        name,
        "google__modify_doc_text" | "google__update_paragraph_style" | "google__insert_doc_elements"
    )
}

fn is_doc_create(name: &str) -> bool {
    matches!(name, "google__create_doc" | "google__import_to_google_doc")
}

/// Lowercase title a create call asks for (`file_name` or `title`).
fn doc_title(args: &str) -> Option<String> {
    let v: Value = serde_json::from_str(args).ok()?;
    let t = v.get("file_name").or_else(|| v.get("title"))?.as_str()?.trim().to_lowercase();
    if t.is_empty() { None } else { Some(t) }
}

fn doc_id_for_edit(name: &str, args: &str) -> Option<String> {
    if !is_doc_edit(name) {
        return None;
    }
    serde_json::from_str::<Value>(args)
        .ok()?
        .get("document_id")?
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The id of a document in a tool result: from its `/document/d/<id>` link,
/// or an `ID: <id>` line.
fn doc_id_in(result: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?:/d/|\bID:?\s+)([A-Za-z0-9_-]{20,})").ok()?;
    re.captures(result).map(|c| c[1].to_string())
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
        // The date is told up front (by day, so the prompt stays cacheable)
        // and the agents do not need to look it up.
        let mut system = agents::system_prompt(agent, memory_enabled, web_enabled);
        // The general assistant has no Google tools, so when Google is not
        // usable it must say why instead of inventing a reason.
        if matches!(agent, AgentId::Utility | AgentId::Google) {
            if let Some(note) = ctx.app.state::<crate::mcp::McpManager>().google_note() {
                system.push_str("\n\n");
                system.push_str(&note);
            }
        }
        let mut messages = history;
        let mut empty_retries = 0;
        // A widget block from a tool result, kept so the reply
        // always carries it even if the model forgets to paste it.
        let mut widget_block: Option<String> = None;

        for _ in 0..MAX_ITERATIONS {
            ctx.check()?;

            let defs = agents::tool_defs(&ctx.app, agent, memory_enabled);
            // What the session already knows (date, location) is re-read every
            // round, so a lookup by any agent shows up in the others at once.
            let session_note = ctx.app.state::<crate::session::SessionInfo>().prompt_note();
            let mut request: Vec<ChatMessage> =
                vec![ChatMessage::text("system", format!("{system}\n\n{session_note}"))];
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
            crate::openrouter::cache_system_prompt(&mut body);

            let reply = if ctx.stream && ctx.depth == 0 {
                let app = ctx.app.clone();
                let mut on_delta = move |text: &str| emit(&app, AgentEvent::Delta { text: text.to_string() });
                // Text written before a tool call is kept: it is Leo telling the
                // user what it is about to do, and stays as a message of its own.
                post_chat_stream(&ctx.api_key, &body, &mut on_delta).await?
            } else {
                post_chat(&ctx.api_key, &body).await?
            };
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
                return Ok(crate::widget::with_block(widget_block, text));
            }

            // The model often goes straight to a tool without a word. For the
            // voice, say what is happening anyway (the model's own lead-in, when
            // there is one, is spoken as it streams).
            if ctx.stream && ctx.depth == 0 && text.trim().is_empty() {
                let mcp = ctx.app.state::<crate::mcp::McpManager>();
                let line = calls
                    .iter()
                    .filter(|c| !mcp.requires_approval(&c.function.name))
                    .find_map(|c| narration_line(&c.function.name, &c.function.arguments, &c.id));
                if let Some(text) = line {
                    emit(&ctx.app, AgentEvent::Narrate { text });
                }
            }

            for call in calls {
                ctx.check()?;
                let content = execute_call(ctx, agent, &call).await?;
                if let Some(block) = crate::widget::extract_block(&content) {
                    widget_block = Some(block);
                }
                messages.push(ChatMessage::tool_result(&call.id, content));
            }
        }

        Err("Stopped after too many steps without finishing.".to_string())
    })
}

/// What Leo says aloud as a tool starts, built from the tool's own plain-language
/// title: "Sure, I'm searching the web for latest news." Quick background
/// lookups, questions and approvals are not announced.
fn narration_line(name: &str, args: &str, call_id: &str) -> Option<String> {
    if matches!(name, "current_datetime" | "get_location" | "recall_memory" | "remember" | "ask_user" | "set_calendar_auto_approve") {
        return None;
    }
    let p = tools::present(name, args);
    let mut title = p.title.trim().trim_end_matches(['\u{2026}', '.']).to_string();
    let first = title.chars().next()?;
    title = format!("{}{}", first.to_lowercase(), &title[first.len_utf8()..]);
    // Only a short subject is worth reading out (a place, a short search).
    let subject = match name {
        "web_search" | "get_weather" => p.detail.filter(|d| d.chars().count() <= 40).map(|d| format!(" for {d}")),
        _ => None,
    };
    let openers = ["Sure, I'm", "One moment, I'm", "Okay, I'm"];
    let opener = openers[call_id.bytes().map(usize::from).sum::<usize>() % openers.len()];
    Some(format!("{opener} {title}{}.", subject.unwrap_or_default()))
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
    } else if writes_unfinished_text(name, &call.function.arguments) {
        // Checked before an approval card, like cut-off arguments.
        Err(UNFINISHED_TEXT.to_string())
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

    // Refused before anything is shown, so no tool row appears for it.
    if ctx.too_many_failures(name) {
        return Err(format!(
            "This action has already failed {MAX_TOOL_FAILURES} times in this request. Do not try it again or look for a workaround: tell the user plainly what went wrong."
        ));
    }
    let web_use = match name {
        "web_search" => Some((&ctx.searches, MAX_SEARCHES, "searches")),
        "fetch_page" => Some((&ctx.page_reads, MAX_PAGE_READS, "page reads")),
        _ => None,
    };
    if let Some((counter, limit, what)) = web_use {
        if counter.fetch_add(1, Ordering::SeqCst) >= limit {
            return Err(format!(
                "The limit of {limit} web {what} for this request has been reached. Do not try again: answer now with what you already found, and say plainly what you could not find."
            ));
        }
    }

    // A document with this title was already made in this request: edit it
    // instead of creating a copy (and asking for approval again).
    if let Some(id) = ctx.duplicate_doc(name, args) {
        return Err(format!(
            "A document with this title was already created in this request (document id {id}). Do not create another: it is done. Reply with its link, or change it with modify_doc_text using that id."
        ));
    }

    // Anything that changes something needs an approval card: MCP tools not
    // marked read-only, and (once untrusted content has been read) the
    // built-in side-effect tools.
    let needs_approval = (app.state::<crate::mcp::McpManager>().requires_approval(name)
        || (ctx.tainted() && tools::is_side_effect(name))
        // Changing a permission always asks, so the model cannot relax its own limits.
        || name == "set_calendar_auto_approve")
        && !ctx.doc_already_approved(name, args)
        && !crate::prefs::calendar_write_skips_approval(app, name, args);

    if needs_approval {
        // A "no" is remembered for the rest of the request.
        if let Some(stop) = ctx.declined_block(name) {
            return Err(stop);
        }
        let waiting_since = Instant::now();
        let verdict = interact::approve(app, &ctx.turn, ctx.my_turn, call).await;
        ctx.extend(waiting_since.elapsed());
        match verdict? {
            Approval::Allowed => {}
            Approval::Declined(message) => {
                ctx.note_decline(name);
                return Err(message);
            }
        }
    }

    emit(app, tool_start(call));
    let outcome = if name == "look_at_screen" {
        crate::vision::look(ctx, args).await
    } else {
        tools::execute(app, name, args).await
    };
    if matches!(&outcome, Err(e) if e == "cancelled") {
        return Err("cancelled".to_string());
    }

    if tools::returns_untrusted_content(app, name) {
        ctx.tainted.store(true, Ordering::SeqCst);
    }
    if needs_approval && outcome.is_ok() {
        ctx.writes.fetch_add(1, Ordering::SeqCst);
    }
    if let Ok(result) = &outcome {
        ctx.remember_doc(name, args, result);
    }
    if name == "remember" && outcome.is_ok() {
        if let Some(fact) = serde_json::from_str::<Value>(args)
            .ok()
            .and_then(|v| v["fact"].as_str().map(str::to_string))
        {
            ctx.saved.lock().unwrap().push(fact);
        }
    }

    if matches!(&outcome, Err(e) if e != "cancelled") {
        ctx.note_failure(name);
    }

    let title = tools::describe(name, args);
    let line = match &outcome {
        Ok(_) => format!("{title}: done"),
        Err(err) => format!("{title}: failed ({err})"),
    };
    ctx.log.lock().unwrap().push(line.chars().take(MAX_LOG_LINE).collect());
    {
        let snapshot = {
            let mut steps_summary = ctx.steps_summary.lock().unwrap();
            steps_summary.push(line.chars().take(MAX_STEPS_SUMMARY_LINE).collect());
            if steps_summary.len() > MAX_STEPS_SUMMARY_LINES {
                steps_summary.remove(0);
            }
            steps_summary.clone()
        };
        // Written to the chat row the moment the line is created.
        if let Some(chat) = ctx.chat {
            if let Err(err) = app.state::<Db>().set_steps(app, chat, &snapshot) {
                eprintln!("could not save the steps summary: {err}");
            }
        }
    }

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
    images: Option<Vec<String>>,
) -> Result<String, String> {
    run_request(app, state.inner(), api_key, model, planner_model, verifier_model, text, images).await
}

const MAX_IMAGES: usize = 6;
const MAX_IMAGE_CHARS: usize = 8_000_000;
/// Images older than the newest few are dropped from the conversation the
/// model is sent each turn, so a long chat does not carry every picture along.
const KEEP_IMAGE_MESSAGES: usize = 2;

/// Keeps only well-formed image data URLs, within the size limits.
fn clean_images(images: Option<Vec<String>>) -> Result<Vec<String>, String> {
    let images = images.unwrap_or_default();
    if images.len() > MAX_IMAGES {
        return Err(format!("You can attach up to {MAX_IMAGES} images at a time."));
    }
    for url in &images {
        if !url.starts_with("data:image/") || !url.contains(";base64,") {
            return Err("One of the attachments is not a valid image.".to_string());
        }
        if url.len() > MAX_IMAGE_CHARS {
            return Err("One of the images is too large to send.".to_string());
        }
    }
    Ok(images)
}

/// Removes the images from all but the newest `keep` messages that have any.
fn drop_old_images(history: &mut [ChatMessage], keep: usize) {
    let mut seen = 0;
    for message in history.iter_mut().rev() {
        if message.images.is_empty() {
            continue;
        }
        seen += 1;
        if seen > keep {
            message.images.clear();
            let note = "(an image was attached here earlier)";
            message.content = Some(match message.content.take() {
                Some(text) if !text.is_empty() => format!("{text}\n{note}"),
                _ => note.to_string(),
            });
        }
    }
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
    images: Option<Vec<String>>,
) -> Result<String, String> {
    let images = clean_images(images)?;
    // An image sent without words still needs something readable in the chat's record.
    let text = if text.trim().is_empty() && !images.is_empty() { "(sent an image)".to_string() } else { text };
    let main = model.trim().to_string();
    let api_key_for_summary = api_key.clone();
    if main.is_empty() {
        return Err("Set an OpenRouter model name in settings first.".to_string());
    }
    let models = Models {
        planner: non_empty(planner_model).unwrap_or_else(|| main.clone()),
        verifier: non_empty(verifier_model).unwrap_or_else(|| main.clone()),
        main,
    };

    let my_turn = state.turn.fetch_add(1, Ordering::SeqCst) + 1;
    let user_text = match images.len() {
        0 => text.clone(),
        n => format!("{text}\n\n(attached {n} image{})", if n == 1 { "" } else { "s" }),
    };
    // The agents should know the saved profile even before the screen asks.
    crate::profile::ensure_loaded(&app);
    let (chat, new_chat) = ensure_chat(&app, state, &text);
    if let (Some(id), true) = (chat, new_chat) {
        // The model names the chat from its first message, in the background.
        tauri::async_runtime::spawn(crate::history::name_chat(
            app.clone(),
            api_key.clone(),
            models.main.clone(),
            id,
            text.clone(),
        ));
    }
    let ctx = Ctx {
        app: app.clone(),
        api_key: api_key.clone(),
        models: models.clone(),
        turn: state.turn.clone(),
        my_turn,
        depth: 0,
        chat,
        stream: false,
        tainted: Arc::new(AtomicBool::new(false)),
        writes: Arc::new(AtomicUsize::new(0)),
        // The chat's earlier steps summary is read back from the database,
        // so a reopened chat carries on with the notes it already had.
        steps_summary: Arc::new(Mutex::new(
            chat.map(|c| app.state::<Db>().steps(&app, c)).unwrap_or_default(),
        )),
        searches: Arc::new(AtomicUsize::new(0)),
        page_reads: Arc::new(AtomicUsize::new(0)),
        log: Arc::new(Mutex::new(Vec::new())),
        saved: Arc::new(Mutex::new(Vec::new())),
        docs_approved: Arc::new(Mutex::new(HashSet::new())),
        docs_created: Arc::new(Mutex::new(HashMap::new())),
        declines: Arc::new(Mutex::new(Vec::new())),
        failures: Arc::new(Mutex::new(HashMap::new())),
        deadline: Arc::new(Mutex::new(Instant::now() + JOB_LIMIT)),
    };

    // A summary update from the last reply may still be running.
    drop(state.folding.lock().await);

    let mut messages = state.history.lock().unwrap().clone();
    messages.push(ChatMessage { images, ..ChatMessage::text("user", text.clone()) });
    drop_old_images(&mut messages, KEEP_IMAGE_MESSAGES);
    // What the agents read: the summary of older turns, then the recent messages.
    let prompt = crate::summary::with_summary(&state.summary.lock().unwrap().clone(), &messages);

    let route = crate::orchestrator::route(&ctx, &messages).await?;
    let ctx = Ctx { stream: !matches!(route, crate::orchestrator::Route::Plan), ..ctx };
    let reply = match route {
        crate::orchestrator::Route::Utility => run_agent(&ctx, AgentId::Utility, prompt.clone()).await?,
        crate::orchestrator::Route::Google => run_agent(&ctx, AgentId::Google, prompt.clone()).await?,
        crate::orchestrator::Route::Plan => crate::planner::run_job(&ctx, &prompt).await?,
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
        if let Some(chat) = chat {
            if let Err(err) = app.state::<Db>().add_messages(&app, chat, &[("user", &user_text), ("assistant", &reply)]) {
                eprintln!("could not save the chat: {err}");
            }
        }
        messages.push(ChatMessage::text("assistant", reply.clone()));
        trim_history(&mut messages);
        *state.history.lock().unwrap() = messages;
        start_summary_update(&app, state, &api_key_for_summary, &models.main, chat).await;
    }
    Ok(reply)
}

/// Once the chat is long enough, folds its oldest exchange into the summary
/// in the background (see summary.rs). A failure just leaves the messages in
/// place; the next reply tries again with more to fold.
async fn start_summary_update(app: &AppHandle, state: &AgentState, api_key: &str, model: &str, chat: Option<i64>) {
    let (old, current) = {
        let history = state.history.lock().unwrap();
        let current = state.summary.lock().unwrap().clone();
        let n = crate::summary::fold_count(history.len(), !current.is_empty());
        if n == 0 {
            return;
        }
        (history[..n].to_vec(), current)
    };
    let epoch = state.epoch.load(Ordering::SeqCst);
    let guard = state.folding.clone().lock_owned().await;
    let (app, api_key, model) = (app.clone(), api_key.to_string(), model.to_string());
    tauri::async_runtime::spawn(async move {
        let result = crate::summary::update(&api_key, &model, &current, &old).await;
        let state = app.state::<AgentState>();
        match result {
            Ok(text) if state.epoch.load(Ordering::SeqCst) == epoch => {
                let mut history = state.history.lock().unwrap();
                if history.len() >= old.len() {
                    history.drain(..old.len());
                    *state.summary.lock().unwrap() = text.clone();
                    let folded = {
                        let mut folded = state.folded.lock().unwrap();
                        *folded += old.len();
                        *folded
                    };
                    // Written to the chat row the moment the summary is created.
                    if let Some(chat) = chat {
                        if let Err(err) = app.state::<Db>().set_summary(&app, chat, &text, folded) {
                            eprintln!("could not save the chat summary: {err}");
                        }
                    }
                }
            }
            Ok(_) => {}
            Err(err) => eprintln!("summary update failed: {err}"),
        }
        drop(guard);
    });
}

#[tauri::command]
pub fn agent_cancel(state: State<'_, AgentState>) {
    state.turn.fetch_add(1, Ordering::SeqCst);
}

#[tauri::command]
pub fn agent_reset(state: State<'_, AgentState>) {
    state.turn.fetch_add(1, Ordering::SeqCst);
    state.epoch.fetch_add(1, Ordering::SeqCst);
    state.history.lock().unwrap().clear();
    state.summary.lock().unwrap().clear();
    *state.conv.lock().unwrap() = None;
    *state.folded.lock().unwrap() = 0;
}

/// The saved chat for this conversation, created with its first message.
/// Also says whether the chat was just created (then it still needs a name).
fn ensure_chat(app: &AppHandle, state: &AgentState, first_message: &str) -> (Option<i64>, bool) {
    let mut conv = state.conv.lock().unwrap();
    let mut created = false;
    if conv.is_none() {
        match app.state::<Db>().create_chat(app, first_message) {
            Ok(id) => {
                *conv = Some(id);
                created = true;
            }
            Err(err) => eprintln!("could not start a saved chat: {err}"),
        }
    }
    (*conv, created)
}

/// The saved chat that is open now, for highlighting it in the sidebar.
#[tauri::command]
pub fn history_current(state: State<'_, AgentState>) -> Option<i64> {
    *state.conv.lock().unwrap()
}

/// Opens a saved chat: the assistant carries on with its messages, its context
/// summary and its steps summary exactly as they were saved. Returns the
/// messages for the screen.
#[tauri::command]
pub fn history_open(app: AppHandle, state: State<'_, AgentState>, db: State<'_, Db>, id: i64) -> Result<Vec<StoredMessage>, String> {
    let loaded = db.load(&app, id)?;
    state.turn.fetch_add(1, Ordering::SeqCst);
    state.epoch.fetch_add(1, Ordering::SeqCst);
    let mut live = history::live_history(&loaded.messages, loaded.folded);
    trim_history(&mut live);
    *state.history.lock().unwrap() = live;
    *state.summary.lock().unwrap() = loaded.summary;
    *state.folded.lock().unwrap() = loaded.folded;
    *state.conv.lock().unwrap() = Some(id);
    Ok(loaded.messages)
}

#[tauri::command]
pub fn history_delete(app: AppHandle, state: State<'_, AgentState>, db: State<'_, Db>, id: i64) -> Result<(), String> {
    db.delete(&app, id)?;
    let current = *state.conv.lock().unwrap() == Some(id);
    if current {
        agent_reset(state);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::arguments_are_valid;

    #[test]
    fn tools_are_announced_in_plain_words() {
        let line = super::narration_line("get_weather", r#"{"location":"Bengaluru"}"#, "a").unwrap();
        assert!(line.ends_with("checking the weather for Bengaluru."), "{line}");
        let line = super::narration_line("web_search", r#"{"query":"Elon Musk current roles"}"#, "b").unwrap();
        assert!(line.contains("searching the web for Elon Musk current roles"), "{line}");
        // A long query is not read out.
        let long = format!(r#"{{"query":"{}"}}"#, "x".repeat(80));
        assert!(!super::narration_line("web_search", &long, "c").unwrap().contains(" for "));
        // Background lookups and questions stay quiet.
        for quiet in ["current_datetime", "get_location", "recall_memory", "remember", "ask_user"] {
            assert!(super::narration_line(quiet, "{}", "d").is_none(), "{quiet}");
        }
    }

    #[test]
    fn docs_are_approved_once_per_document() {
        let id = "1AbCdEfGhIjKlMnOpQrStUvWxYz0123456789";
        let link = format!("Created. https://docs.google.com/document/d/{id}/edit");
        assert_eq!(super::doc_id_in(&link).as_deref(), Some(id));
        assert_eq!(super::doc_id_in("ID: 1AbCdEfGhIjKlMnOpQrStUvWxYz0123456789 done").as_deref(), Some(id));
        let args = format!(r#"{{"document_id":"{id}","start_index":1}}"#);
        assert_eq!(super::doc_id_for_edit("google__modify_doc_text", &args).as_deref(), Some(id));
        assert_eq!(super::doc_title(r#"{"file_name":"  Formula Cars "}"#).as_deref(), Some("formula cars"));
        // Only edit tools are covered; other tools are not.
        assert_eq!(super::doc_id_for_edit("google__create_doc", &args), None);
        assert_eq!(super::doc_id_for_edit("google__modify_doc_text", "{}"), None);
    }

    #[test]
    fn unfinished_document_text_is_refused() {
        let call = |t: &str| format!(r#"{{"document_id":"x","text":"{t}"}}"#);
        assert!(super::writes_unfinished_text("google__modify_doc_text", &call("built upon pre-war European \u{2026}")));
        assert!(super::writes_unfinished_text("google__modify_doc_text", &call("and then...")));
        assert!(!super::writes_unfinished_text("google__modify_doc_text", &call("A finished sentence.")));
        // Only document writes are checked.
        assert!(!super::writes_unfinished_text("web_search", &call("and then...")));
    }

    #[test]
    fn a_no_is_remembered() {
        let d = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert!(super::decline_block(&d(&[]), "remember").is_none());
        assert!(super::decline_block(&d(&["remember"]), "remember").is_none());
        // The same tool declined twice: not offered again.
        assert!(super::decline_block(&d(&["remember", "remember"]), "remember").is_some());
        // Declined once, another tool is still allowed.
        assert!(super::decline_block(&d(&["remember"]), "google__create_drive_file").is_none());
        // Three declines of anything stop every approval.
        assert!(super::decline_block(&d(&["remember", "google__create_drive_file", "open_app"]), "google__send_gmail_message").is_some());
    }

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

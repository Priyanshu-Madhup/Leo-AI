//! Debug-only timing run: sends one request through the real pipeline
//! (orchestrator, planner, agents, verifier, the real Google connection) and
//! prints a timestamped timeline, answering any cards automatically. There is
//! no window and no UI. It performs real actions with the real account, so it
//! only runs when asked for with environment variables, and it is compiled
//! out of release builds.
//!
//! ```text
//! cd src-tauri
//! LEO_E2E_REQUEST="send a short hello note to you@gmail.com" \
//! LEO_E2E_EMAIL=you@gmail.com OPENROUTER_KEY=... OPENROUTER_MODEL=... \
//! LEO_TRACE=1 cargo run
//! ```

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Listener, Manager};

use crate::agent::{self, AgentState};
use crate::interact::{self, InteractState};
use crate::mcp::{self, McpManager};

pub fn active() -> bool {
    std::env::var_os("LEO_E2E_REQUEST").is_some()
}

/// Starts the run in the background and exits the process when it is done.
pub fn run(handle: &AppHandle) {
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        let ok = timing_run(handle).await;
        std::process::exit(if ok { 0 } else { 1 });
    });
    // Safety net: never hang the terminal if the run itself gets stuck.
    tauri::async_runtime::spawn(async {
        tokio::time::sleep(Duration::from_secs(600)).await;
        println!("
==== gave up after 10 minutes ====");
        std::process::exit(2);
    });
}

async fn timing_run(handle: AppHandle) -> bool {
    let (Ok(key), Ok(model), Ok(email), Ok(request)) = (
        std::env::var("OPENROUTER_KEY"),
        std::env::var("OPENROUTER_MODEL"),
        std::env::var("LEO_E2E_EMAIL"),
        std::env::var("LEO_E2E_REQUEST"),
    ) else {
        println!("set OPENROUTER_KEY, OPENROUTER_MODEL, LEO_E2E_EMAIL and LEO_E2E_REQUEST");
        return false;
    };

    let t0 = Instant::now();
    let timeline: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    // Print every event with a timestamp, and answer the cards.
    {
        let handle = handle.clone();
        let timeline = timeline.clone();
        let email = email.clone();
        let listener = handle.clone();
        listener.listen("agent://event", move |event| {
            let v: Value = serde_json::from_str(event.payload()).unwrap_or(Value::Null);
            let kind = v["kind"].as_str().unwrap_or("?").to_string();
            let detail = match kind.as_str() {
                "progress" => v["text"].as_str().unwrap_or("").to_string(),
                "tool_start" => format!("{} {}", v["label"].as_str().unwrap_or(""), v["detail"].as_str().unwrap_or("")),
                "tool_result" => format!("ok={} {}", v["ok"], v["error"].as_str().unwrap_or("")),
                "ask_user" => format!("QUESTION: {} -> answering with the test email", v["question"].as_str().unwrap_or("")),
                "approval_request" => format!("APPROVAL for {} -> allowing", v["label"].as_str().unwrap_or("")),
                _ => String::new(),
            };
            let line = format!(
                "{:>6.1}s  {kind:<17} {}",
                t0.elapsed().as_secs_f32(),
                detail.chars().take(150).collect::<String>()
            );
            println!("{line}");
            timeline.lock().unwrap().push(line);

            let state = handle.state::<InteractState>();
            let id = v["id"].as_str().unwrap_or("");
            match kind.as_str() {
                "approval_request" => interact::answer(state.inner(), id, json!({ "allow": true })),
                "ask_user" => interact::answer(state.inner(), id, json!({ "text": email })),
                _ => {}
            }
        });
    }

    println!("request: {request}");
    mcp::start_all(&handle);
    let ready_by = Instant::now() + Duration::from_secs(240);
    while !handle.state::<McpManager>().is_ready("google") {
        if Instant::now() > ready_by {
            println!("the Google connection did not start in time");
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    println!("{:>6.1}s  Google connection ready", t0.elapsed().as_secs_f32());

    let started = Instant::now();
    let result = agent::run_request(
        handle.clone(),
        handle.state::<AgentState>().inner(),
        key,
        model,
        None,
        None,
        request,
    )
    .await;
    println!("\n==== finished in {:.1}s ====", started.elapsed().as_secs_f32());
    match &result {
        Ok(reply) => println!("reply: {reply}"),
        Err(err) => println!("ERROR: {err}"),
    }
    result.is_ok()
}

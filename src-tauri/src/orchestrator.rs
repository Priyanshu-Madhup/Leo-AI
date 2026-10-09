//! The orchestrator: decides how a request is handled.
//!
//! One short model call classifies the message into a route:
//!   - `utility`: the utility agent can just answer or do it (chat, advice,
//!     writing, web lookups, memory, opening an app or site);
//!   - `google`: one clear Google Workspace job with everything it needs in
//!     the message ("show my unread mail", "what's on my calendar tomorrow");
//!   - `plan`: several steps where a later step needs an earlier result, or
//!     Google work mixed with lookups ("email the invoice Priya sent to my
//!     accountant", "what's the weather", which first needs the user's city).

use tauri::Manager;

use crate::agent::Ctx;
use crate::mcp::McpManager;
use crate::openrouter::json_call;
use crate::types::ChatMessage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Utility,
    Google,
    Plan,
}

const ROUTER_PROMPT: &str = "You route requests for a personal assistant. Reply with only JSON: {\"route\": \"utility\" | \"google\" | \"plan\"}.\n\
- utility: it can be answered or done in one go with general knowledge, writing, a single web search or page read, the user's memory, the date/time, or opening an app or site. Chat, advice, explanations, and lookups with a clear answer that one search settles (\"who won the match last night\", \"what is the capital of Peru\").\n\
- google: a job that stays INSIDE the user's Google account (Gmail, Calendar, Contacts, Drive, Docs, Sheets, Slides), even if it takes several steps (find a file, read it, change it, save it), as long as the message says what to do. Examples: \"show my unread email\", \"what is on my calendar tomorrow\", \"edit my Festivals doc and make it more professional\", \"add a Total row to my budget sheet\".\n\
- plan: open-ended, broad, vague or time-sensitive research where the first move is to find out what is actually going on (\"latest news on X\", \"what is happening with Y\", a topic with several angles), so the plan can look first and then adjust; or a later step needs a fact an earlier step has to find; or something must be changed or sent after research; or several services are combined. Examples: \"email Priya the report\" (find her address, then send), \"what's the weather\" (needs the user's location from memory first), \"put the top 3 news stories in a new Google Doc\" (search, then create the doc).\n\
A question that one search can settle is utility. Broad, vague or news-style research is plan. Judge the LATEST request, using the earlier messages only for context.";

pub async fn route(ctx: &Ctx, messages: &[ChatMessage]) -> Result<Route, String> {
    ctx.check()?;
    ctx.progress("Deciding how to help…");
    let google = ctx.app.state::<McpManager>().is_ready("google");

    let mut system = ROUTER_PROMPT.to_string();
    if !google {
        system.push_str("\nGoogle Workspace is NOT connected right now, so never answer \"google\".");
    }

    let route = match json_call(&ctx.api_key, &ctx.models.main, &system, &transcript(messages, 6), 300).await {
        Ok(value) => parse_route(value["route"].as_str(), google),
        // If routing itself fails, the planner can handle anything.
        Err(_) => Route::Plan,
    };
    ctx.check()?;
    Ok(route)
}

fn parse_route(route: Option<&str>, google_ready: bool) -> Route {
    match route.map(|r| r.trim().to_lowercase()).as_deref() {
        Some("utility") => Route::Utility,
        Some("google") if google_ready => Route::Google,
        Some("google") => Route::Utility,
        _ => Route::Plan,
    }
}

/// The last few messages as plain text, newest last.
pub fn transcript(messages: &[ChatMessage], keep: usize) -> String {
    let start = messages.len().saturating_sub(keep);
    let mut out = String::new();
    for (i, m) in messages[start..].iter().enumerate() {
        let last = start + i == messages.len() - 1;
        let who = match (m.role.as_str(), last) {
            ("user", true) => "Latest request",
            ("user", false) => "User",
            _ => "Leo",
        };
        let text: String = m.content.clone().unwrap_or_default().chars().take(600).collect();
        out.push_str(&format!("{who}: {text}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_routes() {
        assert_eq!(parse_route(Some("utility"), true), Route::Utility);
        assert_eq!(parse_route(Some(" Plan "), true), Route::Plan);
        assert_eq!(parse_route(Some("google"), true), Route::Google);
        // Google not connected: never route there.
        assert_eq!(parse_route(Some("google"), false), Route::Utility);
        // Anything unexpected goes to the planner.
        assert_eq!(parse_route(Some("banana"), true), Route::Plan);
        assert_eq!(parse_route(None, true), Route::Plan);
    }

    #[test]
    fn transcript_marks_the_latest_request() {
        let messages = vec![
            ChatMessage::text("user", "hi"),
            ChatMessage::text("assistant", "hello"),
            ChatMessage::text("user", "send it"),
        ];
        let t = transcript(&messages, 6);
        assert!(t.contains("User: hi"));
        assert!(t.contains("Leo: hello"));
        assert!(t.trim_end().ends_with("Latest request: send it"));
    }

    /// Live check with a real model:
    /// `OPENROUTER_KEY=... OPENROUTER_MODEL=... cargo test live_routing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_routing() {
        let key = std::env::var("OPENROUTER_KEY").expect("set OPENROUTER_KEY");
        let model = std::env::var("OPENROUTER_MODEL").expect("set OPENROUTER_MODEL");
        let cases = [
            ("what is the capital of France", "utility"),
            ("explain how MPI_Send works", "utility"),
            ("show my unread emails", "google"),
            ("what's on my calendar tomorrow", "google"),
            ("edit my google doc Festivals and make it more professional", "google"),
            ("add a Total row to my budget sheet", "google"),
            ("what's the weather", "plan"),
            ("email Priya the Q3 report", "plan"),
            ("search the web for the top 3 AI news and put them in a new Google Doc", "plan"),
        ];
        tauri::async_runtime::block_on(async {
            let mut wrong = 0;
            for (text, want) in cases {
                let messages = vec![ChatMessage::text("user", text)];
                let got = json_call(&key, &model, ROUTER_PROMPT, &transcript(&messages, 6), 60).await;
                let route = got.as_ref().ok().and_then(|v| v["route"].as_str().map(str::to_string));
                println!("{text:75} -> {route:?} (wanted {want})");
                if route.as_deref() != Some(want) {
                    wrong += 1;
                }
            }
            assert!(wrong <= 1, "{wrong} routes differed from what was expected");
        });
    }
}

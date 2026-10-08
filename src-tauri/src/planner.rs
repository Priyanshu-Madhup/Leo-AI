//! The planner: turns a request into steps, runs them one at a time, and
//! edits the plan when a step does not deliver.
//!
//! 1. The planner writes a raw plan: a few steps, each given to the utility
//!    agent or the Google agent, with what that step's result must contain.
//! 2. Each step runs on its agent. The agent is told the overall request and
//!    the verified results of earlier steps; the later steps stay hidden.
//! 3. The verifier checks the step against ITS expected result only.
//! 4. If it fails, the step is retried with the verifier's feedback (unless
//!    it already changed something, so nothing is done twice). If it still
//!    fails, or the verifier says the result changes the picture, the planner
//!    rewrites the REMAINING steps from what has really happened.
//! 5. After all steps (or when it must stop), one more call writes the final
//!    reply from the verified results.
//!
//! Limits: 6 steps, 2 retries per step, 2 plan edits per request.

use std::collections::VecDeque;

use serde_json::{json, Value};
use tauri::Manager;

use crate::agent::{run_agent, Ctx};
use crate::agents::AgentId;
use crate::mcp::McpManager;
use crate::openrouter::{json_call, quick_chat};
use crate::orchestrator::transcript;
use crate::types::ChatMessage;
use crate::verifier;

const MAX_STEPS: usize = 6;
const MAX_RETRIES: usize = 2;
const MAX_REPLANS: usize = 2;
const MAX_RESULT_CHARS: usize = 1200;

#[derive(Clone, Debug)]
struct Step {
    agent: AgentId,
    task: String,
    expected: String,
}

struct Done {
    agent: AgentId,
    task: String,
    output: String,
}

enum StepOutcome {
    Verified { output: String, replan: bool },
    Failed { output: String, reason: String },
}

const AGENTS_TEXT: &str = "- utility: the general assistant. Web search and reading pages, the user's memory (recall or save facts about them), the current date and time, opening apps or sites, reasoning and writing.\n\
- google: Google Workspace: Gmail, Calendar, Contacts, Drive, Docs, Sheets, Slides.";

const GOOGLE_RULES: &str = "- Anything inside the user's Google account (a document, spreadsheet, slide deck, Drive file, email, event or contact) can ONLY be read or changed by the google agent. The utility agent cannot open them: a web address for a Google document does not work for it.\n\
- Never split one job that a single agent can do from start to finish (for example: read a document and rewrite it) into several steps. Results passed between steps are shortened, so a whole document cannot be carried from one step to the next. Put the whole job in one step.\n";

const FORMAT_TEXT: &str = "Reply with only JSON: {\"steps\":[{\"agent\":\"utility\"|\"google\",\"task\":\"...\",\"expected\":\"...\"}]}";

fn plan_prompt(google_ready: bool) -> String {
    let availability = if google_ready {
        ""
    } else {
        "\nGoogle Workspace is NOT connected, so never use the google agent."
    };
    format!(
        "You plan jobs for Leo, a personal assistant. Break the user's request into the fewest steps that get it done, each handled by one agent:\n{AGENTS_TEXT}{availability}\n\
Rules:\n\
- Each step's `task` must stand on its own: say exactly what to do and what to return. Results of earlier steps are passed to later steps separately, so never write \"the previous result\"; just say what is needed.\n\
- `expected` states what that step's result must contain for the step to count as done (the concrete facts, ids, links, or a confirmation). Be specific.\n\
{GOOGLE_RULES}\
- Keep any action that changes something (sending an email, creating a document, changing an event) as its own step, after the lookups it depends on, unless one agent can do the whole job itself.\n\
- If the user did not say something a step needs and it can't be looked up, still plan the step: the agent will ask the user.\n\
- At most {MAX_STEPS} steps.\n{FORMAT_TEXT}"
    )
}

fn revise_prompt(google_ready: bool) -> String {
    let availability = if google_ready {
        ""
    } else {
        "\nGoogle Workspace is NOT connected, so never use the google agent."
    };
    format!(
        "A plan for Leo, a personal assistant, is running. Agents:\n{AGENTS_TEXT}{availability}\n\
You are given the request, the steps already finished (verified), the step that went wrong or the finding that changes things, and the steps that were still to come. \
Write the REMAINING steps only: do not repeat finished work, and change the approach if a step failed (a different query, asking the user, another agent). \
Same rules as before: self-contained tasks, a concrete `expected` for each, at most {MAX_STEPS} steps.\n{GOOGLE_RULES}\
If the job cannot be done, reply {{\"steps\":[],\"give_up\":\"why, in one sentence\"}}.\n{FORMAT_TEXT}"
    )
}

const FINAL_PROMPT: &str = "You write Leo's final reply to the user after a job was carried out by several assistants. \
Use only the verified results below; never invent anything. Lead with the outcome in one short sentence, then add details in Markdown (links, short lists) if they help. \
Never mention agents, steps, tools or planning. If the job stopped early, say plainly what was completed and what was not and why, and ask how they would like to continue.";

fn clip(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

fn last_user_text(chat: &[ChatMessage]) -> String {
    chat.iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.content.clone())
        .unwrap_or_default()
}

fn parse_steps(value: &Value, google_ready: bool) -> Result<VecDeque<Step>, String> {
    let mut steps = VecDeque::new();
    for s in value["steps"].as_array().into_iter().flatten() {
        let agent = match s["agent"].as_str().unwrap_or("") {
            "utility" => AgentId::Utility,
            "google" if google_ready => AgentId::Google,
            "google" => return Err("Google is not connected".to_string()),
            other => return Err(format!("unknown agent {other:?}")),
        };
        let task = s["task"].as_str().unwrap_or("").trim().to_string();
        if task.is_empty() {
            continue;
        }
        steps.push_back(Step {
            agent,
            task,
            expected: s["expected"].as_str().unwrap_or("").trim().to_string(),
        });
        if steps.len() >= MAX_STEPS {
            break;
        }
    }
    Ok(steps)
}

fn describe_steps<'a>(steps: impl Iterator<Item = &'a Step>) -> String {
    let lines: Vec<String> = steps
        .enumerate()
        .map(|(i, s)| format!("{}. [{}] {} (expected: {})", i + 1, s.agent.key(), s.task, s.expected))
        .collect();
    if lines.is_empty() {
        "(none)".to_string()
    } else {
        lines.join("\n")
    }
}

fn describe_done(done: &[Done]) -> String {
    if done.is_empty() {
        return "(nothing yet)".to_string();
    }
    done.iter()
        .enumerate()
        .map(|(i, d)| format!("{}. [{}] {}\n   Result: {}", i + 1, d.agent.key(), d.task, clip(&d.output, MAX_RESULT_CHARS)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// What one agent is told for one step: the whole request, the verified
/// results so far, its own task, and (on a retry) why the last try was refused.
fn step_message(goal: &str, done: &[Done], step: &Step, feedback: Option<&str>) -> String {
    let mut text = format!("Overall request from the user: {goal}\n");
    if !done.is_empty() {
        text.push_str("\nVerified results from earlier steps:\n");
        text.push_str(&describe_done(done));
        text.push('\n');
    }
    text.push_str(&format!("\nYour task: {}\n", step.task));
    if let Some(feedback) = feedback {
        text.push_str(&format!("\nYour previous attempt was rejected: {feedback}\nFix that.\n"));
    }
    text.push_str("\nWhen finished, reply with the concrete result (facts, ids, links, or what you did).");
    text
}

pub async fn run_job(ctx: &Ctx, chat: &[ChatMessage]) -> Result<String, String> {
    let google = ctx.app.state::<McpManager>().is_ready("google");
    let goal = last_user_text(chat);

    ctx.progress("Planning the steps…");
    let plan = json_call(&ctx.api_key, &ctx.models.planner, &plan_prompt(google), &transcript(chat, 6), 1500)
        .await
        .and_then(|v| parse_steps(&v, google));
    ctx.check()?;
    let mut steps = match plan {
        Ok(steps) if !steps.is_empty() => steps,
        // No usable plan: let one agent do its best with the whole request.
        // The Google agent can also reach the utility agent (web, memory), so
        // it is the broader choice whenever Google is connected.
        other => {
            if let Err(err) = other {
                eprintln!("planning failed: {err}");
            }
            let fallback = if google { AgentId::Google } else { AgentId::Utility };
            return run_agent(ctx, fallback, chat.to_vec()).await;
        }
    };

    let mut done: Vec<Done> = Vec::new();
    let mut replans = 0;
    let mut stopped: Option<String> = None;

    while let Some(step) = steps.pop_front() {
        ctx.check()?;
        ctx.progress(&format!("Step {} of {}: {}", done.len() + 1, done.len() + 1 + steps.len(), clip(&step.task, 70)));
        match run_step(ctx, &goal, &done, &step).await? {
            StepOutcome::Verified { output, replan } => {
                done.push(Done { agent: step.agent, task: step.task.clone(), output });
                if replan && replans < MAX_REPLANS && !steps.is_empty() {
                    replans += 1;
                    ctx.progress("Adjusting the plan…");
                    match revise_plan(ctx, &goal, &done, None, &steps, google).await {
                        Ok((new_steps, _)) => steps = new_steps,
                        Err(err) => eprintln!("could not revise the plan: {err}"),
                    }
                }
            }
            StepOutcome::Failed { output, reason } => {
                if replans >= MAX_REPLANS {
                    stopped = Some(reason);
                    break;
                }
                replans += 1;
                ctx.progress("Adjusting the plan…");
                match revise_plan(ctx, &goal, &done, Some((&step, &reason, &output)), &steps, google).await {
                    Ok((new_steps, give_up)) if !new_steps.is_empty() => {
                        let _ = give_up;
                        steps = new_steps;
                    }
                    Ok((_, give_up)) => {
                        stopped = Some(give_up.unwrap_or(reason));
                        break;
                    }
                    Err(_) => {
                        stopped = Some(reason);
                        break;
                    }
                }
            }
        }
    }

    ctx.check()?;
    ctx.progress("Writing the answer…");
    finalize(ctx, &goal, &done, stopped.as_deref()).await
}

/// Runs one step with retries, checking each attempt with the verifier.
async fn run_step(ctx: &Ctx, goal: &str, done: &[Done], step: &Step) -> Result<StepOutcome, String> {
    let mut feedback: Option<String> = None;
    let mut last_output = String::new();

    for _ in 0..=MAX_RETRIES {
        ctx.check()?;
        ctx.clear_log();
        let writes_before = ctx.writes();

        let message = step_message(goal, done, step, feedback.as_deref());
        let output = run_agent(ctx, step.agent, vec![ChatMessage::text("user", message)]).await?;
        let log = ctx.take_log();

        ctx.progress("Checking the result…");
        let verdict = verifier::check(ctx, &step.task, &step.expected, &output, &log).await;
        ctx.check()?;
        if verdict.pass {
            return Ok(StepOutcome::Verified { output, replan: verdict.replan });
        }

        let reason = if verdict.reason.is_empty() {
            "The result did not match what was expected.".to_string()
        } else {
            verdict.reason
        };
        last_output = output;
        if ctx.writes() > writes_before {
            // Something was already changed; trying again could do it twice.
            return Ok(StepOutcome::Failed {
                output: last_output,
                reason: format!("{reason} (an action had already been carried out, so it was not repeated)"),
            });
        }
        feedback = Some(reason);
        ctx.progress("Trying that step again…");
    }

    Ok(StepOutcome::Failed { output: last_output, reason: feedback.unwrap_or_default() })
}

/// Asks the planner for new remaining steps. Returns the steps and, when it
/// gave up, its reason.
async fn revise_plan(
    ctx: &Ctx,
    goal: &str,
    done: &[Done],
    failed: Option<(&Step, &str, &str)>,
    remaining: &VecDeque<Step>,
    google_ready: bool,
) -> Result<(VecDeque<Step>, Option<String>), String> {
    let mut user = format!(
        "Request: {goal}\n\nFinished steps (verified):\n{}\n",
        describe_done(done)
    );
    match failed {
        Some((step, reason, output)) => user.push_str(&format!(
            "\nThe step that went wrong:\n[{}] {} (expected: {})\nWhy it was rejected: {}\nWhat the agent answered: {}\n",
            step.agent.key(),
            step.task,
            step.expected,
            reason,
            clip(output, 800),
        )),
        None => user.push_str("\nThe last finished step passed, but its result suggests the remaining plan may no longer fit.\n"),
    }
    user.push_str(&format!("\nSteps that were still to come:\n{}\n", describe_steps(remaining.iter())));

    let value = json_call(&ctx.api_key, &ctx.models.planner, &revise_prompt(google_ready), &user, 1500).await?;
    ctx.check()?;
    let give_up = value["give_up"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    Ok((parse_steps(&value, google_ready)?, give_up))
}

/// Writes the user-facing answer from the verified results.
async fn finalize(ctx: &Ctx, goal: &str, done: &[Done], stopped: Option<&str>) -> Result<String, String> {
    let mut user = format!("Request: {goal}\n\nVerified results:\n{}\n", describe_done(done));
    if let Some(reason) = stopped {
        user.push_str(&format!("\nThe job stopped early. Reason: {reason}\n"));
    }

    let body = json!({
        "model": ctx.models.main,
        "messages": [
            { "role": "system", "content": FINAL_PROMPT },
            { "role": "user", "content": user },
        ],
        "temperature": 0.5,
        "max_tokens": 1500,
    });
    let reply = quick_chat(&ctx.api_key, body).await?;
    let text = reply.content.unwrap_or_default();
    if !text.trim().is_empty() {
        return Ok(text);
    }
    // The model had nothing to say; fall back to the raw results.
    Ok(done.last().map(|d| d.output.clone()).unwrap_or_else(|| {
        stopped
            .map(|r| format!("I couldn't finish that: {r}"))
            .unwrap_or_else(|| "I couldn't work out how to do that.".to_string())
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plan() {
        let v = json!({"steps": [
            {"agent": "utility", "task": "Find the user's city", "expected": "A city name"},
            {"agent": "google", "task": "Email Priya", "expected": "Confirmation it was sent"},
            {"agent": "utility", "task": "  ", "expected": "x"}
        ]});
        let steps = parse_steps(&v, true).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].agent, AgentId::Utility);
        assert_eq!(steps[1].agent, AgentId::Google);
    }

    #[test]
    fn refuses_unknown_or_unavailable_agents() {
        assert!(parse_steps(&json!({"steps": [{"agent": "gmail", "task": "x"}]}), true).is_err());
        assert!(parse_steps(&json!({"steps": [{"agent": "google", "task": "x"}]}), false).is_err());
    }

    #[test]
    fn caps_the_number_of_steps() {
        let many: Vec<Value> = (0..10).map(|i| json!({"agent": "utility", "task": format!("t{i}")})).collect();
        assert_eq!(parse_steps(&json!({ "steps": many }), true).unwrap().len(), MAX_STEPS);
    }

    #[test]
    fn step_message_carries_results_and_feedback_but_not_later_steps() {
        let done = vec![Done { agent: AgentId::Utility, task: "Find city".into(), output: "Bangalore".into() }];
        let step = Step { agent: AgentId::Utility, task: "Get the weather".into(), expected: "Temperature".into() };
        let m = step_message("weather please", &done, &step, Some("no temperature given"));
        assert!(m.contains("weather please"));
        assert!(m.contains("Bangalore"));
        assert!(m.contains("Get the weather"));
        assert!(m.contains("no temperature given"));
        // The verifier's expectation is not given to the agent.
        assert!(!m.contains("Temperature"));
    }

    /// Live check: `OPENROUTER_KEY=... OPENROUTER_MODEL=... cargo test live_plan -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_plan() {
        let key = std::env::var("OPENROUTER_KEY").expect("set OPENROUTER_KEY");
        let model = std::env::var("OPENROUTER_MODEL").expect("set OPENROUTER_MODEL");
        tauri::async_runtime::block_on(async {
            for request in [
                "Email Priya the notes from today's meeting",
                "What's the weather like today?",
                "Find the latest news about MPI and save a summary in a new Google Doc called MPI News",
            ] {
                let messages = vec![ChatMessage::text("user", request)];
                let v = json_call(&key, &model, &plan_prompt(true), &transcript(&messages, 6), 1200).await.unwrap();
                let steps = parse_steps(&v, true).expect("a valid plan");
                println!("
REQUEST: {request}");
                for (i, s) in steps.iter().enumerate() {
                    println!("  {}. [{}] {}
       expects: {}", i + 1, s.agent.key(), s.task, s.expected);
                }
                assert!(!steps.is_empty());
            }

            // A failing step must make the planner change course.
            let user = "Request: Email Priya the notes

Finished steps (verified):
(nothing yet)

                The step that went wrong:
[google] Find Priya's email address (expected: one email address)
                Why it was rejected: three contacts named Priya matched, no single address
                What the agent answered: Priya Sharma, Priya Nair, Priya Rao

                Steps that were still to come:
1. [google] Send the email (expected: confirmation)
";
            let v = json_call(&key, &model, &revise_prompt(true), user, 1200).await.unwrap();
            let steps = parse_steps(&v, true).expect("a valid revised plan");
            println!("
REVISED PLAN after the failure:");
            for (i, s) in steps.iter().enumerate() {
                println!("  {}. [{}] {}", i + 1, s.agent.key(), s.task);
            }
            assert!(!steps.is_empty());
        });
    }
}

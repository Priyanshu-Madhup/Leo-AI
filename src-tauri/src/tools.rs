//! Built-in tools. MCP tools are added alongside these later (see mcp.rs).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::mcp::{self, McpManager};
use crate::memory::MemoryState;
use crate::session::SessionInfo;
use crate::web::WebState;

/// Tool schemas in OpenAI function-calling format. Memory tools are only
/// offered once a MemoryLake key is configured.
pub fn definitions(app: &AppHandle, memory_enabled: bool) -> serde_json::Value {
    let mut tools = base_definitions();
    if memory_enabled {
        if let (Some(list), Some(extra)) = (tools.as_array_mut(), memory_definitions().as_array()) {
            list.extend(extra.iter().cloned());
        }
    }
    if let Some(list) = tools.as_array_mut() {
        list.extend(app.state::<McpManager>().definitions());
        // Search needs a Tavily key; without one the model isn't offered it.
        if app.state::<WebState>().key().is_none() {
            list.retain(|t| t["function"]["name"] != "web_search");
        }
    }
    tools
}

/// Tools that change something or reach outside the app. After the agent has
/// read untrusted content (email, files, web) these are refused for the rest
/// of the turn until the approval cards exist.
pub fn is_side_effect(name: &str) -> bool {
    matches!(name, "open_url" | "open_app" | "remember")
}

/// True for tools whose results come from outside the app and may contain
/// text written by someone else.
pub fn returns_untrusted_content(app: &AppHandle, name: &str) -> bool {
    matches!(name, "web_search" | "fetch_page" | "look_at_screen") || app.state::<McpManager>().owns(name)
}

pub fn memory_definitions() -> serde_json::Value {
    serde_json::json!([
        {
            "type": "function",
            "function": {
                "name": "recall_memory",
                "description": "Search the user's long-term memory for facts about them (name, location, preferences, people, projects, past conversations) and saved documents. Use it whenever an answer or action depends on personal information you don't already have.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "What to look up, as a short natural-language question, e.g. \"user's home city\"" }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "remember",
                "description": "Save something to the user's long-term memory. Use it when the user shares a lasting fact or preference about themselves, or asks you to remember something.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "fact": { "type": "string", "description": "One self-contained sentence written from the user's point of view, e.g. \"My name is Priyanshu and I live in Delhi.\"" }
                    },
                    "required": ["fact"]
                }
            }
        }
    ])
}

fn base_definitions() -> serde_json::Value {
    serde_json::json!([
        {
            "type": "function",
            "function": {
                "name": "ask_user",
                "description": "Ask the user a question in the chat and wait for their answer. Use it when the request is ambiguous, when several options are plausible (for example several contacts with the same name or several matching files), or when you need a detail you cannot look up. Offer 2-6 short options when you can; the user can also type their own answer. Never guess.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "question": { "type": "string", "description": "The question, phrased briefly." },
                        "options": {
                            "type": "array",
                            "description": "Suggested answers shown as buttons.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "label": { "type": "string", "description": "Short button text, e.g. a name and email address." },
                                    "description": { "type": "string", "description": "Optional extra detail shown under the label." }
                                },
                                "required": ["label"]
                            }
                        }
                    },
                    "required": ["question"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Search the web for current information: news, facts that may have changed, anything you are not sure of. Returns titles, links and short snippets. Follow up with fetch_page when a snippet is not enough.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search terms, like you would type into a search box." },
                        "max_results": { "type": "integer", "description": "How many results to return (1-8, default 5)." },
                        "topic": { "type": "string", "enum": ["general", "news"], "description": "Use \"news\" for recent events and headlines." }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "fetch_page",
                "description": "Read the text of a public web page, for example a search result. Returns plain text, truncated. Cannot open local or private addresses.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "Full http(s) URL of the page." }
                    },
                    "required": ["url"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "look_at_screen",
                "description": "Take a screenshot of the user's screen and get a detailed written description of it from the vision model. Use it whenever the request involves what is on the screen (\"what is this error\", \"read what I have open\", \"what am I looking at\") or when you need to see the result of something on screen. The screenshot is temporary and never saved. Put in `question` exactly what you need to know so the description focuses on it.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "question": { "type": "string", "description": "What you want to find out from the screen, e.g. \"what does the error dialog say?\". Leave empty for a full description." }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "current_datetime",
                "description": "Get the user's current local date, time and time zone. Read from the session clock shared by all agents.",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_location",
                "description": "Get the user's approximate location (city, region, country, time zone) from their internet connection. The result is stored for the whole session and shared by all agents, so only call it if your instructions say the location is not known yet.",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "open_url",
                "description": "Open a web page (http or https) in the user's default browser.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "Full URL starting with http:// or https://" }
                    },
                    "required": ["url"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "open_app",
                "description": "Launch an installed Windows application by its name, e.g. \"Notepad\" or \"Spotify\".",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "The app's display name" }
                    },
                    "required": ["name"]
                }
            }
        }
    ])
}

/// Which logo to show next to a tool call. `keys` are asset names to try in
/// order (file names in `src/assets/logos`, without the extension); `icon` is
/// the generic icon drawn when none of them exists.
#[derive(Serialize, Clone, Default)]
pub struct Brand {
    pub keys: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl Brand {
    fn icon(icon: &str) -> Self {
        Self { keys: Vec::new(), icon: Some(icon.to_string()) }
    }

    fn branded(keys: &[&str], icon: &str) -> Self {
        Self {
            keys: keys.iter().map(|k| k.to_string()).collect(),
            icon: Some(icon.to_string()),
        }
    }

    /// A website: its host, then parent domains and the bare name, so
    /// `en.wikipedia.org` finds `wikipedia.org` or just `wikipedia`.
    fn site(host: &str) -> Self {
        Self { keys: site_keys(host), icon: Some("globe".to_string()) }
    }

    /// An app, by name as a file name: "Visual Studio Code" is
    /// `visual-studio-code`.
    fn app(name: &str) -> Self {
        Self { keys: vec![slug(name)], icon: Some("tool".to_string()) }
    }
}

fn site_keys(host: &str) -> Vec<String> {
    let host = host.to_lowercase();
    let parts: Vec<&str> = host.split('.').collect();
    let mut keys: Vec<String> = (0..parts.len().saturating_sub(1))
        .map(|i| parts[i..].join("."))
        .collect();
    if parts.len() >= 2 {
        keys.push(parts[parts.len() - 2].to_string());
    }
    if keys.is_empty() {
        keys.push(host);
    }
    keys
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// How a tool call is described to the user: plain words, never internal
/// tool, server or search-engine names.
pub struct Presentation {
    pub title: String,
    pub detail: Option<String>,
    pub brand: Brand,
}

fn host_of(url: &str) -> Option<String> {
    let host = reqwest::Url::parse(url.trim()).ok()?.host_str()?.to_string();
    Some(host.strip_prefix("www.").unwrap_or(&host).to_string())
}

pub fn present(name: &str, args: &str) -> Presentation {
    let v: serde_json::Value = serde_json::from_str(args).unwrap_or_default();
    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    let some = |s: String| if s.is_empty() { None } else { Some(s) };
    let plain = |title: &str, detail: Option<String>, brand: Brand| Presentation {
        title: title.to_string(),
        detail,
        brand,
    };

    match name {
        "current_datetime" => plain("Checking the time", None, Brand::branded(&["clock"], "clock")),
        "get_location" => plain("Finding your location", None, Brand::icon("globe")),
        "web_search" => plain("Searching the web", some(field("query")), Brand::branded(&["web-search"], "globe")),
        "fetch_page" => match host_of(&field("url")) {
            Some(host) => plain("Reading a page", Some(host.clone()), Brand::site(&host)),
            None => plain("Reading a page", None, Brand::icon("globe")),
        },
        "open_url" => match host_of(&field("url")) {
            Some(host) => plain("Opening a site", Some(host.clone()), Brand::site(&host)),
            None => plain("Opening a site", None, Brand::icon("globe")),
        },
        "open_app" => {
            let app = field("name");
            let brand = if app.is_empty() { Brand::icon("tool") } else { Brand::app(&app) };
            plain("Opening an app", some(app), brand)
        }
        "look_at_screen" => plain("Looking at your screen", some(field("question")), Brand::branded(&["screen"], "eye")),
        "recall_memory" => plain("Checking memory", None, Brand::branded(&["memory"], "memory")),
        "remember" => plain("Saving to memory", None, Brand::branded(&["memory"], "memory")),
        "ask_user" => plain("Asking you", None, Brand::icon("tool")),
        other => match other.split_once("__") {
            Some((_, tool)) => {
                let mut p = google_presentation(tool);
                // Drive has no delete tool; trashing is an update with a flag.
                if tool == "update_drive_file" && v.get("trashed").and_then(|t| t.as_bool()) == Some(true) {
                    p.title = "Moving a file to trash".to_string();
                }
                p
            }
            None => plain("Working on it", None, Brand::icon("tool")),
        },
    }
}

/// Plain-language title and logo for the Google Workspace tools. Each tries
/// its own logo first, then the general `google` one.
fn google_presentation(tool: &str) -> Presentation {
    let has = |words: &[&str]| words.iter().any(|w| tool.contains(w));
    let reads = tool.starts_with("get_") || tool.starts_with("search_") || tool.starts_with("list_") || tool.starts_with("read_");
    let (title, keys): (&str, &[&str]) = if has(&["gmail"]) {
        (if tool.starts_with("send_") { "Sending an email" } else { "Checking Gmail" }, &["gmail", "google"])
    } else if has(&["event", "calendar"]) {
        (if reads { "Checking your calendar" } else { "Updating your calendar" }, &["google-calendar", "google"])
    } else if has(&["contact"]) {
        (if reads { "Looking up contacts" } else { "Updating contacts" }, &["google-contacts", "google"])
    } else if has(&["sheet"]) {
        (
            if reads {
                "Reading a spreadsheet"
            } else if tool.starts_with("format_") {
                "Formatting a spreadsheet"
            } else {
                "Updating a spreadsheet"
            },
            &["google-sheets", "google"],
        )
    } else if has(&["slides", "presentation"]) {
        (
            if reads {
                "Reading a presentation"
            } else if tool.starts_with("create_") || tool.starts_with("import_") {
                "Creating a presentation"
            } else {
                "Designing the slides"
            },
            &["google-slides", "google"],
        )
    } else if has(&["doc", "paragraph"]) {
        (
            if reads {
                "Reading a document"
            } else if tool.starts_with("create_") || tool.starts_with("import_") {
                "Creating a document"
            } else {
                "Editing a document"
            },
            &["google-docs", "google"],
        )
    } else if has(&["drive"]) {
        (if reads { "Searching Drive" } else { "Saving to Drive" }, &["google-drive", "google"])
    } else {
        ("Working on it", &[])
    };
    Presentation {
        title: title.to_string(),
        detail: None,
        brand: Brand::branded(keys, "tool"),
    }
}

/// Short human-readable title for a call (used on approval cards).
pub fn describe(name: &str, args: &str) -> String {
    present(name, args).title
}

pub async fn execute(app: &AppHandle, name: &str, args: &str) -> Result<String, String> {
    match name {
        "current_datetime" => Ok(app.state::<SessionInfo>().now_text()),
        "get_location" => {
            let place = app.state::<SessionInfo>().location().await?;
            Ok(format!("{} (approximate, from the user's internet connection)", place.describe()))
        }
        "web_search" => {
            let key = app
                .state::<WebState>()
                .key()
                .ok_or("Web search isn't set up. Add the Tavily key in settings.")?;
            crate::web::search(&key, args).await
        }
        "fetch_page" => crate::web::fetch(args).await,
        "look_at_screen" => Err("The screen is read through the agent loop.".to_string()),
        "open_url" => open_url(app, args),
        "open_app" => open_app(app, args),
        "recall_memory" => {
            let QueryArgs { query } = serde_json::from_str(args).map_err(|e| format!("Bad arguments: {e}"))?;
            app.state::<MemoryState>().recall(&query).await
        }
        "remember" => {
            let FactArgs { fact } = serde_json::from_str(args).map_err(|e| format!("Bad arguments: {e}"))?;
            app.state::<MemoryState>().remember(&fact).await
        }
        other if app.state::<McpManager>().owns(other) => mcp::call_tool(app, other, args).await,
        other => Err(format!("Unknown tool: {other}")),
    }
}

#[derive(Deserialize)]
struct UrlArgs {
    url: String,
}

#[derive(Deserialize)]
struct QueryArgs {
    /// Models sometimes call this with `{}` meaning "what do you know about me".
    #[serde(default = "default_query")]
    query: String,
}

fn default_query() -> String {
    "everything known about the user".to_string()
}

#[derive(Deserialize)]
struct FactArgs {
    fact: String,
}

#[derive(Deserialize)]
struct NameArgs {
    name: String,
}

fn open_url(app: &AppHandle, args: &str) -> Result<String, String> {
    let UrlArgs { url } = serde_json::from_str(args).map_err(|e| format!("Bad arguments: {e}"))?;
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    let scheme_ok = lower.starts_with("https://") || lower.starts_with("http://");
    if !scheme_ok || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("Only plain http/https URLs can be opened.".to_string());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())?;
    Ok(format!("Opened {url}"))
}

fn open_app(app: &AppHandle, args: &str) -> Result<String, String> {
    let NameArgs { name } = serde_json::from_str(args).map_err(|e| format!("Bad arguments: {e}"))?;
    let wanted = name.trim().to_lowercase();
    if wanted.is_empty() {
        return Err("No app name given.".to_string());
    }

    // Apps are resolved against Start Menu shortcuts and opened by path, so
    // the model-supplied name is only ever compared against file names and
    // never passed to a shell.
    let mut shortcuts = Vec::new();
    for root in start_menu_roots() {
        collect_shortcuts(&root, 0, &mut shortcuts);
    }

    let stem = |p: &PathBuf| {
        p.file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    };
    let best = shortcuts
        .iter()
        .find(|p| stem(p) == wanted)
        .or_else(|| {
            shortcuts
                .iter()
                .filter(|p| stem(p).contains(&wanted))
                .min_by_key(|p| stem(p).len())
        })
        .ok_or_else(|| format!("No installed app matching \"{}\".", name.trim()))?;

    app.opener()
        .open_path(best.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())?;
    Ok(format!("Opened {}", best.file_stem().unwrap_or_default().to_string_lossy()))
}

fn start_menu_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for var in ["ProgramData", "APPDATA"] {
        if let Ok(base) = std::env::var(var) {
            roots.push(Path::new(&base).join("Microsoft/Windows/Start Menu/Programs"));
        }
    }
    roots
}

fn collect_shortcuts(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_shortcuts(&path, depth + 1, out);
        } else if path
            .extension()
            .map(|e| e.eq_ignore_ascii_case("lnk"))
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
}

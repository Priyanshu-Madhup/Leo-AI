//! Minimal MCP client over stdio (newline-delimited JSON-RPC 2.0).
//!
//! Servers are described in `mcp.json` in the app's config directory:
//!
//! ```json
//! { "servers": { "google": {
//!     "command": "uvx", "args": ["workspace-mcp@2.0.1", "--read-only"],
//!     "env": { "GOOGLE_OAUTH_CLIENT_ID": "..." },
//!     "inject": { "user_google_email": "me@example.com" },
//!     "tools_allow": ["search_drive_files"],   // optional whitelist
//!     "enabled": true } } }
//! ```
//!
//! `inject` supplies fixed tool arguments: they are hidden from the model's
//! view of the tool schema and filled in on every call.
//!
//! Each server's tools are exposed to the model as `<server>__<tool>`.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex as AsyncMutex};
use tokio::time::timeout;

const PROTOCOL_VERSION: &str = "2025-06-18";
/// First start can download packages, so be generous.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(240);
const LIST_TIMEOUT: Duration = Duration::from_secs(30);
/// Covers the user completing an interactive sign-in in the browser.
const CALL_TIMEOUT: Duration = Duration::from_secs(180);
const STDERR_TAIL_LINES: usize = 20;
const GOOGLE_AUTH_PREFIX: &str = "https://accounts.google.com/";

fn default_true() -> bool {
    true
}

#[derive(Deserialize, Default)]
struct FileConfig {
    #[serde(default)]
    servers: HashMap<String, ServerConfig>,
}

#[derive(Deserialize, Clone)]
struct ServerConfig {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    inject: Map<String, Value>,
    /// When set, only these tools are offered to the model. Lets a server run
    /// a large tool tier without handing the model dozens of tools it doesn't
    /// need.
    #[serde(default)]
    tools_allow: Option<Vec<String>>,
    #[serde(default = "default_true")]
    enabled: bool,
}

struct ToolInfo {
    name: String,
    description: String,
    schema: Value,
    /// From the tool's `readOnlyHint` annotation; anything not marked
    /// read-only needs the user's approval before it runs.
    read_only: bool,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

struct Server {
    /// Held so the process is killed when the server is dropped.
    _child: Child,
    stdin: AsyncMutex<ChildStdin>,
    pending: Pending,
    next_id: AtomicU64,
    tools: Vec<ToolInfo>,
    inject: Map<String, Value>,
}

#[derive(Serialize, Clone)]
pub struct ServerStatus {
    name: String,
    /// "starting" | "ready" | "error"
    state: String,
    tools: usize,
    message: String,
    /// An injected setting the user still has to provide (e.g. their Google
    /// email), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    needs: Option<String>,
}

#[derive(Default)]
pub struct McpManager {
    servers: Mutex<HashMap<String, Arc<Server>>>,
    status: Mutex<HashMap<String, ServerStatus>>,
    /// Values the user typed in Settings that replace/fill `inject` entries.
    overrides: Mutex<HashMap<String, Value>>,
}

impl Server {
    async fn send(&self, message: Value) -> Result<(), String> {
        let mut line = message.to_string();
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("server not reachable: {e}"))?;
        stdin.flush().await.map_err(|e| e.to_string())
    }

    async fn request(&self, method: &str, params: Value, limit: Duration) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);

        let sent = self
            .send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        if let Err(err) = sent {
            self.pending.lock().unwrap().remove(&id);
            return Err(err);
        }

        let response = match timeout(limit, rx).await {
            Ok(Ok(value)) => value,
            Ok(Err(_)) => return Err("the server exited".to_string()),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                return Err(format!("{method} timed out"));
            }
        };
        if let Some(err) = response.get("error") {
            let message = err["message"].as_str().unwrap_or("unknown error");
            return Err(message.to_string());
        }
        Ok(response["result"].clone())
    }
}

fn config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    // Lets tests point at a real config folder (and its saved sign-in).
    if let Some(dir) = std::env::var_os("LEO_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    app.path().app_config_dir().map_err(|e| e.to_string())
}

fn read_config(app: &AppHandle) -> FileConfig {
    let path = match config_dir(app) {
        Ok(dir) => dir.join("mcp.json"),
        Err(_) => return FileConfig::default(),
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return FileConfig::default();
    };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        eprintln!("mcp.json is invalid: {e}");
        FileConfig::default()
    })
}

/// Starts every enabled server in the background; failures only mark that
/// server as errored.
pub fn start_all(app: &AppHandle) {
    let config = read_config(app);
    let base = config_dir(app).unwrap_or_default().join("servers");

    for (name, cfg) in config.servers {
        if !cfg.enabled {
            continue;
        }
        let app = app.clone();
        let workdir = base.join(&name);
        set_status(&app, &name, "starting", 0, "");
        let uv = bundled_uv(&app);
        tauri::async_runtime::spawn(async move {
            match connect(&cfg, &workdir, uv).await {
                Ok(server) => {
                    let tools = server.tools.len();
                    let manager = app.state::<McpManager>();
                    manager.servers.lock().unwrap().insert(name.clone(), Arc::new(server));
                    set_status(&app, &name, "ready", tools, "");
                }
                Err(err) => set_status(&app, &name, "error", 0, &err),
            }
        });
    }
}

fn set_status(app: &AppHandle, name: &str, state: &str, tools: usize, message: &str) {
    app.state::<McpManager>().status.lock().unwrap().insert(
        name.to_string(),
        ServerStatus {
            name: name.to_string(),
            state: state.to_string(),
            tools,
            message: message.to_string(),
            needs: None,
        },
    );
}

/// The `uv` program shipped inside the installer, if present. (`uvx` is just
/// `uv tool run`.) Development builds fall back to whatever is on the PATH.
fn bundled_uv(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().resource_dir().ok()?;
    ["resources/uv.exe", "uv.exe"].iter().map(|p| dir.join(p)).find(|p| p.exists())
}

async fn connect(cfg: &ServerConfig, workdir: &PathBuf, uv: Option<PathBuf>) -> Result<Server, String> {
    // Servers keep state (e.g. saved sign-in tokens) in their working
    // directory, so give each one its own folder outside the project.
    std::fs::create_dir_all(workdir).map_err(|e| e.to_string())?;

    // Use the bundled uv for `uvx` commands so users need to install nothing.
    let (program, args): (String, Vec<String>) = match (&uv, cfg.command.as_str()) {
        (Some(path), "uvx") => (
            path.to_string_lossy().to_string(),
            ["tool", "run"].iter().map(|a| a.to_string()).chain(cfg.args.iter().cloned()).collect(),
        ),
        _ => (cfg.command.clone(), cfg.args.clone()),
    };

    let mut command = Command::new(&program);
    command
        .args(&args)
        .envs(&cfg.env)
        .current_dir(workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

    let mut child = command
        .spawn()
        .map_err(|e| format!("couldn't start `{}`: {e}", cfg.command))?;
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;

    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));

    // Responses are matched to requests by id; anything else (log lines,
    // notifications) is ignored.
    let reader_pending = pending.clone();
    tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let is_response = value.get("result").is_some() || value.get("error").is_some();
            if let (true, Some(id)) = (is_response, value["id"].as_u64()) {
                if let Some(tx) = reader_pending.lock().unwrap().remove(&id) {
                    let _ = tx.send(value);
                }
            }
        }
        // Process ended: dropping the senders wakes every waiting request.
        reader_pending.lock().unwrap().clear();
    });

    let tail: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    let stderr_tail = tail.clone();
    tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let mut tail = stderr_tail.lock().unwrap();
            if tail.len() >= STDERR_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    });

    let mut server = Server {
        _child: child,
        stdin: AsyncMutex::new(stdin),
        pending,
        next_id: AtomicU64::new(1),
        tools: Vec::new(),
        inject: cfg.inject.clone(),
    };

    let with_context = |err: String| {
        let lines: Vec<String> = tail.lock().unwrap().iter().rev().take(3).cloned().collect();
        let context: Vec<String> = lines.into_iter().rev().collect();
        format!("{err} ({})", context.join(" | "))
    };

    server
        .request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "leo", "version": env!("CARGO_PKG_VERSION") },
            }),
            STARTUP_TIMEOUT,
        )
        .await
        .map_err(&with_context)?;
    server
        .send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await?;

    let mut cursor: Option<String> = None;
    loop {
        let params = match &cursor {
            Some(c) => json!({ "cursor": c }),
            None => json!({}),
        };
        let page = server
            .request("tools/list", params, LIST_TIMEOUT)
            .await
            .map_err(&with_context)?;
        for tool in page["tools"].as_array().into_iter().flatten() {
            if let Some(name) = tool["name"].as_str() {
                server.tools.push(ToolInfo {
                    name: name.to_string(),
                    description: tool["description"].as_str().unwrap_or("").to_string(),
                    schema: tool["inputSchema"].clone(),
                    read_only: tool["annotations"]["readOnlyHint"].as_bool().unwrap_or(false),
                });
            }
        }
        cursor = page["nextCursor"].as_str().map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    if let Some(allow) = &cfg.tools_allow {
        server.tools.retain(|t| allow.iter().any(|a| a == &t.name));
    }
    Ok(server)
}

/// Removes arguments that are injected on every call, so the model never
/// sees (or has to guess) them.
fn visible_schema(schema: &Value, inject: &Map<String, Value>) -> Value {
    let mut schema = if schema.is_object() {
        schema.clone()
    } else {
        json!({ "type": "object", "properties": {} })
    };
    if let Some(props) = schema.get_mut("properties").and_then(|p| p.as_object_mut()) {
        for key in inject.keys() {
            props.remove(key);
        }
    }
    if let Some(required) = schema.get_mut("required").and_then(|r| r.as_array_mut()) {
        required.retain(|r| r.as_str().is_none_or(|k| !inject.contains_key(k)));
    }
    schema
}

impl McpManager {
    /// Tool schemas for every ready server, in OpenAI function format.
    pub fn definitions(&self) -> Vec<Value> {
        let servers = self.servers.lock().unwrap();
        let mut names: Vec<&String> = servers.keys().collect();
        names.sort();

        let mut out = Vec::new();
        for name in names {
            let server = &servers[name];
            for tool in &server.tools {
                out.push(json!({
                    "type": "function",
                    "function": {
                        "name": format!("{name}__{}", tool.name),
                        "description": tool.description,
                        "parameters": visible_schema(&tool.schema, &server.inject),
                    }
                }));
            }
        }
        out
    }

    /// True for MCP tools that are not marked read-only (or are unknown).
    pub fn requires_approval(&self, full_name: &str) -> bool {
        match self.lookup(full_name) {
            Some((server, tool)) => server
                .tools
                .iter()
                .find(|t| t.name == tool)
                .map(|t| !t.read_only)
                .unwrap_or(true),
            None => false,
        }
    }

    /// True once the named server has connected and its tools are known.
    pub fn is_ready(&self, server: &str) -> bool {
        self.servers.lock().unwrap().contains_key(server)
    }

    pub fn owns(&self, full_name: &str) -> bool {
        self.lookup(full_name).is_some()
    }

    fn lookup(&self, full_name: &str) -> Option<(Arc<Server>, String)> {
        let (server_name, tool) = full_name.split_once("__")?;
        let server = self.servers.lock().unwrap().get(server_name)?.clone();
        server.tools.iter().any(|t| t.name == tool).then(|| (server, tool.to_string()))
    }

    pub fn statuses(&self) -> Vec<ServerStatus> {
        let overrides = self.overrides.lock().unwrap();
        let servers = self.servers.lock().unwrap();
        let mut list: Vec<ServerStatus> = self
            .status
            .lock()
            .unwrap()
            .values()
            .cloned()
            .map(|mut status| {
                if let Some(server) = servers.get(&status.name) {
                    status.needs = server
                        .inject
                        .iter()
                        .find(|(key, value)| {
                            value.as_str().is_some_and(|v| v.trim().is_empty()) && !overrides.contains_key(*key)
                        })
                        .map(|(key, _)| key.clone());
                }
                status
            })
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }
}

pub async fn call_tool(app: &AppHandle, full_name: &str, args: &str) -> Result<String, String> {
    let (server, tool) = app
        .state::<McpManager>()
        .lookup(full_name)
        .ok_or_else(|| format!("Unknown tool: {full_name}"))?;

    let mut arguments: Map<String, Value> = if args.trim().is_empty() {
        Map::new()
    } else {
        match serde_json::from_str(args) {
            Ok(Value::Object(map)) => map,
            _ => return Err("Bad arguments: expected a JSON object.".to_string()),
        }
    };
    // Settings the user typed (e.g. their Google email) win over the file.
    let overrides = app.state::<McpManager>().overrides.lock().unwrap().clone();
    for (key, value) in &server.inject {
        let value = overrides.get(key).cloned().unwrap_or_else(|| value.clone());
        if value.as_str().is_some_and(|v| v.trim().is_empty()) {
            return Err(if key == "user_google_email" {
                "Add your Google account email in settings first.".to_string()
            } else {
                format!("Finish setting up this connection in settings first ({key} is empty).")
            });
        }
        arguments.insert(key.clone(), value);
    }

    let result = server
        .request("tools/call", json!({ "name": tool, "arguments": arguments }), CALL_TIMEOUT)
        .await?;

    let text = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let text = open_sign_in_links(app, &text);

    if result["isError"].as_bool().unwrap_or(false) {
        Err(friendly_api_error(&text).unwrap_or(text))
    } else {
        Ok(text)
    }
}

/// Google servers answer an unauthorised call with a sign-in link. Opening it
/// ourselves (only for Google's own sign-in host) means the model never has to
/// read a long URL aloud.
fn open_sign_in_links(app: &AppHandle, text: &str) -> String {
    let Some(start) = text.find(GOOGLE_AUTH_PREFIX) else {
        return text.to_string();
    };
    let rest = &text[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, ')' | '>' | ']' | '"' | '\'' | '`'))
        .unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches(['.', ',', ';']);

    match app.opener().open_url(url, None::<&str>) {
        Ok(()) => format!(
            "{}[Google sign-in opened in the user's browser. Tell them to approve access there, then ask again.]{}",
            &text[..start],
            &text[start + url.len()..]
        ),
        Err(_) => text.to_string(),
    }
}

#[tauri::command]
pub fn mcp_status(manager: tauri::State<'_, McpManager>) -> Vec<ServerStatus> {
    manager.statuses()
}

#[tauri::command]
pub fn mcp_set_inject(manager: tauri::State<'_, McpManager>, key: String, value: String) {
    let mut overrides = manager.overrides.lock().unwrap();
    if value.trim().is_empty() {
        overrides.remove(&key);
    } else {
        overrides.insert(key, Value::String(value.trim().to_string()));
    }
}

/// The Google tools Leo offers. Kept in step with the agents in agents.rs.
const DEFAULT_GOOGLE_TOOLS: &[&str] = &[
    "search_gmail_messages",
    "get_gmail_message_content",
    "get_gmail_messages_content_batch",
    "send_gmail_message",
    "list_calendars",
    "get_events",
    "manage_event",
    "list_contacts",
    "get_contact",
    "search_contacts",
    "manage_contact",
    "search_drive_files",
    "get_drive_file_content",
    "get_drive_file_download_url",
    "get_drive_shareable_link",
    "create_drive_folder",
    "create_drive_file",
    "update_drive_file",
    "get_doc_content",
    "create_doc",
    "modify_doc_text",
    "import_to_google_doc",
    "update_paragraph_style",
    "insert_doc_elements",
    "read_sheet_values",
    "modify_sheet_values",
    "create_spreadsheet",
    "import_to_google_sheets",
    "format_sheet_range",
    "get_spreadsheet_info",
    "get_presentation",
    "create_presentation",
    "import_to_google_slides",
    "batch_update_presentation",
];

/// An `mcp.json` written by an older version lacks the Google tools added
/// since (it is never overwritten). Adds any that are missing to the Google
/// server's allow-list so every install gets them after an update.
fn add_missing_tools(file: &std::path::Path) {
    let Ok(text) = std::fs::read_to_string(file) else { return };
    let Ok(mut config) = serde_json::from_str::<Value>(&text) else { return };
    let Some(allow) = config["servers"]["google"]["tools_allow"].as_array_mut() else { return };
    let mut changed = false;
    for tool in DEFAULT_GOOGLE_TOOLS {
        if !allow.iter().any(|t| t.as_str() == Some(tool)) {
            allow.push(json!(tool));
            changed = true;
        }
    }
    if changed {
        if let Ok(updated) = serde_json::to_string_pretty(&config) {
            let _ = std::fs::write(file, updated);
        }
    }
}

/// On a fresh install there is no `mcp.json` yet. If this build carries a
/// Google sign-in client (baked in by the release workflow), write a ready-made
/// config for the Google connection; the user then only types their email in
/// Settings. An existing file is never touched.
pub fn ensure_default_config(app: &AppHandle) {
    if let Ok(dir) = config_dir(app) {
        add_missing_tools(&dir.join("mcp.json"));
    }
    let (Some(id), Some(secret)) = (option_env!("LEO_GOOGLE_CLIENT_ID"), option_env!("LEO_GOOGLE_CLIENT_SECRET"))
    else {
        return;
    };
    if id.is_empty() || secret.is_empty() {
        return;
    }
    let Ok(dir) = config_dir(app) else { return };
    let file = dir.join("mcp.json");
    if file.exists() {
        return;
    }

    let config = json!({
        "servers": { "google": {
            "command": "uvx",
            "args": [
                "workspace-mcp@2.0.1", "--single-user", "--tool-tier", "extended",
                "--permissions", "gmail:full", "calendar:full", "drive:full",
                "docs:full", "sheets:full", "slides:full", "contacts:full"
            ],
            "env": {
                "GOOGLE_OAUTH_CLIENT_ID": id,
                "GOOGLE_OAUTH_CLIENT_SECRET": secret,
                "OAUTHLIB_INSECURE_TRANSPORT": "1"
            },
            "inject": { "user_google_email": "" },
            "tools_allow": DEFAULT_GOOGLE_TOOLS,
            "enabled": true
        } }
    });
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(text) = serde_json::to_string_pretty(&config) {
        let _ = std::fs::write(file, text);
    }
}

/// Google answers a call to an API that is not switched on for the project
/// with a long, technical 403. Turn it into one line that says what to do.
fn friendly_api_error(text: &str) -> Option<String> {
    if !text.contains("SERVICE_DISABLED") && !text.contains("has not been used in project") {
        return None;
    }
    let url = regex::Regex::new(r"https://console\.(?:developers|cloud)\.google\.com/apis/api/[A-Za-z0-9._/?=&-]+")
        .ok()?
        .find(text)?
        .as_str()
        .trim_end_matches(['.', ',', '\'', '"'])
        .to_string();
    let title = regex::Regex::new(r"'serviceTitle': '([^']+)'")
        .ok()
        .and_then(|re| re.captures(text))
        .map(|c| c[1].to_string())
        .unwrap_or_else(|| "A Google API".to_string());
    Some(format!(
        "{title} is not switched on for Leo's Google project yet. Open {url}, click Enable, wait a minute or two, then try again."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_config_gets_the_newer_google_tools() {
        let dir = std::env::temp_dir().join(format!("leo-mcp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("mcp.json");
        std::fs::write(&file, r#"{"servers":{"google":{"command":"uvx","tools_allow":["create_doc","my_custom_tool"]}}}"#).unwrap();
        add_missing_tools(&file);
        let config: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let allow: Vec<&str> = config["servers"]["google"]["tools_allow"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.as_str())
            .collect();
        assert!(allow.contains(&"import_to_google_doc"));
        assert!(allow.contains(&"update_paragraph_style"));
        assert!(allow.contains(&"my_custom_tool"));
        assert_eq!(allow.iter().filter(|t| **t == "create_doc").count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    use super::friendly_api_error;

    #[test]
    fn explains_a_disabled_google_api() {
        let raw = "Error calling tool 'search_contacts': API error: <HttpError 403 ... \"People API has not been used in project 825586925944 before or it is disabled. Enable it by visiting https://console.developers.google.com/apis/api/people.googleapis.com/overview?project=825586925944 then retry.\". Details: \"[{'reason': 'SERVICE_DISABLED', 'metadata': {'serviceTitle': 'People API', 'service': 'people.googleapis.com'}}]\">";
        let msg = friendly_api_error(raw).expect("recognised");
        assert!(msg.starts_with("People API is not switched on"));
        assert!(msg.contains("https://console.developers.google.com/apis/api/people.googleapis.com/overview?project=825586925944"));
        assert!(friendly_api_error("some other failure").is_none());
    }
}

# Leo — detailed project guide

This file is written so that a person or an AI assistant can understand the whole project **without reading the source first**. It covers what Leo is, how it is built, every file's job, every command and event, every setting, how to run and test it, the safety rules, the conventions, and what is not built yet. Keep it up to date when you change behaviour.

> Project root: `P:\Leo\siri-orb` (the app; it is the root of the GitHub repo `Priyanshu-Madhup/Leo-AI`). `README.md` is the short user-facing page (install, release steps). One level up, `P:\Leo\Leo_ Functionalities.excalidraw` is the original feature diagram.

---

## 1. What Leo is

Leo is a **Windows desktop chat assistant** with a living 3D orb. You type to it and it answers in a chat window, typed out character by character. It is **agentic**: it can call tools (search the web, read your Gmail and Calendar, send email, create Google Docs, remember facts about you, open apps and sites) and it asks you before changing anything.

Two window modes:

- **Full mode** — a 440×700 frosted-glass chat window, centred. Large orb on top, chat below, composer at the bottom.
- **Widget mode** — a 160×150 transparent window with just the orb. Reached with the minimise button (–) in the top bar. It can be **dragged anywhere** on screen, remembers where you left it, and expands back to the chat when you click it (or with the ⤢ button that appears on hover).

### Look and feel rules (do not break these)

- **Monochrome only: black, white, grey, metal.** No coloured accents anywhere (the orb is a chrome sphere; state shows as brightness and pace, never hue).
- **Never show technical terms to the user.** Tool rows say "Searching the web", "Checking Gmail", "Sending an email" — never tool names, server names, search-engine names or "MemoryLake". The system prompt also tells the model not to mention them.
- Replies are Markdown, typed out character by character.

---

## 2. Tech stack

| Layer | Technology |
|---|---|
| Shell | **Tauri 2** (Rust backend + system WebView2). Identifier `com.priyanshu-madhup.siri-orb`, product name `siri-orb`, window title "Leo". |
| Frontend | **Vanilla TypeScript + Vite** (no UI framework). Plain DOM, CSS, one WebGL canvas. Vite dev server on **port 1420, strict**. |
| Backend | Rust (edition 2021): `reqwest` (rustls), `tokio`, `serde`/`serde_json`, `chrono`, `regex`, `window-vibrancy` (acrylic blur), `tauri-plugin-opener`. |
| LLM | **OpenRouter** (`https://openrouter.ai/api/v1/chat/completions`) — the only model provider. Chat and tool calling go through it. |
| Memory | **MemoryLake** REST API (`https://app.memorylake.ai/openapi/memorylake/api/v3`). |
| Web search | **Tavily** (`https://api.tavily.com/search`). |
| Google | Community MCP server **`workspace-mcp@2.0.1`** run through `uvx` (needs `uv`/Python installed). |

There is **no git repository** and no CI. The Rust build cache is redirected to `C:\Users\Priyanshu Madhup\.cargo-targets\siri-orb` because the `P:` drive is tiny (`.cargo/config.toml`).

---

## 3. Run, build, test

```
cd P:\Leo\siri-orb
npm install                 # once
npm run tauri dev           # full app (starts Vite on :1420 and the Rust shell)
npx tsc --noEmit            # typecheck the frontend
cd src-tauri
cargo check                 # typecheck the backend
cargo test                  # offline unit tests
```

Live tests (ignored by default, need real keys in environment variables, never commit keys):

```
MEMORYLAKE_KEY=...  cargo test live_recall  -- --ignored --nocapture
TAVILY_KEY=...      cargo test live_search  -- --ignored --nocapture
```

Notes:
- After changing **Rust**, the dev app must be restarted (`tauri dev` rebuilds). Frontend changes hot-reload.
- A plain browser tab at `http://localhost:1420` shows the UI but **Tauri commands do not work** there (useful only for visual checks of CSS, markdown, the orb). A hidden browser tab pauses `requestAnimationFrame`, so animations must be driven manually when testing in an automated browser.
- Tooling quirk in this environment: shell heredocs containing apostrophes can break; write helper scripts to a file and run them instead.

---

## 4. First-time setup (what the user must enter)

Open the app → ⚙ (settings) in the top bar. A **Settings window** opens over the chat (close it with ×, Esc, or a click on the dimmed backdrop). It is also opened automatically the first time, or when you send a message without a key/model. Each section shows a connection status and a **"Get a key" link** that opens the right website in your browser. Key fields have an eye button to show/hide the key. **Save** stores the keys and model; the blur toggle and transparency slider apply instantly.

| Section | Fields | Notes |
|---|---|---|
| AI model | OpenRouter API key (link: openrouter.ai/keys), Model name (link: models list filtered to tool support), and under *Advanced* an optional Planner model and Checker model | **Key and model required.** The model must support **tool calling**. |
| Long-term memory | MemoryLake API key (link: app.memorylake.ai) | Optional. The URL is **hardcoded** to `https://app.memorylake.ai` (no URL field). |
| Web search | Tavily API key (link: app.tavily.com/home) | Optional. Without it `web_search` is not offered. |
| Google | **Your Google account email** (stored as `leo.google.email`) | The Google tools act on the user's own account, so the backend needs their address (`mcp_set_inject`). The connection itself is configured in `mcp.json` (section 9); on a fresh install that file is written automatically (section 16). The Google Cloud app is published (In production), so any Google account can sign in. |
| Appearance | Blur behind window, Window transparency (0–100%) | 0% = solid dark panel, 100% = only the blur/clear glass. Default 90%. |

No base URLs are ever asked for; only API keys. The status chips (AI model, memory, web search, Google) refresh whenever the window opens and after Save.

### Local storage keys (frontend, per WebView)

`leo.openrouter.apiKey`, `leo.openrouter.model`, `leo.openrouter.plannerModel`, `leo.openrouter.verifierModel`, `leo.memorylake.apiKey`, `leo.tavily.key`, `leo.google.email`, `leo.mode` ("full"/"widget"), `leo.blur` ("1"/"0"), `leo.transparency` (0–100).

API keys live only in WebView local storage and in Rust memory at runtime. At startup the frontend pushes the MemoryLake and Tavily keys to Rust (`memory_configure`, `web_configure`) **only if non-empty**, so a reload can never switch them off; only an explicit Save can. The OpenRouter key and model are passed on every `agent_run` call.

### Files outside the project (Windows)

- `%APPDATA%\com.priyanshu-madhup.siri-orb\mcp.json` — MCP server config (contains the Google OAuth client ID/secret: **secret, never share or commit**).
- `%APPDATA%\com.priyanshu-madhup.siri-orb\servers\<name>\` — each MCP server's working directory (the Google server keeps its saved sign-in tokens here).
- `%APPDATA%\com.priyanshu-madhup.siri-orb\widget-position.json` — where the minimised orb was last dropped (`{"x":…,"y":…}`, physical pixels).

---

## 5. Architecture overview

```
┌────────────────────────── WebView (TypeScript) ──────────────────────────┐
│ main.ts  chat, cards, tool rows, settings window, widget dragging         │
│ orb.ts (WebGL)   markdown.ts   typewriter.ts   agent.ts (invoke/listen)   │
└───────────────┬───────────────────────────────▲──────────────────────────┘
        invoke  │ commands                      │ events "agent://event"
┌───────────────▼───────────────────────────────┴──────────────────────────┐
│ Rust: the agents                                                          │
│                                                                           │
│   agent_run ─▶ Orchestrator ─┬─▶ Utility agent                            │
│                (orchestrator.rs) │   replies, web, memory, time, open apps │
│                              ├─▶ Google agent ─┬─ Gmail    Calendar        │
│                              │                 ├─ Contacts Drive           │
│                              │                 └─ Docs Sheets Slides       │
│                              └─▶ Planner ── steps ──▶ Utility / Google     │
│                                   (planner.rs)  ▲                          │
│                                     edits plan  │ pass / fail              │
│                                                 └── Verifier (verifier.rs) │
│                                                                           │
│   shared: tools.rs · interact.rs (cards) · memory.rs · web.rs · mcp.rs     │
└───────────────────────────────────────────────────────────────────────────┘
        MCP servers (child processes over stdio):  Google Workspace
```

**Design principle:** the Rust backend owns the agents, secrets-at-runtime, subprocesses and network calls. The frontend renders events and sends the user's answers.

### The agents (`agents.rs` is the registry)

| Agent | Role | Own tools | May call |
|---|---|---|---|
| **Utility** | Simple replies, web search and page reading, memory, date/time, opening apps/sites | `current_datetime`, `web_search`, `fetch_page`, `open_url`, `open_app` | nobody |
| **Google** | Coordinates all Google Workspace work; has **no Google tools itself** | – | the 7 sub-agents + utility |
| **Gmail** | search/read mail, send email | 4 Gmail tools | utility |
| **Calendar** | list calendars, read/create/change events | 3 | utility |
| **Contacts** | look up and manage contacts | 4 | utility |
| **Drive** | find/read/create/update files, share links, move to trash | 7 | utility |
| **Docs** / **Sheets** / **Slides** | read, create, edit | 4 / 4 / 3 | utility |

Every agent also has `ask_user`, `recall_memory` and `remember` (**the memory tool is available to all**). Calling another agent is a tool call named `ask_<agent>` (e.g. `ask_gmail`, `ask_utility`) that runs that agent's own loop and returns its answer. **There are no cycles by construction**: the utility agent calls nobody; sub-agents call only the utility agent; Google calls its sub-agents and utility; depth is also capped at 3. An agent is *offered* only its own tools and a call to anything else is refused (`AgentId::allows`). Unit tests in `agents.rs` enforce the hierarchy, the tool scoping and that each of the 29 Google tools has exactly one owner.

### The roles that are not tool-using agents

- **Orchestrator** (`orchestrator.rs`): one short model call classifies the latest request into a route: `utility` (answer or do it in one go), `google` (ONE clear Google job with everything needed in the message), or `plan` (several steps where a later step needs an earlier result, or services are combined; e.g. "email Priya the report", "what's the weather" which first needs the user's city from memory). If Google isn't connected it never routes there; if routing fails it chooses `plan`.
- **Planner** (`planner.rs`): writes the raw plan (≤ 6 steps; each step = which agent, a self-contained task, and the `expected` result), runs the steps one at a time, and **edits the remaining steps** when needed (see §6). It can hand any step to the utility agent, which is how other agents get web data.
- **Verifier** (`verifier.rs`): sees only ONE step: its task, its `expected` result, the agent's answer and a log of the tools that really ran. It never sees the rest of the plan. Returns `{pass, reason, replan}`. A verifier error never blocks a job (the step is accepted).

### Models
Everything uses the main model by default. Settings → AI model → *Advanced* has optional **Planner model** and **Checker model** fields (`leo.openrouter.plannerModel` / `verifierModel`); blank means "main". The router, final-answer writer and all agents use the main model.

---

## 6. Request lifecycle (one turn)

1. **Input**: the user types in the composer (`sendMessage`). If a question/approval card is waiting, the text **answers the card** instead of starting a turn.
2. Frontend adds the user bubble, shows the typing dots, sets the orb to *thinking*, calls **`agent_run(apiKey, model, plannerModel, verifierModel, text)`**.
3. **`agent_run`** bumps the turn counter (cancelling any older run), builds the shared `Ctx` (models, cancel flag, "tainted" flag, tool log, saved-facts list) and asks the **orchestrator** for a route.
4. **Route `utility` / `google`**: that agent runs its loop on the conversation so far and its final text is the reply.
5. **Route `plan`** (`planner::run_job`):
   1. The planner writes the plan. If no usable plan comes back, the utility agent just handles the request.
   2. For each step: the agent gets the overall request, the **verified results of earlier steps**, and its own task (never the later steps or the expected result).
   3. The **verifier** checks that step. **Pass** → next step. **Fail** → retry up to 2 times with the verifier's reason as feedback, **unless the step already changed something** (an approved write), in which case it is not repeated.
   4. If a step still fails, or the verifier flags `replan`, the planner **rewrites the remaining steps** from what actually happened (finished work is never redone). At most **2 plan edits** per request; then the job stops and the final reply says what was done and what wasn't.
   5. A final model call writes the user-facing answer from the verified results.
6. **Inside every agent loop** (`agent::run_agent`, ≤ 8 rounds): build prompt (role + memory section if MemoryLake is set + web section for the utility agent) → call OpenRouter (retries) → if the reply has no tool calls it is the answer (an empty reply is nudged once, then a polite fallback) → otherwise for each tool call: refuse it if the agent isn't allowed it; refuse **cut-off arguments before anything is shown** (§13); `ask_user` → question card; `ask_<agent>` → run that agent; otherwise approval card if needed, then execute, emit `tool_start`/`tool_result`, and feed the result back.
7. The reply returns to the frontend: shown as Markdown and **typed out** while the orb ripples to the rhythm of the text. The conversation history stores only the user's messages and Leo's final replies (tool traffic stays inside the run).
8. **Memory pass** (reflection.rs) runs in the background (see §10), told which facts were already saved.

**Cancellation:** `agent_cancel` bumps the turn counter; every loop, the planner and card waits notice at their next checkpoint and return `"cancelled"`, which the frontend ignores. The frontend calls it when a new message is sent while one is running.

**Conversation memory vs. long-term memory:** the in-process history (last 40 messages) is lost on restart or **New chat**. Long-term facts live in MemoryLake.

---

## 7. Backend modules (`src-tauri/src/`)

| File | Responsibility |
|---|---|
| `lib.rs` | Tauri setup; command registration; window modes (`set_mode`), acrylic glass (`set_glass`, `set_blur`), transparent-window handling, **widget position persistence and clamping**; window-event hook that tracks the orb's position. |
| `main.rs` | Calls `siri_orb_lib::run()`. |
| `build.rs` | Tauri build + `rerun-if-env-changed` for the baked-in Google client (section 16). |
| `agent.rs` | The engine: `Ctx` (per-request shared state), **`run_agent`** (the one tool loop all agents use), delegation, approval/argument checks, `agent_run/agent_cancel/agent_reset`, `AgentEvent`, `tool_start()`. |
| `agents.rs` | The registry: `AgentId`, each agent's tools, who may call whom, delegation tool schemas, **all role prompts**. |
| `orchestrator.rs` | Routing call (`utility` / `google` / `plan`). |
| `planner.rs` | Plan creation, step running with retries, plan revision, final answer. |
| `verifier.rs` | Step checking. |
| `interact.rs` | `InteractState` (pending cards), `ask_user` and `approve`, `agent_answer` command. |
| `tools.rs` | Built-in tool schemas and execution, `definitions()`, `present()` (plain-language title/detail/brand for UI), `is_side_effect`, `returns_untrusted_content`, Start-Menu app launcher. |
| `mcp.rs` | Minimal MCP stdio client (JSON-RPC 2.0 over newline-delimited stdout/stdin), config loading, tool listing/calling, sign-in-link opener, `mcp_status`. |
| `memory.rs` | MemoryLake client: config, bootstrap, `remember`, `recall`, recent-facts cache. |
| `web.rs` | Tavily `search`, safe page reader `fetch`, `WebState` (key). |
| `reflection.rs` | After-reply memory pass. |
| `openrouter.rs` | Shared HTTP client, `post_chat` with retry/backoff. |
| `types.rs` | `ChatMessage`, `ToolCall`, `FunctionCall` (OpenAI-format messages). |
| `updater.rs` | Silent self-update on launch (section 16). |

### Tauri commands (frontend → Rust)

| Command | Args | Purpose |
|---|---|---|
| `agent_run` | `apiKey, model, plannerModel?, verifierModel?, text` | Run one request through the orchestrator; returns the reply text. |
| `agent_cancel` | – | Cancel the running turn. |
| `agent_reset` | – | Cancel and clear history (New chat). |
| `agent_answer` | `id, payload` | Answer a card. Ask: `{text}`. Approval: `{allow, note?}`. |
| `memory_configure` / `memory_is_configured` | `apiKey` / – | Set/read the MemoryLake key (the URL is a constant in `memory.rs`). |
| `web_configure` / `web_is_configured` | `apiKey` / – | Set/read Tavily key. |
| `mcp_status` | – | List MCP servers with state (`starting`/`ready`/`error`) and, if a required setting is missing, `needs` (e.g. `user_google_email`). |
| `mcp_set_inject` | `key, value` | Set a value that fills an `inject` entry (the user's Google email). Wins over the file; empty removes it. |
| `set_mode` | `mode: "full"\|"widget"` | Resize/reposition the window, toggle glass and taskbar entry. |
| `set_blur` | `enabled` | Acrylic blur on/off. |

### Event to the frontend: `agent://event`

Tagged by `kind` (snake_case):

- `tool_start { id, name, label, detail?, brand: { keys[], icon? } }`
- `tool_result { id, name, ok, error? }`
- `ask_user { id, question, options: [{label, description?}] }`
- `approval_request { id, name, label, args }`

### Built-in tools (what the model can call)

| Tool | Needs | Notes |
|---|---|---|
| `ask_user` | – | Question card with option buttons + free text. For ambiguity ("which Priya?"). |
| `current_datetime` | – | Local date/time/zone. |
| `open_url` | – | http/https only, opened in the default browser. |
| `open_app` | – | Matches Start Menu shortcuts by name and opens by path (never through a shell). |
| `web_search` | Tavily key | Returns a summary plus titles/links/snippets. Optional `topic: "news"`. |
| `fetch_page` | – | Reads a public page as text (truncated). Refuses localhost/private/link-local addresses, checks every redirect hop, 1.5 MB / 6,000-char caps. |
| `recall_memory` | MemoryLake | Search facts/documents. |
| `remember` | MemoryLake | Save one fact. |

Plus every MCP tool, named `<server>__<tool>` (e.g. `google__search_gmail_messages`).

### Safety rules in the agent loop

- **Approval cards** are required for: every **MCP tool not marked read-only** (`readOnlyHint` annotation), and — once the turn has read untrusted content ("tainted") — the built-in side-effect tools `open_url`, `open_app`, `remember`.
- **Tainted** = a tool that returns text written by others has run this turn: `web_search`, `fetch_page`, any MCP tool.
- Tool results and web/email text are **untrusted data**; the prompts tell the model never to follow instructions found in them.
- There is **no "always allow"** yet; every approval asks.
- The page reader is hardened against local-network access (SSRF).
- Questions/approvals time out after 10 minutes.

---

## 8. Frontend (`src/`)

| File | Responsibility |
|---|---|
| `main.ts` (~690 lines) | Everything UI. Sections in order: element lookups, keys/transparency/blur → orb state → **window mode** (`setMode`) → **chat transcript** (`addMessage`, typing dots, `followEnd`) → **question/approval cards** → **tool progress rows** (logos) → **settings window** (`openSettings`, `refreshStatus`, Save) → **sending** (`sendMessage`) → **dragging the minimised orb** → start-up. |
| `agent.ts` | `AgentClient` (`ask`, `cancel`, `reset`), event types, `answerCard`, `onAgentEvent`. |
| `orb.ts` | `SiriOrb`: WebGL raymarched chrome sphere. States `idle | thinking | speaking` (speaking = the reply is being written out); each has tones, speed, glow and **ripple** strength. Smooth, frame-rate-independent easing; travelling ripple waves (strongest while generating), gentle breathing when idle. `setHover(true)` (the canvas `pointerenter`/`pointerleave`) lifts ripple, pace and glow to a lively level for as long as the pointer is over the orb. `setState`, `setLevel(0..1)`. |
| `markdown.ts` | Safe Markdown renderer (builds DOM nodes, never `innerHTML`; only http(s) links). |
| `typewriter.ts` | `typewrite(el, onTick, onDone)`: reveals a rendered message character by character (~120 chars/s, speeds up so long replies finish in ≤3.5 s, eases in, short sentence pauses, blinking caret, blocks fade in). Respects reduced-motion. |
| `styles.css` | All styling. CSS variables: `--ink`, `--glass` (set live by the transparency slider), `--tint`, `--metal`, etc. |
| `assets/logos/` | Brand logos (see §11). |
| `vite-env.d.ts` | Vite client types (needed for `import.meta.glob`). |
| `../index.html` | Static layout: top bar, orb, caption, chat + composer, settings panel. |

### Cards (question and approval)

- **Ask card**: question, option chips (with optional description), "or type your own answer". Answer sent as `{text}`.
- **Approval card**: title ("Send this email?" if args look like an email), a table of the arguments (free-text fields like `body`/`content` get a tall box), **Send/Allow** (focused so Enter confirms), **Decline**, and "or say what to change" (decline with a note so the model revises). Typing "yes/ok/send it…" allows; "no/cancel…" declines; anything else declines with that text as the note.

### Tool progress rows

Each tool call is a row: a white disc with the brand logo (or a neutral icon) and a thin ring that **spins while running**, then plain-language title and optional detail. The logo is preloaded before the row appears. Failed rows show the short error text underneath.

### Window behaviours

- Full mode: acrylic blur (Windows) + CSS glass tint; 8 px corners come from Windows 11 (`set_shadow(true)`).
- Full-mode layout: the orb floats **over** the chat (`#orb-wrap` is absolutely positioned and `pointer-events: none`; only the canvas takes clicks). `#messages` spans the whole window and starts below the orb via top padding. A CSS **mask** on `#messages` (radial hole centred on the orb + a fade under the top bar) keeps only a circle around the orb clear, so text scrolls up past the orb's left and right sides. The orb diameter is the registered animatable property `--orb-d` (340 px empty, 140 px once there are messages), so the orb, the mask and the padding animate together.
- Widget mode: no blur, transparent, no taskbar entry; **drag from anywhere on the orb** (press-and-move > 5 px starts an OS drag; a press without movement is a click and opens the chat). A 10 px strip at the top and the capability `core:window:allow-start-dragging` also support dragging.
- Window flags (`tauri.conf.json`): borderless, transparent, always on top, not resizable, `shadow` toggled at runtime.

---

## 9. MCP (Model Context Protocol) servers

`mcp.rs` is a small custom stdio client (no external MCP crate). It starts every enabled server **in the background at app launch**, runs `initialize`, lists tools, and exposes them to the agent. A failing server only marks itself `error`.

`%APPDATA%\com.priyanshu-madhup.siri-orb\mcp.json`:

```json
{ "servers": { "google": {
  "command": "uvx",
  "args": ["workspace-mcp@2.0.1", "--single-user", "--tool-tier", "extended",
           "--permissions", "gmail:full", "calendar:full", "drive:full",
           "docs:full", "sheets:full", "slides:full", "contacts:full"],
  "env": { "GOOGLE_OAUTH_CLIENT_ID": "...", "GOOGLE_OAUTH_CLIENT_SECRET": "...",
           "OAUTHLIB_INSECURE_TRANSPORT": "1" },
  "inject": { "user_google_email": "<the user's Google address>" },
  "tools_allow": ["search_gmail_messages", "send_gmail_message", "..."],
  "enabled": true } } }
```

- `inject`: arguments filled on **every call** and hidden from the model's view of the schema (the Google tools all require `user_google_email`). A non-empty value typed in Settings (`mcp_set_inject`) overrides the file; an empty value with no override makes the tool return "Add your Google account email in settings first."
- `tools_allow`: optional whitelist; the server runs the large `extended` tier (63 tools) but Leo exposes only **29** (the core set plus `update_drive_file`).
- Tool approval comes from each tool's `readOnlyHint` annotation (`false`/missing ⇒ approval card).
- **Sign-in**: an unauthorised call returns a Google link; `mcp.rs` opens links starting with `https://accounts.google.com/` in the browser itself and tells the model to ask the user to approve. The OAuth client is a **Desktop** client in a Google Cloud project with the Gmail, Calendar, Drive, Docs, Sheets, Slides and People/Contacts **APIs** enabled and the user as a test user. In Testing mode, refresh tokens expire after about 7 days (re-sign-in).
- **Google capabilities now**: read/search Gmail and send mail; read/create/update Calendar events and contacts; Drive search/read/create/update (including **moving to trash** via `update_drive_file` with `trashed: true` — there is **no permanent delete tool**); create/edit Docs; read/write Sheets; create Slides. The model is told to confirm which file before trashing and to say "moved to trash", never "deleted".
- Google's own hosted MCP servers (`*mcp.googleapis.com`) were evaluated and **not used**: they need the Workspace Developer Preview Program (a Workspace account, not personal Gmail), a Web OAuth client and streamable-HTTP + OAuth support in the client, and offer fewer tools.

To add another MCP server: add an entry to `mcp.json` (stdio command + args + env), restart the app; its tools appear as `<name>__<tool>`. Consider `tools_allow` to keep the tool list small, and add friendly titles/brands in `tools::present()` (otherwise it shows "Working on it").

---

## 10. Memory (MemoryLake)

- Concepts: a **workspace** (the account's default), an **actor** (the key owner's HUMAN actor), a **project** `leo-memory` (created on first use), and **conversations**. Facts are stored per project, so they outlive any one conversation.
- **Writing**: `remember` appends a message to a conversation; MemoryLake extracts facts asynchronously (≈15–20 s until searchable). The API only allows appending to the current head message, so **each app session creates its own conversation** (`leo-<unix-nanos>`) and chains messages by `parent_message_id` (first message: `null`). On an append error it starts a fresh conversation once.
- **Reading**: `recall` → `POST /workspaces/{id}/memories/search {query, top_k: 8}`. Facts saved in the last **10 minutes** are also kept in a local list and appended to every recall as "Just saved (still being indexed)", so a lookup right after saving works.
- **Tools only exist when a key is set**, and the memory section of the system prompt is only sent then.
- **After-reply memory pass** (`reflection.rs`): after **every** exchange a background call (same model, max 4 steps, only `recall_memory`/`remember`) decides what to save: durable facts about the user, corrections (supersede old facts), written as first-person sentences; never secrets, small talk, or anything that came from email/files/web. If the turn was **tainted**, the assistant's reply is withheld from this pass (only the user's own words are shown). It is told which facts were already saved mid-turn. Its tool rows ("Checking memory", "Saving to memory") appear in the chat after the reply.

---

## 11. Brand logos (assets folder)

Put image files in `src/assets/logos/` (png, svg, webp, jpg). **The file name without extension is the key** (lowercase). Vite bundles them at build time (`import.meta.glob`), so there are no network lookups and no keys. If no logo matches, a neutral icon is drawn. Full list is also in `src/assets/logos/README.txt`.

- Google: `google` (fallback for all Google products), `gmail`, `google-calendar`, `google-docs`, `google-sheets`, `google-slides`, `google-drive`, `google-contacts`.
- Built-in steps: `web-search`, `memory`, `clock`.
- Websites Leo reads/opens: the site name (`wikipedia.org` or just `wikipedia`; parent domains and the bare name are tried).
- Apps Leo opens: the app name lowercased with dashes (`spotify`, `visual-studio-code`).

The Rust side decides the keys (`Brand { keys[], icon }` in `tools::present`); the frontend picks the first key that has a file (`findLogo`). Logos sit on a white disc, 24 px, with a 16 px image.

---

## 12. How to extend (recipes)

- **Add a built-in tool**: add its schema in `tools.rs` (`base_definitions`), a `present()` arm (title/detail/brand), an `execute()` arm; decide if it is a side effect (`is_side_effect`) or returns untrusted content (`returns_untrusted_content`). If it needs a key, offer it conditionally like `web_search`/memory in `definitions()` and `agent.rs` prompts.
- **Add an MCP server / Google capability**: edit `mcp.json` (args/permissions/`tools_allow`); update `google_presentation` in `tools.rs` for friendly titles; add logo files.
- **Add a setting**: add a field (with its "Get a key" link if it is a key) to a group in the `#settings` window in `index.html`, key constant + `fillSettings` + Save in `main.ts`, and (if Rust needs it) a `*_configure` command like `memory_configure`. Remember "startup pushes only non-empty values".
- **Change the model's behaviour**: edit the prompt constants in `agent.rs`. Memory/web text is appended conditionally.
- **Change the orb**: `orb.ts` — per-state numbers in `STATE_STYLE`, shader in `FRAGMENT_SRC`.
- **Change typing speed**: `BASE_CHARS_PER_SECOND` and `MAX_SECONDS` in `typewriter.ts`.
- **Change glass strength**: acrylic tint alpha in `set_glass` (`lib.rs`), CSS `--glass` / transparency slider default in `main.ts`.

---

## 13. Reliability notes

- **"Thinking" models and short calls**: models like `qwen/qwen3.7-flash` spend a small token budget entirely on hidden reasoning and return an empty answer, which made routing and planning fail ("the model did not return JSON") and requests hang on "Thinking". The short structured calls (routing, planning, checking, the final write-up) now go through `openrouter::quick_chat`: first with `"reasoning": {"enabled": false}`, and if the answer is empty or the model refuses that setting, again with a 4x larger budget. If planning still fails, the whole request goes to the Google agent when Google is connected (it can also reach the utility agent), otherwise to the utility agent.

- **Cut-off tool calls**: a long tool call (e.g. a whole document) used to be truncated by the 1,024-token reply limit, producing broken JSON ("Bad arguments") that still reached the approval card and was retried. Now agent replies get **4,096 tokens**, and every tool call's arguments are validated **before** any approval card; a cut-off call is rejected with an instruction to send less per call (the Docs agent is told to write long documents in parts of ≤ ~1,500 words). See `agent::arguments_are_valid`.

- `post_chat` retries up to 3 times on 429/5xx (also when a 200 body carries an error code), honouring `Retry-After` (max 8 s). The limit is usually the model **provider's shared capacity**, not the OpenRouter account. If a cheap model keeps rate-limiting, choose another model.
- Empty model replies: nudged once, then a fallback message ("I couldn't work out how to do that with the tools I have.").
- Failed tool rows show the error text; tool failures are returned to the model so it can recover.
- OpenRouter requires a **tool-calling** model for the main model.

---

### Timing / end-to-end run (debug builds only)
`src-tauri/src/e2e.rs` is a headless mode of the app, compiled only in debug builds. With `LEO_E2E_REQUEST` set, the app starts with no UI, runs that one request through the real pipeline (real Google connection and sign-in, auto-answering every question/approval card) and prints a timestamped timeline (`LEO_TRACE=1` adds every model call with its duration), then exits. **It performs real actions** (e.g. it really sends the email), so use it deliberately: `cd src-tauri` then `LEO_E2E_REQUEST="..." LEO_E2E_EMAIL=you@gmail.com OPENROUTER_KEY=... OPENROUTER_MODEL=... LEO_TRACE=1 cargo run` (add `CARGO_TARGET_DIR=...` to avoid clashing with a running dev app). Reference timing for "mail myself a hello note" with `qwen/qwen3.7-flash`: about 27 s (route 2 s, plan 1 s, 3 contact lookups 7 s, send 3 s, check + final answer 6 s).

---

## 14. Status

**Built and working**: orb + full/widget modes (drag, remembered position), glass window with blur and a transparency slider, a Settings window with key links and status, text chat with Markdown + typewriter, the multi-agent system (orchestrator, planner, verifier, utility agent, Google agent with 7 sub-agents), question/approval cards, MCP client + Google Workspace (Gmail, Calendar, Drive, Docs, Sheets, Slides, Contacts), MemoryLake memory with after-reply memory pass, Tavily web search + safe page reader, brand-logo rows, retries.

**Not built yet**: "always allow" for approvals; browser-control and filesystem MCP servers; streaming replies; a settings UI for MCP servers (config is a file); a forget-memory tool; Google tokens refresh handling beyond the 7-day Testing-mode limit.

**Known limits**: personal-account Google hosted MCP not usable; Drive cannot permanently delete; web search needs a Tavily key; blur may stutter when dragging on some Windows builds; logo files must be named as in §11.

---

## 15. Security checklist

- Voice (microphone, wake word, transcription, speech output) was **removed entirely** from the codebase; do not reintroduce it without being asked.
- Never put API keys in source or commit them. Keys live in local storage / `mcp.json` only. If a key has been pasted anywhere, **rotate it**.
- `mcp.json` holds the Google OAuth client secret — keep it private.
- Do not widen auto-approval: MCP write tools and tainted side effects must keep asking.
- Keep `fetch_page` local-network blocking and redirect checking intact.
- Keep Markdown rendering DOM-based (no `innerHTML` with model/web text).

---

## 16. Installer, updates and releases

### The installer
`npm run tauri build` produces a Windows **NSIS installer** (`bundle.targets: ["nsis"]`, product name "Leo"). It installs **per user** (`installMode: currentUser`), so no administrator rights are needed, and it downloads the WebView2 runtime silently if the machine lacks it. `bundle.createUpdaterArtifacts: true` additionally produces the signed update package and its `.sig`. Building locally needs the signing key: set `TAURI_SIGNING_PRIVATE_KEY_PATH` (and an empty `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`).

### The app icon
Leo's icon is the glass tile with the chrome orb. The original artwork is `src/assets/logos/leo-app-icon.png` (kept out of the bundled tool-row logos by an exclusion in the `import.meta.glob` in `main.ts`). It was cropped tighter and given a rounded-square silhouette so it stays readable at 16-32 px, saved as `src-tauri/icons/app-icon-source.png` (1024x1024), and expanded with `npx tauri icon src-tauri/icons/app-icon-source.png` into everything in `src-tauri/icons/` (the `.ico` holds 16-256 px). The `android/` and `ios/` sets that command also creates are deleted because Leo is Windows-only. Tauri uses these icons for the installer, the exe, the taskbar and the window.

### Silent self-update (`updater.rs`, plugin `tauri-plugin-updater`)
At every launch of a **release** build (never in `tauri dev`), a background task asks `https://github.com/Priyanshu-Madhup/Leo-AI/releases/latest/download/latest.json` whether a newer version exists (15 s timeout; failures are only logged). If so it emits `app://updating` (the UI shows "Updating Leo…"), downloads the signed installer, runs it quietly (`plugins.updater.windows.installMode: "quiet"`), and restarts the app. Updates are verified against the public key in `tauri.conf.json` (`plugins.updater.pubkey`). The matching **private key** lives outside the repo (`%USERPROFILE%\.tauri\leo-updater.key`) and in the GitHub secret `TAURI_SIGNING_PRIVATE_KEY`. **If that key is lost, installed apps can never update again** (they would need a new manual install with a new public key).

### Release pipeline (`.github/workflows/release.yml`)
Trigger: every push to `main` (and manual dispatch). The workflow reads `version` from `src-tauri/tauri.conf.json`; **if a tag `v<version>` already exists it stops**, so only a version bump produces a release. Otherwise it: sets up Node/Rust, downloads **uv** into `src-tauri/resources/uv.exe`, runs `npm ci`, and runs `tauri-apps/tauri-action`, which builds the installer, signs the update, creates the GitHub release `v<version>` and uploads the installer, the `.sig` and `latest.json`.

**To ship a new version:** raise `version` in `tauri.conf.json` (keep `package.json` and `Cargo.toml` in step), commit, push to `main`.

GitHub repository secrets used: `TAURI_SIGNING_PRIVATE_KEY` (required), `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (empty if none), `LEO_GOOGLE_CLIENT_ID` and `LEO_GOOGLE_CLIENT_SECRET` (optional).

### What ships so users install nothing else
- **uv** is bundled as a resource (`src-tauri/resources/uv.exe`, added by CI; git-ignored). `mcp.rs` (`bundled_uv`) runs `uvx` commands as `uv.exe tool run ...` from the install folder; in development it falls back to `uvx` on the PATH. uv downloads Python and the Google server package itself on first use.
- **Google sign-in client**: the Desktop OAuth client ID/secret are compiled in from the two secrets above (`option_env!("LEO_GOOGLE_CLIENT_ID")` / `..._SECRET`), never stored in the repository. On first launch, if `%APPDATA%\com.priyanshu-madhup.siri-orb\mcp.json` does not exist, `mcp::ensure_default_config` writes the standard Google server config (with an empty `user_google_email`). An existing file is never overwritten; to regenerate defaults after a release that changes them, delete `mcp.json`. For installed apps the client secret is not truly secret (Google treats desktop-app secrets as public); do not reuse that client for anything server-side.
- The user then only needs: an OpenRouter key + model, and (for Google) their email; Google asks them to sign in the first time.

### Things to know
- Windows only (x64). SmartScreen may warn on the first run because the installer is not code-signed with a purchased certificate; users click "More info -> Run anyway".
- The Google app being "In production" but **unverified** means the "Google hasn't verified this app" screen and a ~100-user cap until verification.
- `tauri dev` never self-updates and never reads the bundled uv.

---

## 17. Additions since 2.1.0 (summary)

- **Global shortcut** (Settings → Shortcut): `set_shortcut` registers one system-wide key combination (stored as `leo.shortcut`). Pressing it shows Leo and toggles between the orb widget and the full chat (event `shortcut://pressed`).
- **Streaming**: `openrouter::post_chat_stream` reads the SSE stream; for `utility`/`google` routes `run_agent` emits `delta` events (and `delta_reset` if the model then calls a tool) and the frontend draws the reply live. Plan jobs are not streamed.
- **Prompt caching**: `openrouter::cache_system_prompt` marks system prompts `cache_control: ephemeral` (Qwen needs this). `LEO_TRACE=1` prints cached tokens.
- **Chat summary** (`summary.rs`): from 10 messages on, older turns are folded into a rolling summary (≤1000 words); the model sees summary + last exchange + new message. Cleared by New chat.
- **Steps summary** (per request, `Ctx.steps_summary`): the tool calls already made, handed to each plan step so work is not repeated. Separate from the chat summary. Today's date is in every agent's prompt.
- **Planner**: broad/vague/time-sensitive research starts with a `scout` step; the plan is always reviewed after it. Utility research steps are not retried. Hard limits per request: 5 web searches, 4 page reads. Tavily search uses `advanced` depth.
- **Formatting**: Docs are created from Markdown through `import_to_google_doc`; `update_paragraph_style`, `insert_doc_elements`, `format_sheet_range`, `get_spreadsheet_info` and `batch_update_presentation` are exposed (34 Google tools). Existing `mcp.json` files must list the new tools (or be deleted to regenerate).
- **Animations** (`space.ts`): on a normal launch the orb just floats up from the bottom (CSS only, no stars); the starfield is only for updates: during an update a 15-second flight through space plays (the updater waits that long before installing, then the app restarts into the new version). A minimised widget opens to the full window first. Dev preview: Ctrl+Shift+U.
- **Updates** (Settings → Updates): shows the installed version and a "Check for updates" button (`check_for_updates`). The updater now writes every step (check, download, install, errors) to `updater.log` in `%APPDATA%\com.priyanshu-madhup.siri-orb\`; read it when an update does not arrive.
- **First launch of a new install**: the space flight plays once as "Setting up Leo" (flag `leo.setupShown` in local storage; delete it to see it again), then the orb rises and, without keys, the Settings welcome opens.
- **Smooth minimise**: the chat fades out (200 ms), then `set_mode` with `animate: true` runs `glide_to_widget`, which animates the window rectangle (size and position) down to the widget over about 0.3 s before switching off glass and the taskbar entry.
- **Streaming typer** (`createStreamTyper` in `typewriter.ts`): streamed text is queued and typed character by character (about 110 chars/s, catching up so it never lags more than about 1.4 s).
- **mcp.json upgrades**: on every launch `add_missing_tools` adds any Google tool from `DEFAULT_GOOGLE_TOOLS` that an older `mcp.json` lacks (custom entries are kept), so installs created by older versions get new tools after an update.
- **Update check retries**: each check waits up to 30 s and is tried up to 3 times; every failed attempt is written to `updater.log`.
- **Google not connected**: when Google cannot be used (not set up, starting, no email, or failed) the assistant is told why and what to tell the user (`McpManager::google_note`), instead of inventing a reason.

---

## 18. Additions in 2.3.0

- **Seeing the screen** (`vision.rs`): every agent has `look_at_screen {question}`. It captures the primary monitor in memory (Leo's own window is hidden for ~0.2 s), shrinks it to ≤1600 px, encodes JPEG, sends it to the main model through OpenRouter and returns a detailed written description. The image is never written to disk and is dropped after the call. The main model must accept image input. Its result counts as untrusted content (taints the turn). Tool row: "Looking at your screen".
- **Shared session facts** (`session.rs`, `SessionInfo`): the date/time clock and the IP-based location (`get_location`, via ipwho.is) are stored once per app session. `current_datetime` reads the stored clock (it keeps running from the first read); `get_location` returns the stored place or looks it up once. Every agent's prompt is rebuilt each round with what is already known, so agents use it instead of calling a tool.
- **All agents** now have `web_search`, `current_datetime`, `get_location` and `look_at_screen` (plus `ask_user`, memory). `fetch_page`, `open_url`, `open_app` stay with the utility agent. Search limits stay per request.
- **Docs approvals**: after the user approves writing to a Google document (created or first edited) in a request, further edits to that same document (`modify_doc_text`, `update_paragraph_style`, `insert_doc_elements`) in the same request no longer ask again. Scope is the request and that document id only. The Docs prompt also tells the agent to write in as few calls as possible.
- Correction: the project **is** a git repository (`origin` = GitHub `Priyanshu-Madhup/Leo-AI`, branch `main`); Google tools count is 34.

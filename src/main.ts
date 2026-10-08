import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import { SiriOrb, type OrbState } from "./orb";
import { AgentClient, answerCard, onAgentEvent, type AgentEvent, type Brand } from "./agent";
import { renderMarkdown } from "./markdown";
import { typewrite, type Typer } from "./typewriter";

const STORAGE_KEY = "leo.openrouter.apiKey";
const STORAGE_MODEL = "leo.openrouter.model";
const STORAGE_PLANNER_MODEL = "leo.openrouter.plannerModel";
const STORAGE_VERIFIER_MODEL = "leo.openrouter.verifierModel";
const STORAGE_MEMORY_KEY = "leo.memorylake.apiKey";
const STORAGE_SEARCH_KEY = "leo.tavily.key";
const STORAGE_GOOGLE_EMAIL = "leo.google.email";
const STORAGE_MODE = "leo.mode";
const STORAGE_BLUR = "leo.blur";
const STORAGE_TRANSPARENCY = "leo.transparency";

const canvas = document.querySelector<HTMLCanvasElement>("#orb")!;
const widget = document.querySelector<HTMLDivElement>("#widget")!;
const caption = document.querySelector<HTMLDivElement>("#caption")!;
const settingsBtn = document.querySelector<HTMLButtonElement>("#settings-btn")!;
const closeBtn = document.querySelector<HTMLButtonElement>("#close-btn")!;
const minimizeBtn = document.querySelector<HTMLButtonElement>("#minimize-btn")!;
const expandBtn = document.querySelector<HTMLButtonElement>("#expand-btn")!;
const newChatBtn = document.querySelector<HTMLButtonElement>("#new-chat-btn")!;
const statusText = document.querySelector<HTMLSpanElement>("#status-text")!;
const messagesEl = document.querySelector<HTMLDivElement>("#messages")!;
const composer = document.querySelector<HTMLFormElement>("#composer")!;
const composerInput = document.querySelector<HTMLTextAreaElement>("#composer-input")!;
const sendBtn = document.querySelector<HTMLButtonElement>("#send-btn")!;

const settingsModal = document.querySelector<HTMLDivElement>("#settings")!;
const settingsClose = document.querySelector<HTMLButtonElement>("#settings-close")!;
const apiKeyInput = document.querySelector<HTMLInputElement>("#api-key")!;
const modelInput = document.querySelector<HTMLInputElement>("#model-name")!;
const plannerModelInput = document.querySelector<HTMLInputElement>("#planner-model")!;
const verifierModelInput = document.querySelector<HTMLInputElement>("#verifier-model")!;
const memoryKeyInput = document.querySelector<HTMLInputElement>("#memory-key")!;
const searchKeyInput = document.querySelector<HTMLInputElement>("#search-key")!;
const googleEmailInput = document.querySelector<HTMLInputElement>("#google-email")!;
const blurToggle = document.querySelector<HTMLInputElement>("#blur-toggle")!;
const transparencySlider = document.querySelector<HTMLInputElement>("#transparency")!;
const transparencyValue = document.querySelector<HTMLSpanElement>("#transparency-value")!;
const saveBtn = document.querySelector<HTMLButtonElement>("#save-key-btn")!;
const settingsStatus = document.querySelector<HTMLParagraphElement>("#settings-status")!;
const chipAi = document.querySelector<HTMLSpanElement>("#chip-ai")!;
const chipMemory = document.querySelector<HTMLSpanElement>("#chip-memory")!;
const chipSearch = document.querySelector<HTMLSpanElement>("#chip-search")!;
const chipGoogle = document.querySelector<HTMLSpanElement>("#chip-google")!;

const orb = new SiriOrb(canvas);
orb.start();

function getApiKey(): string {
  return localStorage.getItem(STORAGE_KEY) ?? "";
}

function getModel(): string {
  return localStorage.getItem(STORAGE_MODEL) ?? "";
}

function getPlannerModel(): string {
  return localStorage.getItem(STORAGE_PLANNER_MODEL) ?? "";
}

function getVerifierModel(): string {
  return localStorage.getItem(STORAGE_VERIFIER_MODEL) ?? "";
}

const llm = new AgentClient(getApiKey, getModel, getPlannerModel, getVerifierModel);

// Memory and web search live in the Rust backend, which only offers those
// tools to the model once it has a key. At startup an empty stored key is
// skipped rather than sent, so a page reload can never switch them off; only
// an explicit Save can.
async function configureSearch(force = false) {
  const key = localStorage.getItem(STORAGE_SEARCH_KEY) ?? "";
  if (!force && !key) return;
  await invoke("web_configure", { apiKey: key });
}

async function configureMemory(force = false) {
  const key = localStorage.getItem(STORAGE_MEMORY_KEY) ?? "";
  if (!force && !key) return;
  await invoke("memory_configure", { apiKey: key });
}

// The Google tools act on the signed-in user's own account, so the backend
// needs that address. Same rule as above: startup never clears it.
async function configureGoogleEmail(force = false) {
  const email = localStorage.getItem(STORAGE_GOOGLE_EMAIL) ?? "";
  if (!force && !email) return;
  await invoke("mcp_set_inject", { key: "user_google_email", value: email });
}

void configureSearch();
void configureMemory();
void configureGoogleEmail();

// The updater (Rust) downloads and installs a newer release silently, then
// restarts the app; this only tells the user it is happening.
void listen<string>("app://updating", () => {
  statusText.textContent = "Updating Leo…";
  showCaption("Updating…");
});

// Window transparency: 0% is a solid dark panel, 100% shows only the blur (or
// nothing at all with blur off). It sets the opacity of the glass tint.
const DEFAULT_TRANSPARENCY = 90;
function applyTransparency(percent: number) {
  const opacity = (1 - percent / 100).toFixed(3);
  document.documentElement.style.setProperty("--glass", `rgba(14, 14, 16, ${opacity})`);
  transparencyValue.textContent = `${percent}%`;
}
const savedTransparency = Number(localStorage.getItem(STORAGE_TRANSPARENCY) ?? DEFAULT_TRANSPARENCY);
transparencySlider.value = String(Number.isFinite(savedTransparency) ? savedTransparency : DEFAULT_TRANSPARENCY);
applyTransparency(Number(transparencySlider.value));
transparencySlider.addEventListener("input", () => {
  const percent = Number(transparencySlider.value);
  applyTransparency(percent);
  localStorage.setItem(STORAGE_TRANSPARENCY, String(percent));
});

blurToggle.checked = (localStorage.getItem(STORAGE_BLUR) ?? "1") === "1";
void invoke("set_blur", { enabled: blurToggle.checked });
blurToggle.addEventListener("change", () => {
  localStorage.setItem(STORAGE_BLUR, blurToggle.checked ? "1" : "0");
  void invoke("set_blur", { enabled: blurToggle.checked });
});

const STATE_LABEL: Record<OrbState, string> = {
  idle: "Ready",
  thinking: "Thinking",
  speaking: "Replying",
};

let currentState: OrbState = "idle";
function setState(next: OrbState) {
  currentState = next;
  orb.setState(next);
  widget.dataset.state = next;
  statusText.textContent = STATE_LABEL[next];
  if (next === "idle" || next === "speaking") hideTyping();
}

// ---------- window mode: full chat view <-> top-right widget ----------
type Mode = "full" | "widget";
let mode: Mode = localStorage.getItem(STORAGE_MODE) === "widget" ? "widget" : "full";
document.body.dataset.mode = mode;

async function setMode(next: Mode) {
  mode = next;
  localStorage.setItem(STORAGE_MODE, next);
  document.body.dataset.mode = next;
  if (next === "widget") closeSettings();
  showCaption("");
  await invoke("set_mode", { mode: next });
}

// ---------- chat transcript ----------
const typingEl = document.createElement("div");
typingEl.className = "typing hidden";
typingEl.innerHTML = "<i></i><i></i><i></i>";
messagesEl.appendChild(typingEl);

function scrollToEnd() {
  messagesEl.scrollTop = messagesEl.scrollHeight;
}

// The reply currently being typed out, if any.
let currentTyper: Typer | null = null;

// Eases the chat toward the bottom while a reply types, instead of snapping
// on every character.
function followEnd() {
  const target = messagesEl.scrollHeight - messagesEl.clientHeight;
  const distance = target - messagesEl.scrollTop;
  messagesEl.scrollTop = Math.abs(distance) < 1 ? target : messagesEl.scrollTop + distance * 0.25;
}

function addMessage(
  role: "user" | "assistant" | "error",
  text: string,
  typing?: { onTick?: () => void; onDone?: () => void },
) {
  // A new message completes whatever was still being typed.
  currentTyper?.finish();
  currentTyper = null;

  const el = document.createElement("div");
  el.className = `msg ${role}`;
  if (role === "assistant") renderMarkdown(text, el);
  else el.textContent = text;
  messagesEl.insertBefore(el, typingEl);
  document.body.classList.add("has-messages");
  if (role !== "user") hideTyping();
  scrollToEnd();
  if (role === "assistant") {
    currentTyper = typewrite(
      el,
      () => {
        followEnd();
        typing?.onTick?.();
      },
      () => {
        scrollToEnd();
        typing?.onDone?.();
      },
    );
  }
}

// ---------- question and approval cards ----------
// The agent can pause and ask the user something (a choice, a missing detail)
// or ask permission before a tool that changes something. Both show up as a
// card in the chat. While one is waiting, whatever the user types answers it
// instead of starting a new request.
interface PendingCard {
  id: string;
  kind: "ask" | "approval";
  el: HTMLDivElement;
}
let pendingCard: PendingCard | null = null;

const YES_RE = /^\s*(yes|yeah|yep|yup|sure|ok|okay|allow|approve|confirm|send( it)?|go ahead|do it|please do)\b/i;
const NO_RE = /^\s*(no|nope|nah|cancel|don'?t|stop|decline|deny)\b/i;

function finishCard(
  card: PendingCard,
  payload: { text: string } | { allow: boolean; note?: string },
  summary: string,
) {
  if (pendingCard !== card) return;
  pendingCard = null;
  card.el.className = "card answered";
  card.el.textContent = summary;
  void answerCard(card.id, payload);
  setState("thinking");
  showTyping();
  scrollToEnd();
}

function answerPendingCard(text: string) {
  const card = pendingCard;
  if (!card) return;
  const answer = text.trim();
  if (card.kind === "ask") {
    finishCard(card, { text: answer }, `You answered: ${answer}`);
  } else if (YES_RE.test(answer)) {
    finishCard(card, { allow: true }, "Allowed");
  } else if (NO_RE.test(answer)) {
    finishCard(card, { allow: false }, "Declined");
  } else {
    finishCard(card, { allow: false, note: answer }, `Declined: ${answer}`);
  }
}

function cardShell(kind: PendingCard["kind"], title: string): { card: PendingCard; el: HTMLDivElement } {
  const el = document.createElement("div");
  el.className = `card ${kind}`;
  const heading = document.createElement("div");
  heading.className = "card-q";
  heading.textContent = title;
  el.append(heading);
  const card: PendingCard = { id: "", kind, el };
  return { card, el };
}

function addTextRow(
  el: HTMLDivElement,
  placeholder: string,
  buttonLabel: string,
  onSubmit: (text: string) => void,
) {
  const form = document.createElement("form");
  form.className = "card-text";
  const input = document.createElement("input");
  input.type = "text";
  input.placeholder = placeholder;
  input.autocomplete = "off";
  const send = document.createElement("button");
  send.type = "submit";
  send.textContent = buttonLabel;
  form.append(input, send);
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    if (input.value.trim()) onSubmit(input.value.trim());
  });
  el.append(form);
}

function showCard(card: PendingCard) {
  pendingCard = card;
  hideTyping();
  setState("idle");
  messagesEl.insertBefore(card.el, typingEl);
  document.body.classList.add("has-messages");
  scrollToEnd();
}

function showAskCard(event: Extract<AgentEvent, { kind: "ask_user" }>) {
  const { card, el } = cardShell("ask", event.question);
  card.id = event.id;

  if (event.options.length) {
    const list = document.createElement("div");
    list.className = "card-options";
    for (const option of event.options) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "chip";
      button.textContent = option.label;
      if (option.description) {
        const detail = document.createElement("small");
        detail.textContent = option.description;
        button.append(detail);
      }
      button.addEventListener("click", () => finishCard(card, { text: option.label }, `You chose: ${option.label}`));
      list.append(button);
    }
    el.append(list);
  }
  addTextRow(el, event.options.length ? "Or type your own answer" : "Type your answer", "Send", (text) =>
    finishCard(card, { text }, `You answered: ${text}`),
  );

  showCard(card);
}

// Free-text fields get a taller, scrollable box in approval cards.
const LONG_KEYS = new Set(["body", "content", "text", "description", "message"]);

function humanizeKey(key: string): string {
  const text = key.replace(/_/g, " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

function showApprovalCard(event: Extract<AgentEvent, { kind: "approval_request" }>) {
  const args = event.args ?? {};
  const isEmail = "to" in args && ("subject" in args || "body" in args);
  const title = isEmail ? "Send this email?" : `${event.label}?`;
  const { card, el } = cardShell("approval", title);
  card.id = event.id;

  const details = document.createElement("div");
  details.className = "card-details";
  for (const [key, value] of Object.entries(args)) {
    if (value === null || value === undefined || value === "") continue;
    const row = document.createElement("div");
    row.className = LONG_KEYS.has(key) ? "card-row long" : "card-row";
    const label = document.createElement("span");
    label.textContent = humanizeKey(key);
    const text = document.createElement("div");
    text.textContent = typeof value === "string" ? value : JSON.stringify(value);
    row.append(label, text);
    details.append(row);
  }
  if (details.childElementCount) el.append(details);

  const actions = document.createElement("div");
  actions.className = "card-actions";
  const allow = document.createElement("button");
  allow.type = "button";
  allow.className = "primary";
  allow.textContent = isEmail ? "Send" : "Allow";
  allow.addEventListener("click", () => finishCard(card, { allow: true }, isEmail ? "Sent" : "Allowed"));
  const decline = document.createElement("button");
  decline.type = "button";
  decline.textContent = "Decline";
  decline.addEventListener("click", () => finishCard(card, { allow: false }, "Declined"));
  actions.append(allow, decline);
  el.append(actions);

  addTextRow(el, "Or say what to change", "Revise", (note) => finishCard(card, { allow: false, note }, `Declined: ${note}`));

  showCard(card);
  // Focus the confirm button so Enter or Space is enough to say yes.
  allow.focus({ preventScroll: true });
}

// ---------- tool progress rows ----------
// Each tool call is a row: the brand's logo (loaded first) with a ring that
// spins while it works, then the plain-language title. Internal tool and
// service names never reach the screen.
const ICONS: Record<string, string> = {
  globe:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c3 3.2 3 14.8 0 18M12 3c-3 3.2-3 14.8 0 18"/></svg>',
  memory:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><rect x="6" y="6" width="12" height="12" rx="2.5"/><path d="M9 3v3M15 3v3M9 18v3M15 18v3M3 9h3M3 15h3M18 9h3M18 15h3"/></svg>',
  clock:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg>',
  tool:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M14.5 6.5a4 4 0 0 0-5 5L4 17l3 3 5.5-5.5a4 4 0 0 0 5-5l-2.5 2.5-2.5-.5-.5-2.5z"/></svg>',
};

// Logos are image files in src/assets/logos, bundled with the app. A file's
// name (without extension) is its key, e.g. gmail.png or wikipedia.org.svg.
// Vite resolves the list at build time, so a missing logo is known instantly.
const LOGO_FILES = import.meta.glob<string>(["./assets/logos/*.{png,svg,webp,jpg,jpeg}", "!./assets/logos/leo-app-icon.png"], {
  eager: true,
  query: "?url",
  import: "default",
});
const LOGOS = new Map<string, string>();
for (const [path, url] of Object.entries(LOGO_FILES)) {
  const stem = path.slice(path.lastIndexOf("/") + 1).replace(/\.[^.]+$/, "");
  LOGOS.set(stem.toLowerCase(), url);
}

function findLogo(brand: Brand | undefined): string | null {
  for (const key of brand?.keys ?? []) {
    const url = LOGOS.get(key.toLowerCase());
    if (url) return url;
  }
  return null;
}

// Resolves once the image has decoded (so the row never pops in half-drawn),
// or with null if it fails.
const logoCache = new Map<string, Promise<string | null>>();
function preloadLogo(url: string | null): Promise<string | null> {
  if (!url) return Promise.resolve(null);
  let cached = logoCache.get(url);
  if (!cached) {
    cached = new Promise((resolve) => {
      const image = new Image();
      image.onload = () => resolve(url);
      image.onerror = () => resolve(null);
      image.src = url;
    });
    logoCache.set(url, cached);
  }
  return cached;
}

function buildToolRow(title: string, detail: string | null | undefined, logo: string | null, icon: string): HTMLDivElement {
  const row = document.createElement("div");
  row.className = "tool-step running";

  const mark = document.createElement("div");
  mark.className = "tool-logo";
  if (logo) {
    const image = document.createElement("img");
    image.src = logo;
    image.alt = "";
    mark.append(image);
  } else {
    mark.innerHTML = ICONS[icon] ?? ICONS.tool;
  }

  const text = document.createElement("div");
  text.className = "tool-text";
  const heading = document.createElement("span");
  heading.className = "tool-title";
  heading.textContent = title;
  text.append(heading);
  if (detail) {
    const sub = document.createElement("span");
    sub.className = "tool-detail";
    sub.textContent = detail;
    text.append(sub);
  }
  row.append(mark, text);
  return row;
}

type ToolResultEvent = Extract<AgentEvent, { kind: "tool_result" }>;
interface ToolRowState {
  row?: HTMLDivElement;
  result?: ToolResultEvent;
}
const toolRows = new Map<string, ToolRowState>();

function finishToolRow(row: HTMLDivElement, result: ToolResultEvent) {
  row.classList.remove("running");
  row.classList.add(result.ok ? "done" : "failed");
  if (!result.ok && result.error) {
    // Say why it failed, so a broken tool isn't silent.
    const reason = result.error.length > 140 ? `${result.error.slice(0, 140)}…` : result.error;
    const note = document.createElement("span");
    note.className = "tool-error";
    note.textContent = reason;
    row.querySelector(".tool-text")?.append(note);
    row.title = result.error;
  }
}

void onAgentEvent(async (event) => {
  // Progress events describe the plan's steps; they are for the debug timing
  // run only. The chat shows tool rows, never the plan.
  if (event.kind === "progress") return;
  if (event.kind === "ask_user") {
    showAskCard(event);
    return;
  }
  if (event.kind === "approval_request") {
    showApprovalCard(event);
    return;
  }

  if (event.kind === "tool_start") {
    const state: ToolRowState = {};
    toolRows.set(event.id, state);
    if (mode === "widget") showCaption(event.label);

    // The logo is loaded first, so the row appears with it already in place.
    const logo = await preloadLogo(findLogo(event.brand));
    if (toolRows.get(event.id) !== state) return; // chat was cleared meanwhile

    const row = buildToolRow(event.label, event.detail, logo, event.brand?.icon ?? "tool");
    state.row = row;
    messagesEl.insertBefore(row, typingEl);
    document.body.classList.add("has-messages");
    scrollToEnd();
    // The tool may have finished while the logo was loading.
    if (state.result) finishToolRow(row, state.result);
    return;
  }

  const state = toolRows.get(event.id);
  if (!state) return;
  if (state.row) finishToolRow(state.row, event);
  else state.result = event;
});

// Links in replies open in the default browser instead of navigating the
// app window away.
messagesEl.addEventListener("click", (event) => {
  const link = (event.target as HTMLElement).closest("a");
  if (!link) return;
  event.preventDefault();
  void openUrl(link.href);
});

function showTyping() {
  typingEl.classList.remove("hidden");
  scrollToEnd();
}

function hideTyping() {
  typingEl.classList.add("hidden");
}

function clearChat() {
  currentTyper?.finish();
  currentTyper = null;
  messagesEl.querySelectorAll(".msg, .tool-step, .card").forEach((el) => el.remove());
  pendingCard = null;
  toolRows.clear();
  document.body.classList.remove("has-messages");
  hideTyping();
  llm.reset();
}

function errorMessage(err: unknown): string {
  // Tauri's invoke() rejects with the raw String from a Rust `Err(String)`,
  // not an Error instance, so that case has to be checked explicitly.
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  return "Something went wrong.";
}

let captionTimer = 0;
function showCaption(text: string, autoHideMs = 0) {
  window.clearTimeout(captionTimer);
  caption.textContent = text;
  caption.classList.toggle("hidden", !text);
  if (text && autoHideMs > 0) {
    captionTimer = window.setTimeout(() => showCaption(""), autoHideMs);
  }
}

// ---------- settings window ----------
interface McpServerStatus {
  name: string;
  state: "starting" | "ready" | "error";
  tools: number;
  message: string;
  needs?: string;
}

function setChip(el: HTMLElement, on: boolean, onText: string, offText: string) {
  el.classList.toggle("on", on);
  el.textContent = on ? onText : offText;
}

// Shows, beside each section title, whether that connection is working.
async function refreshStatus() {
  setChip(chipAi, Boolean(getApiKey() && getModel()), "Ready", "Needs a key and a model");
  try {
    const [memoryReady, searchReady, servers] = await Promise.all([
      invoke<boolean>("memory_is_configured"),
      invoke<boolean>("web_is_configured"),
      invoke<McpServerStatus[]>("mcp_status"),
    ]);
    setChip(chipMemory, memoryReady, "Connected", "Not set");
    setChip(chipSearch, searchReady, "Connected", "Not set");
    const google = servers.find((s) => s.name === "google");
    if (!google) setChip(chipGoogle, false, "", "Not set up");
    else if (google.state === "ready" && google.needs) setChip(chipGoogle, false, "", "Add your email below");
    else if (google.state === "ready") setChip(chipGoogle, true, "Connected", "");
    else if (google.state === "starting") setChip(chipGoogle, false, "", "Connecting…");
    else setChip(chipGoogle, false, "", "Couldn't connect");
  } catch {
    // Status is a nicety; the form still works without it.
  }
}

function fillSettings() {
  apiKeyInput.value = getApiKey();
  modelInput.value = getModel();
  plannerModelInput.value = getPlannerModel();
  verifierModelInput.value = getVerifierModel();
  memoryKeyInput.value = localStorage.getItem(STORAGE_MEMORY_KEY) ?? "";
  searchKeyInput.value = localStorage.getItem(STORAGE_SEARCH_KEY) ?? "";
  googleEmailInput.value = localStorage.getItem(STORAGE_GOOGLE_EMAIL) ?? "";
  // Keys start hidden each time the window opens.
  for (const input of [apiKeyInput, memoryKeyInput, searchKeyInput]) input.type = "password";
}

function openSettings(message = "") {
  if (mode !== "full") return;
  fillSettings();
  settingsStatus.textContent = message;
  settingsModal.classList.remove("hidden");
  void refreshStatus();
  (getApiKey() ? modelInput : apiKeyInput).focus();
}

function closeSettings() {
  settingsModal.classList.add("hidden");
}

settingsBtn.addEventListener("click", () => {
  if (settingsModal.classList.contains("hidden")) openSettings();
  else closeSettings();
});
settingsClose.addEventListener("click", closeSettings);
settingsModal.addEventListener("click", (event) => {
  // A click on the dimmed backdrop closes the window.
  if (event.target === settingsModal) closeSettings();
  // "Get a key" links open in the default browser.
  const link = (event.target as HTMLElement).closest<HTMLAnchorElement>("a[data-ext]");
  if (link) {
    event.preventDefault();
    void openUrl(link.href);
  }
});
window.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !settingsModal.classList.contains("hidden")) closeSettings();
});

// The eye button beside each key shows or hides it.
document.querySelectorAll<HTMLButtonElement>(".reveal").forEach((button) => {
  button.addEventListener("click", () => {
    const input = document.getElementById(button.dataset.target ?? "") as HTMLInputElement | null;
    if (input) input.type = input.type === "password" ? "text" : "password";
  });
});

let savedTimer = 0;
saveBtn.addEventListener("click", async () => {
  localStorage.setItem(STORAGE_KEY, apiKeyInput.value.trim());
  localStorage.setItem(STORAGE_MODEL, modelInput.value.trim());
  localStorage.setItem(STORAGE_PLANNER_MODEL, plannerModelInput.value.trim());
  localStorage.setItem(STORAGE_VERIFIER_MODEL, verifierModelInput.value.trim());
  localStorage.setItem(STORAGE_MEMORY_KEY, memoryKeyInput.value.trim());
  localStorage.setItem(STORAGE_SEARCH_KEY, searchKeyInput.value.trim());
  localStorage.setItem(STORAGE_GOOGLE_EMAIL, googleEmailInput.value.trim());
  try {
    await Promise.all([configureSearch(true), configureMemory(true), configureGoogleEmail(true)]);
  } catch (err) {
    settingsStatus.textContent = errorMessage(err);
    return;
  }
  await refreshStatus();
  settingsStatus.textContent = "Saved.";
  window.clearTimeout(savedTimer);
  savedTimer = window.setTimeout(() => (settingsStatus.textContent = ""), 2500);
});

minimizeBtn.addEventListener("click", () => void setMode("widget"));
expandBtn.addEventListener("click", () => void setMode("full"));
newChatBtn.addEventListener("click", clearChat);

closeBtn.addEventListener("click", () => {
  void getCurrentWindow().close();
});

// ---------- sending ----------
// Bumped on every send; a reply from an older send is ignored.
let turnGen = 0;

async function sendMessage(text: string) {
  // A waiting question or approval card takes the text as its answer.
  if (pendingCard) {
    answerPendingCard(text);
    return;
  }

  // A new message takes over from whatever was still running.
  llm.cancel();
  const myTurn = ++turnGen;
  orb.setLevel(0);

  addMessage("user", text);
  showTyping();
  setState("thinking");
  try {
    const reply = await llm.ask(text);
    if (myTurn !== turnGen) return;
    // The orb ripples to the rhythm of the text as it appears and settles
    // when it is done.
    setState("speaking");
    addMessage("assistant", reply, {
      onTick: () => orb.setLevel(0.22 + 0.14 * Math.sin(performance.now() / 85)),
      onDone: () => {
        orb.setLevel(0);
        if (currentState === "speaking") setState("idle");
      },
    });
  } catch (err) {
    // "cancelled" means a newer message took over; it owns the UI now.
    if (myTurn !== turnGen || errorMessage(err) === "cancelled") return;
    console.error(err);
    addMessage("error", errorMessage(err));
    setState("idle");
  }
}

function autoGrowInput() {
  composerInput.style.height = "auto";
  composerInput.style.height = `${Math.min(composerInput.scrollHeight, 120)}px`;
  sendBtn.disabled = composerInput.value.trim() === "";
}

composerInput.addEventListener("input", autoGrowInput);
composerInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
    e.preventDefault();
    composer.requestSubmit();
  }
});

composer.addEventListener("submit", (e) => {
  e.preventDefault();
  const text = composerInput.value.trim();
  if (!text) return;
  if (!pendingCard && (!getApiKey() || !getModel())) {
    openSettings("Add your OpenRouter key and a model name to start chatting.");
    return;
  }
  composerInput.value = "";
  autoGrowInput();
  void sendMessage(text);
});

// ---------- dragging the minimised orb ----------
// In widget mode the orb can be dragged from anywhere on it. A press that
// stays put is a click (it opens the chat); one that moves hands over to the
// OS window drag, and the window remembers where it was dropped.
let dragStart: { x: number; y: number } | null = null;
let dragged = false;

widget.addEventListener("pointerdown", (event) => {
  dragged = false;
  if (mode !== "widget" || event.button !== 0 || (event.target as HTMLElement).closest("button")) return;
  dragStart = { x: event.clientX, y: event.clientY };
});

window.addEventListener("pointermove", (event) => {
  if (!dragStart || mode !== "widget") return;
  if (Math.hypot(event.clientX - dragStart.x, event.clientY - dragStart.y) > 5) {
    dragStart = null;
    dragged = true;
    void getCurrentWindow().startDragging();
  }
});

window.addEventListener("pointerup", () => {
  dragStart = null;
});

// Hovering over the orb makes it ripple, for as long as the pointer stays.
canvas.addEventListener("pointerenter", () => orb.setHover(true));
canvas.addEventListener("pointerleave", () => orb.setHover(false));

canvas.addEventListener("click", () => {
  if (dragged) {
    // The press was a drag, not a click.
    dragged = false;
    return;
  }
  if (mode === "widget") void setMode("full");
});

// ---------- start ----------
setState("idle");
if (mode === "widget") void invoke("set_mode", { mode });
else if (!getApiKey() || !getModel()) {
  openSettings("Welcome! Add your OpenRouter key and a model to get started.");
}

import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import { SiriOrb, type OrbState } from "./orb";
import { AgentClient, answerCard, onAgentEvent, type AgentEvent, type Brand } from "./agent";
import { renderMarkdown } from "./markdown";
import { greeting } from "./greeting";
import { BUG_FIXES_LINE, RELEASES, releasesSince, type Release } from "./changelog";
import { createStreamTyper, typewrite, type StreamTyper, type Typer } from "./typewriter";
import { playWarp, type SpaceRun } from "./space";
import { VoiceControl } from "./voiceControl";
import { AttachmentTray, isImageFile, renderBubbleImages } from "./attachments";

const STORAGE_KEY = "leo.openrouter.apiKey";
const STORAGE_MODEL = "leo.openrouter.model";
const STORAGE_PLANNER_MODEL = "leo.openrouter.plannerModel";
const STORAGE_VERIFIER_MODEL = "leo.openrouter.verifierModel";
const STORAGE_MEMORY_KEY = "leo.memorylake.apiKey";
const STORAGE_SEARCH_KEY = "leo.tavily.key";
const STORAGE_GOOGLE_EMAIL = "leo.google.email";
const STORAGE_MODE = "leo.mode";
const STORAGE_SETUP = "leo.setupShown";
const STORAGE_BLUR = "leo.blur";
const STORAGE_SHORTCUT = "leo.shortcut";
const STORAGE_LAST_VERSION = "leo.lastVersion";
const STORAGE_PROFILE_PROMPTED = "leo.profilePrompted";
const STORAGE_TRANSPARENCY = "leo.transparency";
const STORAGE_CALENDAR_AUTO = "leo.autoApproveCalendar";

const canvas = document.querySelector<HTMLCanvasElement>("#orb")!;
const widget = document.querySelector<HTMLDivElement>("#widget")!;
const caption = document.querySelector<HTMLDivElement>("#caption")!;
const settingsBtn = document.querySelector<HTMLButtonElement>("#settings-btn")!;
const closeBtn = document.querySelector<HTMLButtonElement>("#close-btn")!;
const minimizeBtn = document.querySelector<HTMLButtonElement>("#minimize-btn")!;
const expandBtn = document.querySelector<HTMLButtonElement>("#expand-btn")!;
const newChatBtn = document.querySelector<HTMLButtonElement>("#new-chat-btn")!;
const historyBtn = document.querySelector<HTMLButtonElement>("#history-btn")!;
const historyPanel = document.querySelector<HTMLDivElement>("#history")!;
const historyList = document.querySelector<HTMLDivElement>("#history-list")!;
const historyClose = document.querySelector<HTMLButtonElement>("#history-close")!;
const statusText = document.querySelector<HTMLSpanElement>("#status-text")!;
const messagesEl = document.querySelector<HTMLDivElement>("#messages")!;
const composer = document.querySelector<HTMLFormElement>("#composer")!;
const composerInput = document.querySelector<HTMLTextAreaElement>("#composer-input")!;
const sendBtn = document.querySelector<HTMLButtonElement>("#send-btn")!;
const attachBtn = document.querySelector<HTMLButtonElement>("#attach-btn")!;
const attachInput = document.querySelector<HTMLInputElement>("#attach-input")!;
const attachTray = document.querySelector<HTMLDivElement>("#attachments")!;

const settingsModal = document.querySelector<HTMLDivElement>("#settings")!;
const settingsClose = document.querySelector<HTMLButtonElement>("#settings-close")!;
const apiKeyInput = document.querySelector<HTMLInputElement>("#api-key")!;
const modelInput = document.querySelector<HTMLInputElement>("#model-name")!;
const plannerModelInput = document.querySelector<HTMLInputElement>("#planner-model")!;
const verifierModelInput = document.querySelector<HTMLInputElement>("#verifier-model")!;
const memoryKeyInput = document.querySelector<HTMLInputElement>("#memory-key")!;
const searchKeyInput = document.querySelector<HTMLInputElement>("#search-key")!;
const googleEmailInput = document.querySelector<HTMLInputElement>("#google-email")!;
const shortcutInput = document.querySelector<HTMLInputElement>("#shortcut-input")!;
const shortcutClear = document.querySelector<HTMLButtonElement>("#shortcut-clear")!;
const blurToggle = document.querySelector<HTMLInputElement>("#blur-toggle")!;
const calendarAuto = document.querySelector<HTMLInputElement>("#calendar-auto")!;
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

// Voice is an input/output layer around the same text path: spoken turns go
// through sendMessage() like typed ones, and Leo's streamed reply is spoken as
// it arrives. Cancelling here is the single place that stops a reply.
let deltasOpen = true;
// True from the moment a message is sent until its reply has arrived.
let turnBusy = false;
const voice = new VoiceControl({
  submit: (text) => sendMessage(text),
  cancelResponse: () => {
    // A question or approval card means Leo is waiting for an answer, not generating.
    if (pendingCard) return;
    deltasOpen = false; // late pieces of the old reply are ignored
    turnBusy = false;
    llm.cancel();
    turnGen++;
    finishLive();
    orb.setLevel(0);
    hideTyping();
    setState("idle");
  },
  setOrbState: (state) => setState(state),
  setOrbLevel: (level) => orb.setLevel(level),
  showError: (message) => addMessage("error", message),
});

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
// While it downloads, a flight through space plays for 15 seconds (the
// updater waits that long before installing); then the app restarts and the
// launch animation plays in the new version. If the update fails, the flight
// ends and the launch animation plays right here instead.
const spaceCanvas = document.querySelector<HTMLCanvasElement>("#space")!;
const spaceLine = document.querySelector<HTMLDivElement>("#space-line")!;
const spaceBar = document.querySelector<HTMLElement>("#space-bar i")!;
const UPDATE_ANIMATION_MS = 15000;
const INTRO_MS = 2100;
const SPACE_LINES: Array<[number, string]> = [
  [0, "Preparing for launch"],
  [0.1, "Engaging thrusters"],
  [0.28, "Crossing the void"],
  [0.55, "Gathering new light"],
  [0.78, "Slowing for arrival"],
  [0.92, "Almost there"],
];
let spaceRun: SpaceRun | null = null;
let warping = false;

// Normal launch: just the orb floating up from the bottom (CSS only). The
// starfield is reserved for updating.
function startIntro() {
  document.body.classList.add("intro");
  orb.setHover(true);
  window.setTimeout(() => {
    document.body.classList.remove("intro");
    orb.setHover(false);
  }, INTRO_MS + 100);
}

function startUpdateWarp(title = "Updating Leo") {
  if (warping) return;
  warping = true;
  spaceRun?.stop();
  document.body.classList.remove("intro");
  // The flight needs the whole window, so a minimised orb opens up first.
  if (mode === "widget") void setMode("full", true);
  closeSettings();
  showCaption("");
  document.body.classList.add("warping");
  setState("thinking");
  statusText.textContent = `${title}…`;
  document.querySelector<HTMLElement>("#space-title")!.textContent = title;
  orb.setHover(true);
  spaceRun = playWarp(spaceCanvas, UPDATE_ANIMATION_MS, {
    onProgress(p) {
      spaceBar.style.transform = `scaleX(${p})`;
      const line = [...SPACE_LINES].reverse().find(([from]) => p >= from);
      if (line) spaceLine.textContent = line[1];
    },
  });
}

function endUpdateWarp() {
  if (!warping) return;
  warping = false;
  document.querySelector<HTMLButtonElement>("#update-btn")!.disabled = false;
  spaceRun?.stop(() => {
    document.body.classList.remove("warping");
    statusText.textContent = "Ready";
    setState("idle");
    startIntro();
  });
}

void listen<string>("app://updating", () => startUpdateWarp());
void listen("app://update-failed", endUpdateWarp);

// Development only: Ctrl+Shift+U previews the update flight and then the launch.
if (import.meta.env.DEV) {
  window.addEventListener("keydown", (event) => {
    if (event.ctrlKey && event.shiftKey && event.key.toLowerCase() === "u") {
      startUpdateWarp();
      window.setTimeout(endUpdateWarp, UPDATE_ANIMATION_MS + 300);
    }
  });
}

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
// Calendar events without an approval card: off unless the user chose it. The
// screen keeps the choice and tells the backend at every start.
calendarAuto.checked = localStorage.getItem(STORAGE_CALENDAR_AUTO) === "1";
void invoke("set_calendar_auto_approve", { enabled: calendarAuto.checked });
calendarAuto.addEventListener("change", () => {
  localStorage.setItem(STORAGE_CALENDAR_AUTO, calendarAuto.checked ? "1" : "0");
  void invoke("set_calendar_auto_approve", { enabled: calendarAuto.checked });
});
// The user asked Leo in chat (and confirmed on the card): keep the switch in step.
void listen<boolean>("prefs://calendar", (event) => {
  calendarAuto.checked = event.payload;
  localStorage.setItem(STORAGE_CALENDAR_AUTO, event.payload ? "1" : "0");
});

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
// Leo always starts as the small orb; the chat opens when asked.
let mode: Mode = "widget";
document.body.dataset.mode = mode;

// Minimising is the opening played backwards, step by step, in the opposite
// order (the animations are the same ones, run in reverse): the interface fades
// out and the big orb sinks; the window closes to a line and then a point; the
// window shrinks to the small orb window around that point and the point lights
// up; it flies to where the orb lives; its flare calms down; and it grows into
// the orb.
const SLOW = 1.15; // same factor as --slow in styles.css
const SHINE_MS = 320 * SLOW; // matches dot-shine-small
const LAND_MS = 90 * SLOW; // matches dot-out-small
const IMPLODE_MS = 420 * SLOW; // matches orb-implode
const CRT_ON_MS = 639.6 * SLOW; // matches crt-on (the vertical part is 12% slower)
// The end of the opening (orb rise 2.1 s after a 50 ms offset, interface fade
// 0.9 s) played backwards; these are not scaled by SLOW, like the opening.
const INTRO_REVERSED_MS = 2150;
// The minimise is 25% shorter than the opening it mirrors (--mfast in styles.css,
// MINIMISE_SPEED in lib.rs for the flight): every wait in it is scaled by this.
const MFAST = 0.75;
let minimizing = false;

const sleep = (ms: number) => new Promise<void>((resolve) => window.setTimeout(resolve, ms));
const nextFrame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

/** After the window shrinks, the page can still be laid out at the old size
    for a frame or two; wait until it has caught up so the orb grows in place. */
async function viewportSettled(big = false, maxMs = 700) {
  const started = performance.now();
  const ready = () => (big ? window.innerWidth >= 380 : window.innerWidth <= 260);
  while (!ready() && performance.now() - started < maxMs) await nextFrame();
  await nextFrame();
  await nextFrame();
}

async function playMinimize() {
  const cls = document.body.classList;
  closeSettings();
  closeHistory();
  closeWhatsNew();
  showCaption("");
  // 1. The end of the opening backwards: the interface fades out, the orb sinks.
  cls.add("sinking");
  orb.setHover(true);
  await sleep(INTRO_REVERSED_MS * MFAST);
  orb.setHover(false);
  // 2. The window closes to a line, then to a point (the opening of the window
  //    backwards). The blur goes first, as it came last.
  cls.remove("sinking");
  cls.add("collapsing");
  void invoke("glass_off");
  await sleep(CRT_ON_MS * MFAST);
  // 3. The window becomes the small orb window around the point, which lights up.
  await invoke("shrink_to_orb_window");
  document.body.dataset.mode = "widget";
  await viewportSettled();
  cls.add("dot-in");
  await sleep(LAND_MS * MFAST);
  // 4. The point flies back to the orb's place.
  await invoke("fly_to_orb");
  // 5. Its flare calms down.
  cls.remove("dot-in");
  cls.add("dot-unshine");
  await sleep(SHINE_MS * MFAST);
  // 6. And it grows into the orb.
  cls.remove("collapsing", "dot-unshine");
  cls.add("unimploding");
  await sleep(IMPLODE_MS * MFAST);
}

// Opening the chat plays the same thing backwards: the orb shrinks to a bright
// point, the point flies to the middle of the screen, opens into a line, and
// the line opens into the chat window.
async function playMaximize() {
  const cls = document.body.classList;
  closeSettings();
  showCaption("");
  cls.add("imploding");
  await sleep(IMPLODE_MS);
  cls.add("shining");
  await sleep(SHINE_MS);
  await invoke("fly_to_center");
  cls.add("landing");
  await sleep(LAND_MS);
}

/** `fast` skips the animation (used when Leo only steps aside for a moment). */
async function setMode(next: Mode, fast = false) {
  if (minimizing) return;
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const animated = !warping && !fast && !reduced;
  const closing = animated && next === "widget" && mode === "full";
  const opening = animated && next === "full" && mode === "widget";
  if (closing || opening) {
    minimizing = true;
    try {
      await (closing ? playMinimize() : playMaximize());
    } catch {
      // If the flight failed the switch below still puts things in place.
    }
  }
  // Opening the chat without the animation plays the rise-up instead. It
  // starts invisible, so the window resizing underneath is not seen.
  const rising = next === "full" && mode === "widget" && !warping && !opening;
  const cls = document.body.classList;
  if (opening) cls.add("expanding");
  mode = next;
  localStorage.setItem(STORAGE_MODE, next);
  document.body.dataset.mode = next;
  if (rising) startIntro();
  if (next === "full") {
    renderGreeting();
    flushWhatsNew();
    window.setTimeout(maybePromptProfile, 3500);
  }
  if (next === "widget") closeSettings();
  showCaption("");
  try {
    await invoke("set_mode", { mode: next, quiet: opening });
  } finally {
    if (closing) {
      // The orb has grown out of the point in its place; clear the sequence.
      document.body.classList.remove("sinking", "collapsing", "dot-in", "dot-unshine", "unimploding");
      minimizing = false;
    } else if (opening) {
      // The point opens into a line, and the line into the chat window.
      await viewportSettled(true);
      cls.remove("imploding", "shining", "landing");
      cls.add("crt-on");
      await sleep(CRT_ON_MS);
      void invoke("glass_on");
      // The window has reached its full size: now the big orb floats up from
      // the bottom, and the rest of the interface fades in after it.
      cls.remove("expanding", "crt-on");
      startIntro();
      minimizing = false;
    }
  }
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

// A reply that is arriving live from the model (streaming). Its text is
// queued into a typer that writes it out character by character, so it reads
// like any other reply while the model is still working.
let liveEl: HTMLDivElement | null = null;
let liveTyper: StreamTyper | null = null;

function addLiveText(text: string) {
  if (!liveEl) {
    currentTyper?.finish();
    currentTyper = null;
    liveEl = document.createElement("div");
    liveEl.className = "msg assistant";
    messagesEl.insertBefore(liveEl, typingEl);
    document.body.classList.add("has-messages");
    hideTyping();
    setState("speaking");
    liveTyper = createStreamTyper(liveEl, () => {
      followEnd();
      if (!voice.ownsOrb) orb.setLevel(0.22 + 0.14 * Math.sin(performance.now() / 85));
    });
  }
  liveTyper?.push(text);
}

/** Throws away a live reply (the model went on to call a tool, or the run failed). */
function dropLive() {
  liveTyper?.finish();
  liveEl?.remove();
  liveEl = null;
  liveTyper = null;
}

/** A new message arrives while a reply is still typing: show what there is and move on. */
function finishLive() {
  liveTyper?.finish();
  liveEl = null;
  liveTyper = null;
}

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
  typing?: { onTick?: () => void; onDone?: () => void; instant?: boolean },
): HTMLDivElement {
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
  if (role === "assistant" && !typing?.instant) {
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
  return el;
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
const NO_RE = /^\s*(no|nope|nah|cancel|don'?t|do not|stop|decline|deny|skip|never ?mind|forget it|leave it|not now|not needed)\b/i;
// A short reply that says it is not wanted ("it's not needed", "I don't need that") is a plain no.
const NO_ANYWHERE_RE = /\b(no need|not needed|don'?t need|do not need|don'?t want|do not want|don'?t save|do not save)\b/i;

/** A plain "no" (sent without a note, so it cannot be mistaken for "change it"). */
function isRefusal(text: string): boolean {
  const t = text.trim();
  return NO_RE.test(t) || (t.split(/\s+/).length <= 6 && NO_ANYWHERE_RE.test(t));
}

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
  } else if (isRefusal(answer)) {
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

  voice.runtime.say(event.question);
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

  addTextRow(el, "Or say what to change", "Revise", (note) =>
    isRefusal(note) ? finishCard(card, { allow: false }, "Declined") : finishCard(card, { allow: false, note }, `Declined: ${note}`),
  );

  // With voice on, the question is read out; for an email, who it goes to and what it is about.
  const to = typeof args.to === "string" ? args.to : "";
  const subject = typeof args.subject === "string" ? args.subject : "";
  voice.runtime.say(isEmail && to ? `${title} To ${to}${subject ? `, subject: ${subject}` : ""}.` : title);
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
  eye:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/></svg>',
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
  if (event.kind === "delta") {
    if (!deltasOpen) return;
    addLiveText(event.text);
    voice.runtime.feedLeoText(event.text);
    return;
  }
  if (event.kind === "narrate") {
    // Spoken only: the tool row on screen already says the same in writing.
    if (!deltasOpen) return;
    voice.runtime.narrate(event.text);
    return;
  }
  if (event.kind === "say") {
    // A line Leo says before a step of a plan; it stays as a message of its own.
    if (!deltasOpen) return;
    addMessage("assistant", event.text, {
      onTick: () => {
        if (!voice.ownsOrb) orb.setLevel(0.22 + 0.14 * Math.sin(performance.now() / 85));
      },
      onDone: resumeWorking,
    });
    voice.runtime.narrate(event.text);
    return;
  }
  if (event.kind === "ask_user") {
    showAskCard(event);
    return;
  }
  if (event.kind === "approval_request") {
    showApprovalCard(event);
    return;
  }

  if (event.kind === "tool_start") {
    // Whatever Leo said before this call ("Let me look that up") stays as a
    // message above the tool row; the next piece of text starts a new one.
    if (liveEl) {
      finishLive();
      voice.runtime.flushLeoText();
      resumeWorking();
    }
    const state: ToolRowState = {};
    toolRows.set(event.id, state);

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

/** Leo said something and is still working: back to the thinking look until the next thing arrives. */
function resumeWorking() {
  if (!turnBusy || pendingCard) return;
  if (!voice.ownsOrb) {
    orb.setLevel(0);
    setState("thinking");
  }
  showTyping();
}

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
  dropLive();
  voice.runtime.stopSpeaking();
  messagesEl.querySelectorAll(".msg, .tool-step, .card").forEach((el) => el.remove());
  pendingCard = null;
  toolRows.clear();
  document.body.classList.remove("has-messages");
  hideTyping();
  llm.reset();
  renderGreeting();
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

// ---------- global shortcut ----------
// Click the field and press a combination; it is stored as an accelerator
// string ("Ctrl+Alt+L") that the Rust side registers system-wide.
function acceleratorFrom(event: KeyboardEvent): string | null {
  const code = event.code;
  let key: string | null = null;
  if (/^Key[A-Z]$/.test(code)) key = code.slice(3);
  else if (/^Digit\d$/.test(code)) key = code.slice(5);
  else if (/^F([1-9]|1\d|2[0-4])$/.test(code)) key = code;
  else if (code === "Space") key = "Space";
  else if (/^Arrow(Up|Down|Left|Right)$/.test(code)) key = code.slice(5);
  if (!key) return null;
  const mods: string[] = [];
  if (event.ctrlKey) mods.push("Ctrl");
  if (event.altKey) mods.push("Alt");
  if (event.shiftKey) mods.push("Shift");
  if (event.metaKey) mods.push("Super");
  // A bare letter would steal normal typing everywhere; function keys are fine.
  if (mods.length === 0 && !/^F\d+$/.test(key)) return null;
  return [...mods, key].join("+");
}

shortcutInput.addEventListener("keydown", (event) => {
  if (event.key === "Tab") return;
  event.preventDefault();
  event.stopPropagation();
  if (event.key === "Escape") {
    shortcutInput.blur();
    return;
  }
  if (event.key === "Backspace" || event.key === "Delete") {
    shortcutInput.value = "";
    return;
  }
  const accel = acceleratorFrom(event);
  if (accel) shortcutInput.value = accel;
});
shortcutClear.addEventListener("click", () => {
  shortcutInput.value = "";
});

// Pressing the shortcut: the orb widget grows to the full chat, and pressing
// it again shrinks the chat back to the widget. Coming back from the taskbar
// only shows the window.
// Leo shrinks to the orb while it takes a screenshot (so the chat is not in
// the picture) and opens the chat again afterwards.
let reopenAfterScreenshot = false;
void listen("screen://prepare", async () => {
  reopenAfterScreenshot = mode === "full";
  try {
    if (reopenAfterScreenshot) await setMode("widget", true);
  } finally {
    await invoke("screen_prepared");
  }
});
void listen("screen://done", async () => {
  if (!reopenAfterScreenshot) return;
  reopenAfterScreenshot = false;
  await setMode("full", true);
});

void listen<boolean>("shortcut://pressed", (event) => {
  if (event.payload && mode === "full") return;
  void setMode(mode === "widget" ? "full" : "widget");
});

async function applyShortcut(): Promise<void> {
  await invoke("set_shortcut", { shortcut: localStorage.getItem(STORAGE_SHORTCUT) ?? "" });
}
applyShortcut().catch(() => {
  // A shortcut another app now owns must not stop Leo from starting.
});

function fillSettings() {
  shortcutInput.value = localStorage.getItem(STORAGE_SHORTCUT) ?? "";
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

// ---------- profile (a second tab inside the settings window) ----------
interface Profile { name: string; dob: string; location: string; occupation: string; interests: string }
const EMPTY_PROFILE: Profile = { name: "", dob: "", location: "", occupation: "", interests: "" };
let profile: Profile = { ...EMPTY_PROFILE };

const profileBack = document.querySelector<HTMLButtonElement>("#profile-back")!;
const profileOpenBtn = document.querySelector<HTMLButtonElement>("#profile-open-btn")!;
const profileSummary = document.querySelector<HTMLSpanElement>("#profile-summary")!;
const profileSaveBtn = document.querySelector<HTMLButtonElement>("#profile-save-btn")!;
const profileStatus = document.querySelector<HTMLParagraphElement>("#profile-status")!;
const settingsTitle = document.querySelector<HTMLHeadingElement>("#settings-title")!;
const profileFields = {
  name: document.querySelector<HTMLInputElement>("#profile-name")!,
  dob: document.querySelector<HTMLInputElement>("#profile-dob")!,
  location: document.querySelector<HTMLInputElement>("#profile-location")!,
  occupation: document.querySelector<HTMLInputElement>("#profile-occupation")!,
  interests: document.querySelector<HTMLTextAreaElement>("#profile-interests")!,
};

function profileIsEmpty(p: Profile): boolean {
  return Object.values(p).every((v) => !v.trim());
}

function showSettingsPane(pane: "main" | "profile") {
  settingsModal.dataset.pane = pane;
  profileBack.classList.toggle("hidden", pane !== "profile");
  settingsTitle.textContent = pane === "profile" ? "Your profile" : "Settings";
  if (pane === "profile") {
    for (const key of Object.keys(profileFields) as (keyof Profile)[]) profileFields[key].value = profile[key];
    profileStatus.textContent = "";
    profileFields.name.focus();
  }
}

function updateProfileSummary() {
  profileSummary.textContent = profile.name.trim() ? `Hi, ${profile.name.trim().split(/\s+/)[0]}` : profileIsEmpty(profile) ? "Not set up yet" : "Saved";
}

// The greeting on the empty chat screen: personal when there is a name, the
// plain heading otherwise. The variant is picked once per session.
const emptyHeading = document.querySelector<HTMLHeadingElement>("#empty h1")!;
const PLAIN_HEADING = emptyHeading.textContent ?? "";
const greetingSeed = Math.floor(Math.random() * 1_000_000);

function renderGreeting() {
  emptyHeading.textContent = greeting(profile, new Date(), greetingSeed) ?? PLAIN_HEADING;
}

async function loadProfile() {
  try {
    profile = await invoke<Profile>("profile_get");
  } catch {
    return; // not running inside the app (plain browser)
  }
  updateProfileSummary();
  renderGreeting();
}

profileOpenBtn.addEventListener("click", () => showSettingsPane("profile"));
profileBack.addEventListener("click", () => showSettingsPane("main"));

let profileSavedTimer = 0;
profileSaveBtn.addEventListener("click", async () => {
  const entered: Profile = {
    name: profileFields.name.value,
    dob: profileFields.dob.value,
    location: profileFields.location.value,
    occupation: profileFields.occupation.value,
    interests: profileFields.interests.value,
  };
  try {
    profile = await invoke<Profile>("profile_save", { profile: entered });
  } catch (err) {
    profileStatus.textContent = errorMessage(err);
    return;
  }
  updateProfileSummary();
  renderGreeting();
  profileStatus.textContent = "Saved.";
  window.clearTimeout(profileSavedTimer);
  profileSavedTimer = window.setTimeout(() => (profileStatus.textContent = ""), 2500);
});

// After an update (or on a first run once Leo is set up), existing users with
// an empty profile are asked once to add their details.
const profilePrompt = document.querySelector<HTMLDivElement>("#profile-prompt")!;

function closeProfilePrompt() {
  profilePrompt.classList.add("hidden");
}

function maybePromptProfile() {
  if (localStorage.getItem(STORAGE_PROFILE_PROMPTED)) return;
  const busy =
    mode !== "full" ||
    warping ||
    minimizing ||
    pendingWhatsNew !== null ||
    !whatsNewPanel.classList.contains("hidden") ||
    !settingsModal.classList.contains("hidden") ||
    !historyPanel.classList.contains("hidden");
  if (busy || !profileIsEmpty(profile) || !getApiKey() || !getModel()) return;
  localStorage.setItem(STORAGE_PROFILE_PROMPTED, "1");
  profilePrompt.classList.remove("hidden");
}

document.querySelector<HTMLButtonElement>("#profile-prompt-no")!.addEventListener("click", closeProfilePrompt);
document.querySelector<HTMLButtonElement>("#profile-prompt-yes")!.addEventListener("click", () => {
  closeProfilePrompt();
  openSettings();
  showSettingsPane("profile");
});

function openSettings(message = "") {
  if (mode !== "full") return;
  showSettingsPane("main");
  fillSettings();
  settingsStatus.textContent = message;
  settingsModal.classList.remove("hidden");
  void refreshStatus();
  (getApiKey() ? modelInput : apiKeyInput).focus();
}

function closeSettings() {
  const wasOpen = !settingsModal.classList.contains("hidden");
  settingsModal.classList.add("hidden");
  showSettingsPane("main");
  // Right after the first set-up, offer the profile.
  if (wasOpen) window.setTimeout(maybePromptProfile, 700);
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

// ---------- updates ----------
const appVersionEl = document.querySelector<HTMLSpanElement>("#app-version")!;
const updateBtn = document.querySelector<HTMLButtonElement>("#update-btn")!;
const updateStatus = document.querySelector<HTMLParagraphElement>("#update-status")!;

void getVersion()
  .then((version) => (appVersionEl.textContent = `Leo ${version}`))
  .catch(() => {});

updateBtn.addEventListener("click", async () => {
  updateBtn.disabled = true;
  updateStatus.textContent = "Checking…";
  try {
    const result = await invoke<{ state: string; message: string }>("check_for_updates");
    updateStatus.textContent = result.message;
    // When an update starts, the flight takes over the window; keep the button off.
    if (result.state === "updating") return;
  } catch (err) {
    updateStatus.textContent = errorMessage(err);
  }
  updateBtn.disabled = false;
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
    const previous = localStorage.getItem(STORAGE_SHORTCUT) ?? "";
    localStorage.setItem(STORAGE_SHORTCUT, shortcutInput.value);
    try {
      await applyShortcut();
    } catch (err) {
      localStorage.setItem(STORAGE_SHORTCUT, previous);
      void applyShortcut().catch(() => {});
      shortcutInput.value = previous;
      settingsStatus.textContent = errorMessage(err);
      return;
    }
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

// ---------- chat history (sidebar) ----------
// Chats are saved on this device (SQLite). Opening one restores its messages
// and the assistant carries on with the notes (context summary and steps
// summary) that were saved with it. The model names each chat after its first
// message. Each chat has its own delete button (a second click confirms).
interface ChatInfo { id: number; title: string; updated_at: number }
interface StoredMessage { role: string; content: string }

const TRASH_ICON =
  '<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8h5.8l.6-8M6.8 7v3.5M9.2 7v3.5"/></svg>';

const HISTORY_SLIDE_MS = 240; // matches side-out
let historyCloseTimer = 0;

function closeHistory() {
  if (historyPanel.classList.contains("hidden") || historyPanel.classList.contains("closing")) return;
  historyPanel.classList.add("closing");
  historyCloseTimer = window.setTimeout(() => {
    historyPanel.classList.add("hidden");
    historyPanel.classList.remove("closing");
  }, HISTORY_SLIDE_MS);
}

function whenText(seconds: number): string {
  const d = new Date(seconds * 1000);
  return d.toDateString() === new Date().toDateString()
    ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : d.toLocaleDateString([], { day: "numeric", month: "short" });
}

function showNoChats() {
  const none = document.createElement("div");
  none.className = "history-empty";
  none.textContent = "No saved chats yet.";
  historyList.appendChild(none);
}

async function renderHistory() {
  let chats: ChatInfo[] = [];
  let activeId: number | null = null;
  try {
    [chats, activeId] = await Promise.all([invoke<ChatInfo[]>("history_list"), invoke<number | null>("history_current")]);
  } catch (err) {
    historyList.textContent = errorMessage(err);
    return;
  }
  historyList.textContent = "";
  if (chats.length === 0) {
    showNoChats();
    return;
  }
  for (const chat of chats) {
    const row = document.createElement("div");
    row.className = "history-item" + (chat.id === activeId ? " active" : "");
    row.tabIndex = 0;

    const meta = document.createElement("div");
    meta.className = "meta";
    const title = document.createElement("span");
    title.className = "title";
    title.textContent = chat.title;
    const when = document.createElement("span");
    when.className = "when";
    when.textContent = whenText(chat.updated_at);
    meta.append(title, when);

    const del = document.createElement("button");
    del.className = "del";
    del.title = "Delete chat";
    del.innerHTML = TRASH_ICON;
    let armed = false;
    let timer = 0;
    const disarm = () => {
      armed = false;
      del.classList.remove("confirm");
      del.innerHTML = TRASH_ICON;
    };
    del.addEventListener("click", async (event) => {
      event.stopPropagation();
      if (!armed) {
        armed = true;
        del.classList.add("confirm");
        del.textContent = "Delete?";
        timer = window.setTimeout(disarm, 3000);
        return;
      }
      window.clearTimeout(timer);
      try {
        await invoke("history_delete", { id: chat.id });
      } catch {
        disarm();
        return;
      }
      if (chat.id === activeId) clearChat();
      row.remove();
      if (!historyList.querySelector(".history-item")) showNoChats();
    });
    del.addEventListener("blur", () => {
      window.clearTimeout(timer);
      disarm();
    });

    row.append(meta, del);
    row.addEventListener("click", () => void openChat(chat.id));
    row.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && event.target === row) void openChat(chat.id);
    });
    historyList.appendChild(row);
  }
}

async function openHistory() {
  // Opening while it is still sliding out cancels the close.
  window.clearTimeout(historyCloseTimer);
  historyPanel.classList.remove("closing");
  historyPanel.classList.remove("hidden");
  await renderHistory();
}

async function openChat(id: number) {
  closeHistory();
  llm.cancel();
  clearChat();
  try {
    const messages = await invoke<StoredMessage[]>("history_open", { id });
    for (const m of messages) {
      addMessage(m.role === "user" ? "user" : "assistant", m.content, { instant: true });
    }
    scrollToEnd();
  } catch (err) {
    addMessage("error", errorMessage(err));
  }
}

historyBtn.addEventListener("click", () => void openHistory());
historyClose.addEventListener("click", closeHistory);
historyPanel.addEventListener("click", (event) => {
  if (event.target === historyPanel) closeHistory();
});
window.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !historyPanel.classList.contains("hidden")) closeHistory();
});
// A new chat gets its name from the model a moment after the first message.
void listen("history://titled", () => {
  if (!historyPanel.classList.contains("hidden")) void renderHistory();
});

closeBtn.addEventListener("click", () => {
  void getCurrentWindow().close();
});

// ---------- sending ----------
// Bumped on every send; a reply from an older send is ignored.
let turnGen = 0;

async function sendMessage(text: string, images: string[] = []) {
  // A waiting question or approval card takes the text as its answer.
  if (pendingCard) {
    if (text) answerPendingCard(text);
    return;
  }

  // A new message takes over from whatever was still running.
  llm.cancel();
  const myTurn = ++turnGen;
  deltasOpen = true;
  turnBusy = true;
  voice.runtime.beginResponse(); // drops anything still being said
  orb.setLevel(0);
  finishLive();

  const sent = addMessage("user", text);
  renderBubbleImages(sent, images);
  scrollToEnd();
  showTyping();
  setState("thinking");
  try {
    const reply = await llm.ask(text, images);
    if (myTurn !== turnGen) return;
    turnBusy = false;
    voice.runtime.endLeoText(reply);
    if (liveTyper) {
      // The reply has been typing as it arrived; let the typer work off what is
      // queued, show the final text, then settle the orb.
      const typer = liveTyper;
      liveEl = null;
      liveTyper = null;
      typer.end(reply, () => {
        scrollToEnd();
        if (voice.ownsOrb) return; // the voice is still talking: it owns the orb
        orb.setLevel(0);
        if (currentState === "speaking") setState("idle");
      });
      return;
    }
    // The orb ripples to the rhythm of the text as it appears and settles
    // when it is done.
    setState("speaking");
    addMessage("assistant", reply, {
      onTick: () => {
        if (!voice.ownsOrb) orb.setLevel(0.22 + 0.14 * Math.sin(performance.now() / 85));
      },
      onDone: () => {
        if (voice.ownsOrb) return;
        orb.setLevel(0);
        if (currentState === "speaking") setState("idle");
      },
    });
  } catch (err) {
    // "cancelled" means a newer message took over; it owns the UI now.
    if (myTurn !== turnGen || errorMessage(err) === "cancelled") return;
    turnBusy = false;
    dropLive();
    orb.setLevel(0);
    console.error(err);
    voice.runtime.failResponse();
    let message = errorMessage(err);
    if (images.length && /image|vision|multimodal|modalit/i.test(message)) {
      message += " (Pick a model that can read images in settings.)";
    }
    addMessage("error", message);
    setState("idle");
  }
}

function autoGrowInput() {
  composerInput.style.height = "auto";
  composerInput.style.height = `${Math.min(composerInput.scrollHeight, 120)}px`;
  sendBtn.disabled = composerInput.value.trim() === "" && attachments.count === 0;
}

// ---------- attached images ----------
const attachments = new AttachmentTray(attachTray, autoGrowInput, (message) => showCaption(message, 4000));

attachBtn.addEventListener("click", () => attachInput.click());
attachInput.addEventListener("change", () => {
  void attachments.add(Array.from(attachInput.files ?? []));
  attachInput.value = "";
});
// Pasting a picture (a screenshot, a copied image) attaches it; pasted text is untouched.
composerInput.addEventListener("paste", (event) => {
  const files = Array.from(event.clipboardData?.files ?? []).filter(isImageFile);
  if (files.length === 0) return;
  event.preventDefault();
  void attachments.add(files);
});
// Dropping files anywhere on the window attaches the images among them.
let dragDepth = 0;
const hasFiles = (event: DragEvent) => Array.from(event.dataTransfer?.types ?? []).includes("Files");
window.addEventListener("dragenter", (event) => {
  if (!hasFiles(event)) return;
  dragDepth++;
  document.body.classList.add("dragging");
});
window.addEventListener("dragleave", (event) => {
  if (!hasFiles(event)) return;
  dragDepth = Math.max(0, dragDepth - 1);
  if (dragDepth === 0) document.body.classList.remove("dragging");
});
window.addEventListener("dragover", (event) => {
  if (hasFiles(event)) event.preventDefault();
});
window.addEventListener("drop", (event) => {
  if (!hasFiles(event)) return;
  event.preventDefault();
  dragDepth = 0;
  document.body.classList.remove("dragging");
  void attachments.add(Array.from(event.dataTransfer?.files ?? []));
});

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
  const images = attachments.dataUrls();
  if (!text && images.length === 0) return;
  if (attachments.busy) {
    showCaption("Still preparing your images…", 2000);
    return;
  }
  if (!pendingCard && (!getApiKey() || !getModel())) {
    openSettings("Add your OpenRouter key and a model name to start chatting.");
    return;
  }
  composerInput.value = "";
  attachments.clear();
  autoGrowInput();
  void sendMessage(text, images);
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

// ---------- what's new ----------
// After an update the new features are listed once, inside the app. Bug fixes
// are never itemised: they are one general line.
const whatsNewPanel = document.querySelector<HTMLDivElement>("#whatsnew")!;
const whatsNewBody = document.querySelector<HTMLDivElement>("#whatsnew-body")!;
let pendingWhatsNew: Release[] | null = null;
// Read now, before start-up sets the flag: true when this is not a new install.
const hadEarlierInstall = !!localStorage.getItem(STORAGE_SETUP);

function showWhatsNew(releases: Release[]) {
  whatsNewBody.textContent = "";
  for (const release of releases) {
    const head = document.createElement("h3");
    head.textContent = `Leo ${release.version}`;
    const list = document.createElement("ul");
    for (const feature of release.features) {
      const item = document.createElement("li");
      item.textContent = feature;
      list.appendChild(item);
    }
    whatsNewBody.append(head, list);
  }
  const fixes = document.createElement("p");
  fixes.className = "fixes";
  fixes.textContent = BUG_FIXES_LINE;
  whatsNewBody.appendChild(fixes);
  whatsNewPanel.classList.remove("hidden");
}

function closeWhatsNew() {
  whatsNewPanel.classList.add("hidden");
  window.setTimeout(maybePromptProfile, 500);
}

/** Shown when the chat is on screen; a minimised orb waits until it opens. */
function flushWhatsNew() {
  if (!pendingWhatsNew || mode !== "full" || warping) return;
  const releases = pendingWhatsNew;
  pendingWhatsNew = null;
  window.setTimeout(() => showWhatsNew(releases), 900);
}

void getVersion()
  .then((current) => {
    // Installs from before versions were tracked were on 2.2.8 or older.
    const seen = localStorage.getItem(STORAGE_LAST_VERSION) ?? (hadEarlierInstall ? "2.2.8" : null);
    localStorage.setItem(STORAGE_LAST_VERSION, current);
    // A brand new install has nothing to compare with, so it gets no notes.
    if (!seen || seen === current) return;
    const releases = releasesSince(seen, current);
    if (releases.length > 0) {
      pendingWhatsNew = releases.slice(0, 3);
      flushWhatsNew();
    }
  })
  .catch(() => {});

document.querySelector<HTMLButtonElement>("#whatsnew-close")!.addEventListener("click", closeWhatsNew);
whatsNewPanel.addEventListener("click", (event) => {
  if (event.target === whatsNewPanel) closeWhatsNew();
});
document.querySelector<HTMLButtonElement>("#whatsnew-btn")!.addEventListener("click", () => {
  closeSettings();
  showWhatsNew(RELEASES.slice(0, 3));
});

// ---------- start ----------
setState("idle");
void loadProfile().then(() => window.setTimeout(maybePromptProfile, 2600));
void invoke("set_mode", { mode });

// A new install always gets the space flight once (as "Setting up Leo"), so
// the first thing seen is the same screen an update shows. The flag is stored
// at once, so a crash during it cannot repeat it forever.
let lead = 0;
if (!localStorage.getItem(STORAGE_SETUP)) {
  localStorage.setItem(STORAGE_SETUP, "1");
  startUpdateWarp("Setting up Leo");
  window.setTimeout(endUpdateWarp, UPDATE_ANIMATION_MS + 300);
  lead = UPDATE_ANIMATION_MS + 600;
} else {
  startIntro();
}
if (!getApiKey() || !getModel()) {
  // First run: once the orb has arrived, open the chat window for the welcome.
  window.setTimeout(() => {
    void setMode("full").then(() => openSettings("Welcome! Add your OpenRouter key and a model to get started."));
  }, lead + INTRO_MS);
}

// Types a rendered message out character by character.
//
// The reply is rendered as normal (formatted) DOM first, then its text is
// hidden and revealed in reading order. Formatting therefore never flickers:
// a bold word stays bold as it is typed, and list items, table rows and code
// blocks only appear once typing reaches them.

import { renderMarkdown } from "./markdown";

export interface Typer {
  /** Show the whole message immediately. */
  finish(): void;
}

const BASE_CHARS_PER_SECOND = 120;
/** Long replies speed up so none takes much longer than this to type. */
const MAX_SECONDS = 3.5;
const SENTENCE_PAUSE_MS = 70;
/** Typing eases in over this long instead of starting at full speed. */
const EASE_IN_MS = 350;

type Item =
  | { kind: "text"; node: Text; full: string; chain: HTMLElement[] }
  | { kind: "break"; node: HTMLElement; chain: HTMLElement[] };

function ancestors(node: Node, root: HTMLElement): HTMLElement[] {
  const chain: HTMLElement[] = [];
  for (let p = node.parentElement; p && p !== root; p = p.parentElement) chain.push(p);
  return chain;
}

export function typewrite(root: HTMLElement, onTick?: () => void, onDone?: () => void): Typer {
  const idle: Typer = { finish() {} };
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    onDone?.();
    return idle;
  }

  const items: Item[] = [];
  let total = 0;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    if (n.nodeType === Node.TEXT_NODE) {
      const text = n as Text;
      if (!text.data) continue;
      items.push({ kind: "text", node: text, full: text.data, chain: ancestors(text, root) });
      total += text.data.length;
    } else if ((n as HTMLElement).tagName === "BR") {
      items.push({ kind: "break", node: n as HTMLElement, chain: ancestors(n, root) });
      total += 1;
    }
  }
  if (!total) {
    onDone?.();
    return idle;
  }

  // Hide everything; each element is shown when typing first reaches it.
  const hidden = new Set<HTMLElement>();
  const hide = (el: HTMLElement) => {
    if (!hidden.has(el)) {
      hidden.add(el);
      el.style.display = "none";
    }
  };
  for (const item of items) {
    if (item.kind === "text") item.node.data = "";
    else hide(item.node);
    item.chain.forEach(hide);
  }
  const show = (el: HTMLElement) => {
    if (hidden.delete(el)) {
      el.style.display = "";
      // Each block fades in as typing reaches it.
      el.classList.add("type-in");
    }
  };

  const caret = document.createElement("span");
  caret.className = "type-caret";

  const charsPerSecond = Math.max(BASE_CHARS_PER_SECOND, total / MAX_SECONDS);
  let index = 0;
  let position = 0;
  let budget = 0;
  let pauseUntil = 0;
  const startedAt = performance.now();
  let last = startedAt;
  let smoothDt = 1 / 60;
  let frame = 0;
  let finished = false;

  /** Reveals one character (or line break) and returns it. */
  const revealOne = (): string => {
    const item = items[index];
    if (item.kind === "break") {
      item.chain.forEach(show);
      show(item.node);
      index++;
      return "\n";
    }
    if (position === 0) item.chain.forEach(show);
    position++;
    item.node.data = item.full.slice(0, position);
    const ch = item.full[position - 1];
    if (position >= item.full.length) {
      index++;
      position = 0;
    }
    return ch;
  };

  const placeCaret = () => {
    const current = position > 0 ? items[index] : items[index - 1];
    if (!current) return;
    current.node.parentNode?.insertBefore(caret, current.node.nextSibling);
  };

  const finish = () => {
    if (finished) return;
    finished = true;
    cancelAnimationFrame(frame);
    for (const item of items) if (item.kind === "text") item.node.data = item.full;
    hidden.forEach((el) => (el.style.display = ""));
    hidden.clear();
    caret.remove();
    onTick?.();
    onDone?.();
  };

  const step = (now: number) => {
    if (finished) return;
    const dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    // Frame times wobble; averaging them keeps characters arriving evenly.
    smoothDt += (dt - smoothDt) * 0.2;
    const ramp = Math.min(1, 0.3 + (0.7 * (now - startedAt)) / EASE_IN_MS);

    if (now >= pauseUntil) {
      budget += charsPerSecond * ramp * smoothDt;
      let count = Math.floor(budget);
      budget -= count;
      while (count-- > 0 && index < items.length) {
        const ch = revealOne();
        // A brief beat at the end of a sentence reads more naturally.
        if (ch === "." || ch === "!" || ch === "?" || ch === "\n") {
          pauseUntil = now + SENTENCE_PAUSE_MS;
          break;
        }
      }
    }

    if (index >= items.length) {
      finish();
      return;
    }
    placeCaret();
    onTick?.();
    frame = requestAnimationFrame(step);
  };

  frame = requestAnimationFrame(step);
  return { finish };
}

// ---------- typing a reply that is still arriving ----------
// Text from the model is queued as it comes in and revealed character by
// character at a steady pace, like a finished reply would be. The pace speeds
// up when the queue grows, so the typing never falls far behind the model.

export interface StreamTyper {
  /** Queue more text from the model. */
  push(text: string): void;
  /** The reply is complete: keep typing what is queued, then show `finalText`. */
  end(finalText: string, onDone?: () => void): void;
  /** Show everything received so far at once and stop. */
  finish(): void;
}

const STREAM_CHARS_PER_SECOND = 110;
/** The queue is worked off within about this long, however fast text arrives. */
const STREAM_MAX_LAG_SECONDS = 1.4;
const STREAM_PAUSE_MS = 60;

/** Closes Markdown that is still open in a half-typed reply, so it renders cleanly. */
function closeOpenMarkup(text: string): string {
  let out = text;
  if ((out.match(/```/g)?.length ?? 0) % 2 === 1) return out + "\n```";
  if ((out.match(/\*\*/g)?.length ?? 0) % 2 === 1) out += "**";
  if ((out.match(/`/g)?.length ?? 0) % 2 === 1) out += "`";
  return out;
}

export function createStreamTyper(root: HTMLElement, onTick?: () => void): StreamTyper {
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const caret = document.createElement("span");
  caret.className = "type-caret";

  let received = "";
  let shown = 0;
  let ended = false;
  let finished = false;
  let onDone: (() => void) | undefined;
  let frame = 0;
  let last = 0;
  let budget = 0;
  let pauseUntil = 0;
  let drawn = -1;

  const draw = (withCaret: boolean) => {
    if (shown === drawn && !withCaret === !caret.isConnected) return;
    drawn = shown;
    renderMarkdown(closeOpenMarkup(received.slice(0, shown)), root);
    if (withCaret) {
      let tail: Node = root;
      while (tail.lastChild && tail.lastChild.nodeType === Node.ELEMENT_NODE) tail = tail.lastChild;
      tail.appendChild(caret);
    }
  };

  const complete = () => {
    if (finished) return;
    finished = true;
    cancelAnimationFrame(frame);
    caret.remove();
    renderMarkdown(received, root);
    onTick?.();
    onDone?.();
  };

  const step = (now: number) => {
    frame = 0;
    if (finished) return;
    // Capped only against long stalls, so a slow frame rate does not slow the typing.
    const dt = Math.min(0.25, (now - last) / 1000);
    last = now;

    if (now >= pauseUntil) {
      const backlog = received.length - shown;
      const rate = Math.max(STREAM_CHARS_PER_SECOND, backlog / STREAM_MAX_LAG_SECONDS);
      budget += rate * dt;
      let count = Math.floor(budget);
      budget -= count;
      while (count-- > 0 && shown < received.length) {
        const ch = received[shown++];
        // A brief beat at the end of a sentence reads more naturally.
        if (ch === "." || ch === "!" || ch === "?" || ch === "\n") {
          pauseUntil = now + STREAM_PAUSE_MS;
          break;
        }
      }
    }

    if (shown >= received.length && ended) return complete();
    draw(true);
    onTick?.();
    // Caught up with the model: wait for more text instead of spinning.
    if (shown < received.length || ended) frame = requestAnimationFrame(step);
  };

  const run = () => {
    if (frame || finished) return;
    last = performance.now();
    frame = requestAnimationFrame(step);
  };

  return {
    push(text) {
      if (finished) return;
      received += text;
      if (reduced) {
        shown = received.length;
        draw(false);
        onTick?.();
        return;
      }
      run();
    },
    end(finalText, done) {
      if (finished) return done?.();
      onDone = done;
      ended = true;
      received = finalText.length >= shown ? finalText : received;
      if (reduced) return complete();
      run();
    },
    finish() {
      if (finished) return;
      shown = received.length;
      complete();
    },
  };
}

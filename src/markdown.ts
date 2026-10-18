// A small Markdown renderer for assistant replies.
//
// It builds DOM nodes with createElement/textContent and never touches
// innerHTML, so text that came from emails, web pages or files (which the
// model may repeat) cannot inject markup or scripts. Only http(s) links are
// made clickable.
//
// Supported: headings, paragraphs, bold, italic, inline code, fenced code,
// links and bare URLs, bullet/numbered lists, block quotes, tables, rules.

import { chartCard } from "./chart";

const INLINE_RE =
  /(`[^`\n]+`)|(\*\*[^*\n]+?\*\*|__[^_\n]+?__)|(\[[^\]\n]+\]\(https?:\/\/[^\s)]+\))|(https?:\/\/[^\s<>()]+[^\s<>().,;:!?'"])|(\*[^*\s][^*\n]*?\*|(?<![\w])_[^_\s][^_\n]*?_(?![\w]))/g;

const FENCE_RE = /^\s*```(\w*)/;
const HEADING_RE = /^(#{1,6})\s+(.*)$/;
const RULE_RE = /^\s*([-*_])\1{2,}\s*$/;
const LIST_RE = /^(\s*)([-*•]|\d+[.)])\s+(.*)$/;
const QUOTE_RE = /^\s*>\s?(.*)$/;
const TABLE_SEP_RE = /^\s*\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?\s*$/;

function safeHttpUrl(href: string): string | null {
  try {
    const url = new URL(href);
    return url.protocol === "http:" || url.protocol === "https:" ? url.href : null;
  } catch {
    return null;
  }
}

function appendLink(parent: HTMLElement, label: string, href: string) {
  const safe = safeHttpUrl(href);
  if (!safe) {
    parent.append(document.createTextNode(label));
    return;
  }
  const a = document.createElement("a");
  a.href = safe;
  a.title = safe;
  a.rel = "noopener noreferrer";
  a.textContent = label;
  parent.append(a);
}

function renderInline(text: string, parent: HTMLElement) {
  let last = 0;
  for (const m of text.matchAll(INLINE_RE)) {
    const index = m.index ?? 0;
    if (index > last) parent.append(document.createTextNode(text.slice(last, index)));
    last = index + m[0].length;

    if (m[1]) {
      const code = document.createElement("code");
      code.textContent = m[1].slice(1, -1);
      parent.append(code);
    } else if (m[2]) {
      const strong = document.createElement("strong");
      renderInline(m[2].slice(2, -2), strong);
      parent.append(strong);
    } else if (m[3]) {
      const link = /^\[([^\]]+)\]\((.+)\)$/.exec(m[3]);
      if (link) appendLink(parent, link[1], link[2]);
      else parent.append(document.createTextNode(m[3]));
    } else if (m[4]) {
      appendLink(parent, m[4], m[4]);
    } else if (m[5]) {
      const em = document.createElement("em");
      renderInline(m[5].slice(1, -1), em);
      parent.append(em);
    }
  }
  if (last < text.length) parent.append(document.createTextNode(text.slice(last)));
}

function splitRow(line: string): string[] {
  return line
    .trim()
    .replace(/^\|/, "")
    .replace(/\|$/, "")
    .split("|")
    .map((c) => c.trim());
}

function startsBlock(lines: string[], i: number): boolean {
  const line = lines[i];
  return (
    FENCE_RE.test(line) ||
    HEADING_RE.test(line) ||
    RULE_RE.test(line) ||
    LIST_RE.test(line) ||
    QUOTE_RE.test(line) ||
    (line.includes("|") && i + 1 < lines.length && TABLE_SEP_RE.test(lines[i + 1]))
  );
}

/**
 * Widgets: a fenced `widget` block holds a JSON card spec written by the
 * model (see widget.rs for the format). Icons come from the Iconify API by
 * `prefix:name`; the name is checked before it goes into a URL. Everything is
 * set with textContent, so nothing in the spec can become markup.
 */
const ICON_API = "https://api.iconify.design/";
const ICON_RE = /^([a-z0-9]+(?:-[a-z0-9]+)*):([a-z0-9]+(?:-[a-z0-9]+)*)$/;
/** Single-colour outline sets draw in black by default; the chat is dark, so they get the text colour. */
const MONO_SETS = new Set(["lucide", "tabler", "ph", "mdi", "ri", "feather", "heroicons", "carbon", "bi", "fluent", "material-symbols", "solar", "iconoir"]);
const MAX_ROWS = 24;
const MAX_CHIPS = 10;

type Spec = Record<string, unknown>;

const str = (v: unknown, max = 160): string =>
  typeof v === "string" || typeof v === "number" ? String(v).slice(0, max) : "";
const isObj = (v: unknown): v is Spec => !!v && typeof v === "object" && !Array.isArray(v);

function iconEl(name: unknown, cls: string): HTMLImageElement | null {
  const m = typeof name === "string" ? ICON_RE.exec(name.trim().toLowerCase()) : null;
  if (!m) return null;
  const img = document.createElement("img");
  img.className = cls;
  img.alt = "";
  img.loading = "lazy";
  img.src = `${ICON_API}${m[1]}/${m[2]}.svg${MONO_SETS.has(m[1]) ? "?color=%23eceef1" : ""}`;
  // An icon that doesn't exist or can't load (offline) just leaves the card without it.
  img.onerror = () => img.remove();
  return img;
}

function textEl(cls: string, text: string): HTMLElement | null {
  if (!text) return null;
  const el = document.createElement("div");
  el.className = cls;
  el.textContent = text;
  return el;
}

function widgetCard(json: string): HTMLElement | null {
  let spec: unknown;
  try {
    spec = JSON.parse(json);
  } catch {
    return null; // half-written or invalid: draw nothing
  }
  if (!isObj(spec)) return null;
  const title = str(spec.title);
  const value = str(spec.value, 40);
  if (!title && !value) return null;

  const card = document.createElement("div");
  card.className = "widget";

  const top = document.createElement("div");
  top.className = "widget-top";
  const main = document.createElement("div");
  main.className = "widget-main";
  const sub = str(spec.subtitle);
  for (const el of [
    textEl("widget-title", title),
    textEl("widget-sub", sub),
    textEl("widget-value", value),
    textEl("widget-caption", str(spec.caption)),
  ]) {
    if (el) main.append(el);
  }
  top.append(main);
  const big = iconEl(spec.icon, "widget-icon");
  if (big) top.append(big);
  card.append(top);

  if (Array.isArray(spec.chips) && spec.chips.length) {
    const chips = document.createElement("div");
    chips.className = "widget-chips";
    for (const c of spec.chips.slice(0, MAX_CHIPS)) {
      const text = isObj(c) ? str(c.text, 60) : str(c, 60);
      if (!text) continue;
      const chip = document.createElement("span");
      chip.className = "widget-chip";
      const ic = isObj(c) ? iconEl(c.icon, "widget-chip-icon") : null;
      if (ic) chip.append(ic);
      chip.append(document.createTextNode(text));
      chips.append(chip);
    }
    if (chips.childElementCount) card.append(chips);
  }

  const rows = document.createElement("div");
  rows.className = "widget-rows";
  if (Array.isArray(spec.rows)) {
    for (const r of spec.rows.slice(0, MAX_ROWS)) {
      if (!isObj(r)) continue;
      const row = document.createElement("div");
      row.className = "widget-row";
      const ic = iconEl(r.icon, "widget-row-icon");
      if (ic) row.append(ic);
      const label = document.createElement("div");
      label.className = "widget-row-label";
      label.textContent = str(r.label, 80);
      const note = textEl("widget-row-note", str(r.note, 100));
      if (note) label.append(note);
      const val = document.createElement("div");
      val.className = "widget-row-value";
      val.textContent = str(r.value, 60);
      row.append(label, val);
      rows.append(row);
    }
  }
  if (rows.childElementCount) {
    const expandLabel = str(spec.expandLabel, 40);
    if (expandLabel) {
      const body = document.createElement("div");
      body.className = "widget-expand";
      body.append(rows);
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "widget-toggle";
      btn.setAttribute("aria-expanded", "false");
      btn.append(document.createTextNode(expandLabel));
      const chev = iconEl("lucide:chevron-down", "widget-chevron");
      if (chev) btn.append(chev);
      btn.addEventListener("click", () => {
        const open = card.classList.toggle("open");
        btn.setAttribute("aria-expanded", String(open));
      });
      card.append(btn, body);
    } else {
      card.append(rows);
    }
  }

  const footer = textEl("widget-footer", str(spec.footer));
  if (footer) card.append(footer);
  return card;
}

export function renderMarkdown(source: string, root: HTMLElement) {
  root.replaceChildren();
  const lines = source.replace(/\r\n?/g, "\n").split("\n");
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];

    if (!line.trim()) {
      i++;
      continue;
    }

    if (FENCE_RE.test(line)) {
      const lang = FENCE_RE.exec(line)![1];
      const code: string[] = [];
      i++;
      while (i < lines.length && !/^\s*```/.test(lines[i])) code.push(lines[i++]);
      i++; // closing fence (or end of text)
      if (lang === "widget" || lang === "chart") {
        // A widget or chart, not code. Half-written data draws nothing until it is whole.
        const widget = (lang === "chart" ? chartCard : widgetCard)(code.join("\n"));
        if (widget) root.append(widget);
        continue;
      }
      const pre = document.createElement("pre");
      const codeEl = document.createElement("code");
      codeEl.textContent = code.join("\n");
      pre.append(codeEl);
      root.append(pre);
      continue;
    }

    const heading = HEADING_RE.exec(line);
    if (heading) {
      const el = document.createElement("div");
      el.className = `md-h md-h${Math.min(heading[1].length, 3)}`;
      renderInline(heading[2], el);
      root.append(el);
      i++;
      continue;
    }

    if (RULE_RE.test(line)) {
      root.append(document.createElement("hr"));
      i++;
      continue;
    }

    if (line.includes("|") && i + 1 < lines.length && TABLE_SEP_RE.test(lines[i + 1])) {
      const table = document.createElement("table");
      const head = document.createElement("thead");
      const headRow = document.createElement("tr");
      for (const cell of splitRow(line)) {
        const th = document.createElement("th");
        renderInline(cell, th);
        headRow.append(th);
      }
      head.append(headRow);
      const body = document.createElement("tbody");
      i += 2;
      while (i < lines.length && lines[i].includes("|") && lines[i].trim()) {
        const row = document.createElement("tr");
        for (const cell of splitRow(lines[i])) {
          const td = document.createElement("td");
          renderInline(cell, td);
          row.append(td);
        }
        body.append(row);
        i++;
      }
      table.append(head, body);
      const wrap = document.createElement("div");
      wrap.className = "md-table";
      wrap.append(table);
      root.append(wrap);
      continue;
    }

    if (QUOTE_RE.test(line)) {
      const quote = document.createElement("blockquote");
      const parts: string[] = [];
      while (i < lines.length && QUOTE_RE.test(lines[i])) {
        parts.push(QUOTE_RE.exec(lines[i])![1]);
        i++;
      }
      renderInline(parts.join("\n"), quote);
      root.append(quote);
      continue;
    }

    const firstItem = LIST_RE.exec(line);
    if (firstItem) {
      const ordered = /\d/.test(firstItem[2]);
      const list = document.createElement(ordered ? "ol" : "ul");
      let current: HTMLLIElement | null = null;
      while (i < lines.length) {
        const item = LIST_RE.exec(lines[i]);
        if (item) {
          current = document.createElement("li");
          const depth = Math.min(2, Math.floor(item[1].replace(/\t/g, "  ").length / 2));
          if (depth) current.style.marginLeft = `${depth * 16}px`;
          renderInline(item[3], current);
          list.append(current);
          i++;
        } else if (current && lines[i].trim() && /^\s+\S/.test(lines[i])) {
          // Indented continuation of the previous item.
          current.append(document.createElement("br"));
          renderInline(lines[i].trim(), current);
          i++;
        } else {
          break;
        }
      }
      root.append(list);
      continue;
    }

    // Paragraph: runs until a blank line or the start of another block.
    const p = document.createElement("p");
    let first = true;
    while (i < lines.length && lines[i].trim() && (first || !startsBlock(lines, i))) {
      if (!first) p.append(document.createElement("br"));
      renderInline(lines[i].trim(), p);
      first = false;
      i++;
    }
    root.append(p);
  }
}

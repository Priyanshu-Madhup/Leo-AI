// A small Markdown renderer for assistant replies.
//
// It builds DOM nodes with createElement/textContent and never touches
// innerHTML, so text that came from emails, web pages or files (which the
// model may repeat) cannot inject markup or scripts. Only http(s) links are
// made clickable.
//
// Supported: headings, paragraphs, bold, italic, inline code, fenced code,
// links and bare URLs, bullet/numbered lists, block quotes, tables, rules.

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
      const code: string[] = [];
      i++;
      while (i < lines.length && !/^\s*```/.test(lines[i])) code.push(lines[i++]);
      i++; // closing fence (or end of text)
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

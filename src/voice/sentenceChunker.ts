/**
 * Turns a stream of text deltas into sentences that are worth sending to the
 * voice. It waits for a real boundary (so "Sure." and "I" are never spoken
 * alone), does not break on "Dr.", "3.5" or "1." list numbers, skips fenced
 * code and splits very long run-ons at a comma.
 */

const ABBREVIATIONS = new Set([
  "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "vs", "etc", "e.g", "i.e", "approx", "inc", "ltd", "no", "fig",
]);

export interface ChunkerOptions {
  /** A sentence shorter than this waits for the next one. */
  minChars?: number;
  /** The first chunk of a reply may be shorter, to get the voice going sooner. */
  firstMinChars?: number;
  /** Without a boundary for this long, split at the last comma or space. */
  maxChars?: number;
  /**
   * Let the first chunk of a reply end at a comma once it is this long. Speech
   * takes time in proportion to its length, so a short first chunk gets Leo
   * talking sooner. 0 turns it off.
   */
  firstClauseChars?: number;
}

export class SentenceChunker {
  private buffer = "";
  private inFence = false;
  private pendingTicks = "";
  private emitted = 0;
  private readonly minChars: number;
  private readonly firstMinChars: number;
  private readonly maxChars: number;
  private readonly firstClauseChars: number;

  constructor(options: ChunkerOptions = {}) {
    this.minChars = options.minChars ?? 24;
    this.firstMinChars = options.firstMinChars ?? 14;
    this.maxChars = options.maxChars ?? 220;
    this.firstClauseChars = options.firstClauseChars ?? 22;
  }

  push(token: string): string[] {
    this.buffer += this.stripFences(token);
    return this.drain(false);
  }

  /** The stream ended: whatever is left is spoken, however short. */
  flush(): string[] {
    const out = this.drain(true);
    const rest = this.buffer.trim();
    this.buffer = "";
    if (rest) out.push(rest);
    this.emitted += out.length;
    return out;
  }

  /** Forgets everything not yet emitted (a new reply, or the reply was cut off). */
  reset(): void {
    this.buffer = "";
    this.inFence = false;
    this.pendingTicks = "";
    this.emitted = 0;
  }

  /** Removes ``` fenced code, which is shown in the chat but never read aloud. */
  private stripFences(text: string): string {
    // A fence marker split across two tokens would slip through, so a trailing
    // run of one or two backticks waits for the next token.
    let rest = this.pendingTicks + text;
    this.pendingTicks = "";
    const trailing = /`{1,2}$/.exec(rest);
    if (trailing) {
      this.pendingTicks = trailing[0];
      rest = rest.slice(0, -trailing[0].length);
    }
    let out = "";
    let inside = this.inFence;
    rest.split("```").forEach((part, i) => {
      if (i > 0) inside = !inside; // each marker flips in/out of a fence
      if (!inside) out += part;
    });
    this.inFence = inside;
    return out;
  }

  private drain(final: boolean): string[] {
    const out: string[] = [];
    for (;;) {
      const end = this.findBoundary(final);
      if (end < 0) break;
      const piece = this.buffer.slice(0, end).trim();
      this.buffer = this.buffer.slice(end);
      if (piece) {
        out.push(piece);
        this.emitted++;
      }
    }
    return out;
  }

  private findBoundary(final: boolean): number {
    const min = this.emitted === 0 ? this.firstMinChars : this.minChars;
    const text = this.buffer;

    for (let i = 0; i < text.length; i++) {
      const ch = text[i];
      const isBreak = ch === "\n" && text[i + 1] === "\n";
      const isStop = ch === "." || ch === "!" || ch === "?" || ch === "…";
      const isLineEnd = ch === "\n" && i > 0;
      if (!isBreak && !isStop && !isLineEnd) continue;

      let end = i + 1;
      if (isStop) {
        // Swallow ellipses and closing quotes/brackets after the stop.
        while (end < text.length && /[.!?"'”’)\]*_]/.test(text[end])) end++;
        // Need to see what follows: a boundary is a stop followed by space.
        if (end >= text.length) {
          if (!final) return -1;
        } else if (!/\s/.test(text[end])) {
          i = end - 1;
          continue;
        }
        if (ch === "." && this.isNotASentenceEnd(text, i)) continue;
      }
      // A newline only ends a chunk when it ends a line of real text.
      if (isLineEnd && !isBreak && text.slice(0, i).trim().length === 0) continue;

      const candidate = text.slice(0, end).trim();
      if (candidate.length < min) continue; // too short: keep reading
      return end;
    }

    if (this.emitted === 0 && this.firstClauseChars > 0) {
      const clause = /[,;:] /g;
      for (let m = clause.exec(text); m; m = clause.exec(text)) {
        if (m.index + 1 >= this.firstClauseChars) return m.index + 1;
      }
    }

    if (text.length > this.maxChars) {
      const cut = this.softSplit(text);
      if (cut > 0) return cut;
    }
    return -1;
  }

  /** "Dr. Smith", "3.5", "1. First point", "J. K. Rowling" are not sentence ends. */
  private isNotASentenceEnd(text: string, dot: number): boolean {
    let start = dot;
    while (start > 0 && /[A-Za-z0-9.]/.test(text[start - 1])) start--;
    const word = text.slice(start, dot).toLowerCase();
    if (word === "") return true;
    if (/^\d+$/.test(word)) {
      // "1." at the start of a line is a list number; "in 1999." is a real end.
      const before = text.slice(0, start);
      return before.trim() === "" || before.endsWith("\n");
    }
    if (/^[a-z]$/.test(word)) return true; // an initial
    return ABBREVIATIONS.has(word);
  }

  private softSplit(text: string): number {
    const window = text.slice(0, this.maxChars);
    const comma = Math.max(window.lastIndexOf(", "), window.lastIndexOf("; "), window.lastIndexOf(": "));
    if (comma > this.maxChars / 2) return comma + 1;
    const space = window.lastIndexOf(" ");
    return space > this.maxChars / 2 ? space : -1;
  }
}

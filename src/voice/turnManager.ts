/**
 * Decides when the user has finished their thought. The voice-activity
 * detector only knows "sound / no sound"; this adds a judgement on top. It is
 * an interface so a learned end-of-turn model (such as Smart-Turn) can replace
 * the heuristic below without touching the runtime.
 */
export interface TurnManager {
  /** Whether a transcript is worth sending to Leo at all (not empty, not a noise word). */
  isSubmittable(transcript: string): boolean;
  /**
   * How long to keep waiting, measured from the moment the speaker went
   * quiet, before treating this transcript as the end of the turn.
   */
  holdMs(transcript: string): number;
}

/** Words that, at the end of what was said, mean the sentence is not over. */
const DANGLING = new Set([
  "and", "but", "or", "so", "because", "if", "then", "that", "which", "who", "when", "while", "with", "without",
  "to", "of", "for", "in", "on", "at", "the", "a", "an", "my", "your", "his", "her", "their", "is", "are",
  "was", "were", "i", "i'm", "it", "like", "um", "uh", "er", "hmm", "also", "plus", "about", "from", "into",
]);

/** Sounds Whisper tends to invent from breath, keys and room noise. */
const NOISE = new Set(["uh", "um", "hmm", "mm", "huh", "ah", "oh", "you", "the", "so", "and", "yeah.", "bye.", "thank you."]);

export interface HeuristicOptions {
  /** Wait after a finished-sounding sentence. */
  doneMs?: number;
  /** Wait when the transcript has no closing punctuation. */
  unsureMs?: number;
  /** Wait when it ends in a comma, a dangling word or a filler. */
  openMs?: number;
}

export class HeuristicTurnManager implements TurnManager {
  private readonly doneMs: number;
  private readonly unsureMs: number;
  private readonly openMs: number;

  constructor(options: HeuristicOptions = {}) {
    this.doneMs = options.doneMs ?? 0;
    this.unsureMs = options.unsureMs ?? 250;
    this.openMs = options.openMs ?? 900;
  }

  isSubmittable(transcript: string): boolean {
    const text = transcript.trim();
    if (!/[\p{L}\p{N}]/u.test(text)) return false;
    // Whisper writes silence as "[BLANK_AUDIO]", "(music)", "♪" and the like.
    if (/^[[(*♪].*[\])*♪]$/.test(text)) return false;
    const lower = text.toLowerCase();
    if (NOISE.has(lower)) return false;
    return true;
  }

  holdMs(transcript: string): number {
    const text = transcript.trim();
    if (!text) return this.openMs;
    if (/[,;:…]$|\.\.\.$/.test(text)) return this.openMs;
    const last = text.toLowerCase().replace(/[^\p{L}\p{N}']+$/u, "").split(/\s+/).pop() ?? "";
    if (DANGLING.has(last)) return this.openMs;
    if (/[.!?]["')\]]?$/.test(text)) return this.doneMs;
    // No closing punctuation: probably done, but a very short fragment may continue.
    return text.split(/\s+/).length < 3 ? this.unsureMs + 250 : this.unsureMs;
  }
}

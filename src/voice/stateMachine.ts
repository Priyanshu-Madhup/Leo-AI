import type { VoicePhase } from "./types";

/**
 * The one place that decides which phase the voice session is in. The UI
 * only reads it. A move that is not listed here is refused, which stops
 * late callbacks (an old reply finishing, a stale transcript) from dragging
 * the session into a state it has already left.
 */
const ALLOWED: Record<VoicePhase, VoicePhase[]> = {
  idle: ["loading"],
  loading: ["listening", "error", "idle"],
  listening: ["user_speaking", "thinking", "speaking", "idle", "error"],
  user_speaking: ["listening", "thinking", "idle", "error"],
  // Speaking over a reply that is still being prepared cancels it.
  thinking: ["speaking", "listening", "user_speaking", "interrupted", "idle", "error"],
  speaking: ["listening", "interrupted", "thinking", "idle", "error"],
  interrupted: ["user_speaking", "listening", "thinking", "idle", "error"],
  error: ["idle", "loading", "listening"],
};

export class VoiceState {
  private current: VoicePhase = "idle";

  constructor(private onChange: (phase: VoicePhase, previous: VoicePhase) => void = () => {}) {}

  get phase(): VoicePhase {
    return this.current;
  }

  is(...phases: VoicePhase[]): boolean {
    return phases.includes(this.current);
  }

  /** Moves to `next` if allowed. Staying in the same phase is a quiet success. */
  go(next: VoicePhase): boolean {
    if (next === this.current) return true;
    if (!ALLOWED[this.current].includes(next)) return false;
    const previous = this.current;
    this.current = next;
    this.onChange(next, previous);
    return true;
  }
}

/**
 * Per-turn timing for the voice pipeline, so slowness can be measured instead
 * of guessed at. Times are performance.now() values in ms.
 *
 *   T1 speechEnd         the user stopped talking
 *   T2 transcriptFinal   the final text is ready
 *   T3 submitted         it was handed to Leo
 *   T4 llmFirstToken     the first piece of Leo's reply arrived
 *   T5 ttsFirstAudio     the first synthesized audio exists
 *   T6 audioStart        sound actually started coming out
 */
export interface TurnMetrics {
  speechStart?: number;
  speechEnd?: number;
  transcriptFinal?: number;
  submitted?: number;
  llmFirstToken?: number;
  ttsStart?: number;
  ttsFirstAudio?: number;
  audioStart?: number;
  interruptionTime?: number;
  interruptionStopped?: number;
}

export interface TurnSummary {
  sttMs?: number;
  llmFirstTokenMs?: number;
  ttsFirstAudioMs?: number;
  /** T6 - T1: what the user feels as "how long until it answers". */
  totalMs?: number;
  bargeInMs?: number;
}

const diff = (a?: number, b?: number) => (a === undefined || b === undefined ? undefined : Math.round(b - a));

export function summarize(m: TurnMetrics): TurnSummary {
  return {
    sttMs: diff(m.speechEnd, m.transcriptFinal),
    llmFirstTokenMs: diff(m.submitted, m.llmFirstToken),
    ttsFirstAudioMs: diff(m.ttsStart, m.ttsFirstAudio),
    totalMs: diff(m.speechEnd, m.audioStart),
    bargeInMs: diff(m.interruptionTime, m.interruptionStopped),
  };
}

export class VoiceMetrics {
  current: TurnMetrics = {};
  readonly history: { turn: TurnMetrics; summary: TurnSummary }[] = [];

  constructor(private log: (line: string, data: unknown) => void = () => {}) {}

  /** The first mark of a kind within a turn wins (the first token, the first audio…). */
  mark(key: keyof TurnMetrics, at = performance.now()): void {
    if (this.current[key] === undefined) this.current[key] = at;
  }

  /** For moments that can repeat within a turn, where the last one is the real one. */
  markLatest(key: keyof TurnMetrics, at = performance.now()): void {
    this.current[key] = at;
  }

  unmark(key: keyof TurnMetrics): void {
    delete this.current[key];
  }

  /** Throws away a turn that went nowhere (nothing was submitted). */
  discard(): void {
    this.current = {};
  }

  /** Closes the turn and starts a clean one. */
  finish(): void {
    if (Object.keys(this.current).length === 0) return;
    const summary = summarize(this.current);
    this.history.push({ turn: this.current, summary });
    if (this.history.length > 50) this.history.shift();
    this.log("voice turn", summary);
    this.current = {};
  }
}

import { BargeInDetector } from "./bargeIn";
import { VoiceMetrics } from "./metrics";
import { SentenceChunker } from "./sentenceChunker";
import { speechTextNormalizer } from "./speechText";
import { VoiceState } from "./stateMachine";
import { HeuristicTurnManager, type TurnManager } from "./turnManager";
import type { Player, STT, TTS, Vad, VoiceEventMap, VoiceEventName, VoicePhase } from "./types";

/**
 * The voice layer around Leo. It turns speech into text, hands the text to
 * Leo's existing text path, and turns Leo's streamed reply back into speech.
 * It does not know how Leo works: it is given two callbacks.
 *
 *   submit(text)      sends one user turn to Leo, exactly as typing would
 *   cancelResponse()  stops Leo's current generation
 *
 * Everything else flows through events, so the UI only renders what happens.
 */
export interface VoiceDeps {
  submit(text: string): void | Promise<void>;
  cancelResponse(): void;
}

/** The replaceable parts. Defaults are created by `createVoiceRuntime`. */
export interface VoiceParts {
  vad: Vad;
  stt: STT;
  tts: TTS;
  player: Player;
  turns?: TurnManager;
  bargeIn?: BargeInDetector;
  voice?: string;
  now?: () => number;
  /** Log per-turn timings to the console. */
  debug?: boolean;
}

type Handler<K extends VoiceEventName> = (payload: VoiceEventMap[K]) => void;

/** Seconds of audio kept from before the speech started, and the longest partial re-read. */
const PRE_ROLL_FRAMES = 12;
const PARTIAL_EVERY_MS = 700;
const PARTIAL_MIN_SAMPLES = 16000 * 0.6;
const PARTIAL_MAX_SAMPLES = 16000 * 25;

export class VoiceRuntime {
  private events = new Map<VoiceEventName, Set<Handler<never>>>();
  private state = new VoiceState((phase, previous) => this.emit("phase", { phase, previous }));
  readonly metrics: VoiceMetrics;

  private readonly vad: Vad;
  private readonly stt: STT;
  private readonly tts: TTS;
  private readonly player: Player;
  private readonly turns: TurnManager;
  private readonly bargeIn: BargeInDetector;
  private readonly voice: string | undefined;
  private readonly now: () => number;

  // Leo's reply on its way to the speaker.
  private epoch = 0;
  private accepting = false;
  private responseActive = false;
  private responseDone = false;
  private streamed = false;
  private chunker = new SentenceChunker();
  private queue: { text: string; epoch: number }[] = [];
  private pumping = false;
  private ttsErrored = false;

  // The user's turn.
  private vadSpeaking = false;
  private accepted = false;
  private preRoll: Float32Array[] = [];
  private utterance: Float32Array[] = [];
  private segTexts: string[] = [];
  private segmentsPending = 0;
  private lastSpeechEnd = 0;
  private turnToken = 0;
  private holdTimer: ReturnType<typeof setTimeout> | null = null;
  private partialTimer: ReturnType<typeof setInterval> | null = null;
  private partialBusy = false;
  private nextPartialAt = 0;

  private micRms = 0;
  private muted = false;
  private startToken = 0;

  constructor(
    private deps: VoiceDeps,
    parts: VoiceParts,
  ) {
    this.vad = parts.vad;
    this.stt = parts.stt;
    this.tts = parts.tts;
    this.player = parts.player;
    this.turns = parts.turns ?? new HeuristicTurnManager();
    this.bargeIn = parts.bargeIn ?? new BargeInDetector();
    this.voice = parts.voice;
    this.now = parts.now ?? (() => performance.now());
    this.metrics = new VoiceMetrics(parts.debug ? (line, data) => console.debug(`[leo voice] ${line}`, data) : undefined);

    this.player.onAudioStart = () => {
      this.metrics.mark("audioStart", this.now());
      this.emit("audioStart", undefined);
    };
    this.player.onAudioEnd = () => this.emit("audioEnd", undefined);
  }

  // ---------- public surface ----------

  get phase(): VoicePhase {
    return this.state.phase;
  }

  get active(): boolean {
    return !this.state.is("idle", "loading", "error");
  }

  /** Whether Leo's voice is, or is about to be, coming out of the speaker. */
  get isSpeaking(): boolean {
    return this.state.is("speaking");
  }

  on<K extends VoiceEventName>(name: K, handler: Handler<K>): () => void {
    let set = this.events.get(name);
    if (!set) this.events.set(name, (set = new Set()));
    set.add(handler as Handler<never>);
    return () => set!.delete(handler as Handler<never>);
  }

  /** Loudness of Leo's voice as it plays, 0..1 (read from the real output). */
  outputLevel(): number {
    return this.player.level();
  }

  /** Loudness of the microphone, 0..1. */
  inputLevel(): number {
    return Math.min(1, this.micRms * 4);
  }

  /** Opens the microphone, loads the models and starts listening. Call from a user gesture. */
  async start(): Promise<void> {
    if (!this.state.is("idle", "error") || !this.state.go("loading")) return;
    const token = ++this.startToken;
    try {
      // First, because the browser only lets audio start inside a click.
      await this.player.ensure();

      const progress = { stt: 0, tts: 0 };
      const report = () => this.emit("progress", { fraction: (progress.stt + progress.tts) / 2, what: "voice" });
      await Promise.all([
        this.stt.load((f) => ((progress.stt = f), report())),
        this.tts.load((f) => ((progress.tts = f), report())),
      ]);
      if (token !== this.startToken) return;

      await this.vad.start({
        onFrame: (p, frame) => this.onFrame(p, frame),
        onSpeechStart: () => this.onSpeechStart(),
        onSpeechEnd: (audio) => this.onSpeechEnd(audio),
        onMisfire: () => this.onMisfire(),
        onError: (err) => this.emit("error", { message: err.message, source: "mic", fatal: false }),
      });
      if (token !== this.startToken) {
        await this.vad.stop();
        return;
      }
      this.muted = false;
      this.state.go("listening");
    } catch (err) {
      if (token !== this.startToken) return;
      await this.vad.stop().catch(() => {});
      this.state.go("error");
      this.emit("error", { message: describe(err), source: "runtime", fatal: true });
      this.state.go("idle");
    }
  }

  /** Ends the voice session: the microphone is released and speech stops. Models stay loaded. */
  async stop(): Promise<void> {
    this.startToken++;
    this.clearTimers();
    this.silence();
    this.resetTurn();
    await this.vad.stop().catch(() => {});
    this.state.go("idle");
  }

  /** Releases everything, including the loaded models. */
  async dispose(): Promise<void> {
    await this.stop();
    this.stt.dispose();
    this.tts.dispose();
    this.player.dispose();
  }

  setMuted(muted: boolean): void {
    if (muted === this.muted || !this.active) return;
    this.muted = muted;
    if (muted) {
      this.clearTimers();
      this.resetTurn();
      void this.vad.pause();
    } else {
      void this.vad.resume();
    }
  }

  get isMuted(): boolean {
    return this.muted;
  }

  // ---------- Leo's reply -> speech ----------

  /** A new request is going to Leo (typed or spoken): whatever was being said is dropped. */
  beginResponse(): void {
    if (!this.active) return;
    this.silence();
    this.accepting = true;
    this.responseActive = true;
    this.responseDone = false;
    this.streamed = false;
    if (this.state.is("listening", "speaking", "interrupted")) this.state.go("thinking");
  }

  /** A piece of Leo's streamed reply. */
  feedLeoText(delta: string): void {
    if (!this.accepting || !delta) return;
    this.metrics.mark("llmFirstToken", this.now());
    this.streamed = true;
    this.responseDone = false;
    this.responseActive = true;
    this.emit("leoText", { text: delta });
    this.speakAll(this.chunker.push(delta));
  }

  /**
   * A line spoken on its own while Leo works ("Sure, I'm checking the weather").
   * Unlike streamed reply text it does not count as the reply, so a reply that
   * arrives whole afterwards (a plan's final answer) is still spoken in full.
   */
  narrate(text: string): void {
    if (!this.accepting) return;
    this.speakAll(this.chunker.flush()); // keep what was already written ahead of it
    this.speakAll([text]);
  }

  /** A pause in Leo's text (it is about to run a tool): speak what has been written so far, even without a closing stop. */
  flushLeoText(): void {
    if (!this.accepting) return;
    this.speakAll(this.chunker.flush());
  }

  /** Leo's reply is complete. `reply` is spoken in full if nothing streamed. */
  endLeoText(reply: string): void {
    if (!this.accepting) return;
    if (!this.streamed && reply) {
      this.metrics.mark("llmFirstToken", this.now());
      this.emit("leoText", { text: reply });
      this.speakAll(this.chunker.push(reply));
    }
    this.speakAll(this.chunker.flush());
    this.responseDone = true;
    this.finishIfDrained();
  }

  /** Leo's request failed: stop talking and go back to listening. */
  failResponse(): void {
    if (!this.active) return;
    this.silence();
    this.metrics.discard();
    this.state.go("listening");
  }

  /** Stops talking and drops anything queued, without touching Leo's generation (New chat, switching chats). */
  stopSpeaking(): void {
    if (!this.active) return;
    this.silence();
    this.metrics.discard();
    if (this.state.is("speaking", "thinking", "interrupted")) this.state.go("listening");
  }

  /** Says something outside a normal reply (for example the question on a card). */
  say(text: string): void {
    if (!this.active) return;
    this.beginResponse();
    this.feedLeoText(text.endsWith("\n") ? text : `${text}\n`);
    this.endLeoText("");
  }

  // ---------- the microphone side ----------

  private onFrame(probability: number, frame: Float32Array): void {
    this.micRms = this.micRms * 0.7 + rms(frame) * 0.3;
    if (this.vadSpeaking) this.utterance.push(frame);
    else {
      this.preRoll.push(frame);
      if (this.preRoll.length > PRE_ROLL_FRAMES) this.preRoll.shift();
    }

    // While Leo is talking (or working), only a confident, sustained voice counts.
    if (this.state.is("speaking", "thinking") && this.bargeIn.push(probability, this.player.level())) {
      this.confirmBargeIn();
    }
  }

  private onSpeechStart(): void {
    if (this.muted) return;
    this.vadSpeaking = true;
    this.utterance = [...this.preRoll];
    if (this.state.is("speaking", "thinking")) this.metrics.mark("interruptionTime", this.now());

    if (this.state.is("listening", "user_speaking")) {
      this.beginUserSpeech();
    }
    // Otherwise Leo is talking or working: this is only a candidate until the
    // barge-in detector confirms it is really the user.
  }

  private beginUserSpeech(): void {
    this.accepted = true;
    if (this.holdTimer) {
      // The user carried on: this is the same turn, not a new one.
      clearTimeout(this.holdTimer);
      this.holdTimer = null;
    }
    if (this.segTexts.length === 0) this.metrics.mark("speechStart", this.now());
    this.state.go("user_speaking");
    this.emit("speechStart", undefined);
    this.startPartials();
  }

  /** The user is cutting in on Leo. Audio stops first; everything else follows. */
  private confirmBargeIn(): void {
    this.bargeIn.reset();
    if (!this.vadSpeaking) return;
    this.interrupt();
    this.beginUserSpeech();
  }

  /** Stops Leo mid-reply: audio, queued speech and the generation itself. */
  private interrupt(): void {
    this.silence();
    this.metrics.mark("interruptionStopped", this.now());
    this.metrics.finish(); // closes the interrupted reply's timing
    this.state.go("interrupted");
    this.deps.cancelResponse();
    this.emit("bargeIn", undefined);
  }

  private onSpeechEnd(audio: Float32Array): void {
    const wasSpeaking = this.vadSpeaking;
    this.vadSpeaking = false;
    this.stopPartials();
    this.bargeIn.reset();
    if (!wasSpeaking || !this.accepted || this.muted) {
      // A candidate that never became a real interruption (echo, noise).
      this.accepted = false;
      this.metrics.unmark("interruptionTime");
      return;
    }
    this.lastSpeechEnd = this.now();
    this.metrics.markLatest("speechEnd", this.lastSpeechEnd);

    const token = this.turnToken;
    this.segmentsPending++;
    void this.stt.transcribe(audio, { final: true }).then((text) => {
      this.segmentsPending--;
      if (token !== this.turnToken) return;
      if (text) this.segTexts.push(text);
      this.evaluateTurn();
    });
  }

  private onMisfire(): void {
    if (!this.vadSpeaking) return;
    this.vadSpeaking = false;
    this.stopPartials();
    this.bargeIn.reset();
    if (this.accepted) {
      this.accepted = false;
      this.evaluateTurn();
    }
  }

  /** All the user's speech so far has been transcribed: submit, wait for more, or drop it. */
  private evaluateTurn(): void {
    if (this.vadSpeaking || this.segmentsPending > 0) return;
    const text = this.segTexts.join(" ").replace(/\s+/g, " ").trim();
    this.metrics.markLatest("transcriptFinal", this.now());
    if (!this.turns.isSubmittable(text)) {
      this.discardTurn();
      return;
    }
    const wait = this.turns.holdMs(text) - (this.now() - this.lastSpeechEnd);
    if (this.holdTimer) clearTimeout(this.holdTimer);
    if (wait <= 0) this.submitTurn(text);
    else this.holdTimer = setTimeout(() => this.submitTurn(text), wait);
  }

  private submitTurn(text: string): void {
    this.holdTimer = null;
    this.resetTurn();
    this.state.go("thinking");
    this.metrics.mark("submitted", this.now());
    this.emit("finalTranscript", { text });
    try {
      const result = this.deps.submit(text);
      if (result && typeof (result as Promise<void>).catch === "function") {
        (result as Promise<void>).catch((err) => this.leoFailed(err));
      }
    } catch (err) {
      this.leoFailed(err);
    }
  }

  private leoFailed(err: unknown): void {
    this.emit("error", { message: describe(err), source: "leo", fatal: false });
    this.failResponse();
  }

  /** Nothing worth sending was said. Go back to what we were doing. */
  private discardTurn(): void {
    this.resetTurn();
    this.metrics.discard();
    if (this.active) this.state.go(this.responseActive && !this.responseDone ? "thinking" : "listening");
  }

  private resetTurn(): void {
    this.turnToken++;
    this.segTexts = [];
    this.segmentsPending = 0;
    this.accepted = false;
    this.utterance = [];
    this.stopPartials();
    if (this.holdTimer) clearTimeout(this.holdTimer);
    this.holdTimer = null;
  }

  // ---------- partial transcripts ----------

  private startPartials(): void {
    if (this.partialTimer) return;
    this.partialTimer = setInterval(() => void this.emitPartial(), PARTIAL_EVERY_MS);
  }

  private stopPartials(): void {
    if (this.partialTimer) clearInterval(this.partialTimer);
    this.partialTimer = null;
  }

  private async emitPartial(): Promise<void> {
    if (this.partialBusy || !this.vadSpeaking || !this.accepted || this.now() < this.nextPartialAt) return;
    let samples = this.utterance.reduce((n, f) => n + f.length, 0);
    if (samples < PARTIAL_MIN_SAMPLES) return;
    const frames = this.utterance.slice();
    while (samples > PARTIAL_MAX_SAMPLES && frames.length > 1) samples -= frames.shift()!.length;
    const token = this.turnToken;
    this.partialBusy = true;
    const began = this.now();
    try {
      const text = await this.stt.transcribe(concat(frames), { final: false });
      if (text && token === this.turnToken && this.vadSpeaking) {
        this.emit("partialTranscript", { text: [...this.segTexts, text].join(" ") });
      }
    } finally {
      this.partialBusy = false;
      // A slow recognizer must not be kept busy by previews: the final transcript
      // would queue behind them. Leave idle time in proportion to how long one took.
      this.nextPartialAt = this.now() + (this.now() - began);
    }
  }

  // ---------- text -> audio ----------

  private speakAll(sentences: string[]): void {
    for (const sentence of sentences) {
      const text = speechTextNormalizer(sentence);
      if (/[\p{L}\p{N}]/u.test(text)) this.queue.push({ text, epoch: this.epoch });
    }
    void this.pump();
  }

  private async pump(): Promise<void> {
    if (this.pumping) return;
    this.pumping = true;
    try {
      while (this.queue.length > 0) {
        const job = this.queue.shift()!;
        if (job.epoch !== this.epoch) continue;
        this.metrics.mark("ttsStart", this.now());
        try {
          for await (const chunk of this.tts.synthesize(job.text, { voice: this.voice })) {
            if (job.epoch !== this.epoch) break; // interrupted while synthesizing
            this.metrics.mark("ttsFirstAudio", this.now());
            if (this.player.schedule(chunk.samples, chunk.sampleRate, job.epoch)) this.enterSpeaking();
          }
        } catch (err) {
          if (job.epoch === this.epoch && !this.ttsErrored) {
            this.ttsErrored = true; // once per reply is enough; the text is still in the chat
            this.emit("error", { message: describe(err), source: "tts", fatal: false });
          }
        }
      }
    } finally {
      this.pumping = false;
    }
    this.finishIfDrained();
  }

  private enterSpeaking(): void {
    if (this.state.is("thinking", "listening")) {
      this.bargeIn.reset();
      this.state.go("speaking");
    }
  }

  /** The reply is complete and every sentence has been played. */
  private finishIfDrained(): void {
    if (!this.responseDone || this.pumping || this.queue.length > 0) return;
    const epoch = this.epoch;
    this.player.whenDrained(epoch, () => {
      if (epoch !== this.epoch) return;
      this.responseActive = false;
      this.metrics.finish();
      if (this.state.is("speaking", "thinking")) this.state.go("listening");
    });
  }

  /** Drops all speech: new epoch, playing audio flushed, queued sentences and synthesis cancelled. */
  private silence(): void {
    this.epoch++;
    this.accepting = false;
    this.responseActive = false;
    this.responseDone = false;
    this.ttsErrored = false;
    this.queue = [];
    this.chunker.reset();
    this.tts.cancel();
    this.player.flush(this.epoch);
  }

  private clearTimers(): void {
    this.stopPartials();
    if (this.holdTimer) clearTimeout(this.holdTimer);
    this.holdTimer = null;
  }

  private emit<K extends VoiceEventName>(name: K, payload: VoiceEventMap[K]): void {
    for (const handler of this.events.get(name) ?? []) {
      try {
        (handler as Handler<K>)(payload);
      } catch (err) {
        console.error(`Leo voice: a ${name} listener failed`, err);
      }
    }
  }
}

function rms(frame: Float32Array): number {
  let sum = 0;
  for (let i = 0; i < frame.length; i++) sum += frame[i] * frame[i];
  return Math.sqrt(sum / Math.max(1, frame.length));
}

function concat(frames: Float32Array[]): Float32Array {
  const out = new Float32Array(frames.reduce((n, f) => n + f.length, 0));
  let offset = 0;
  for (const f of frames) {
    out.set(f, offset);
    offset += f.length;
  }
  return out;
}

function describe(err: unknown): string {
  if (err instanceof DOMException && err.name === "NotAllowedError") {
    return "Microphone access was blocked. Allow it for Leo and try again.";
  }
  if (err instanceof DOMException && err.name === "NotFoundError") return "No microphone was found.";
  return err instanceof Error ? err.message : String(err);
}

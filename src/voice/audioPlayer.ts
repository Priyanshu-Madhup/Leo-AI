import type { Player } from "./types";

/**
 * One persistent speaker path for all of Leo's speech.
 *
 * - Chunks are scheduled back to back on the AudioContext clock (`nextAt`),
 *   so there is no gap between sentences and nothing waits for a chunk to end.
 * - Every chunk carries the epoch of the reply it belongs to. `flush(epoch)`
 *   stops what is playing, forgets what is scheduled and moves to the new
 *   epoch; a late chunk from an older epoch is refused. That is what makes it
 *   impossible for an old reply to play over a new one.
 * - Output goes AudioContext -> MediaStreamDestination -> a hidden <audio>
 *   element, not straight to the context's destination. The browser's echo
 *   canceller uses the media element's output as the reference for what the
 *   microphone should NOT treat as the user, which keeps Leo from hearing
 *   itself. If that route is unavailable the player falls back to the
 *   context's own output.
 */

/** The small slice of the Web Audio API the player uses, so tests can fake it. */
export interface AudioPlatform {
  createContext(): AudioContext;
  createElement(): HTMLAudioElement | null;
}

const browserPlatform: AudioPlatform = {
  createContext: () => new AudioContext({ latencyHint: "interactive" }),
  createElement: () => (typeof Audio === "undefined" ? null : new Audio()),
};

/** A little head start so the first chunk does not start in the past. */
const START_LEAD = 0.04;

export class AudioPlayer implements Player {
  private ctx: AudioContext | null = null;
  private analyser: AnalyserNode | null = null;
  private input: GainNode | null = null;
  private element: HTMLAudioElement | null = null;
  private meter: Float32Array<ArrayBuffer> | null = null;

  private currentEpoch = 0;
  private nextAt = 0;
  private sources = new Set<AudioBufferSourceNode>();
  private startTimer: ReturnType<typeof setTimeout> | null = null;
  private drainCallbacks: { epoch: number; callback: () => void }[] = [];
  private playing = false;

  onAudioStart: (() => void) | null = null;
  onAudioEnd: (() => void) | null = null;

  constructor(private platform: AudioPlatform = browserPlatform) {}

  get epoch(): number {
    return this.currentEpoch;
  }

  get isPlaying(): boolean {
    return this.playing;
  }

  /** Creates the audio graph (once) and resumes it. Must run after a user gesture. */
  async ensure(): Promise<void> {
    if (!this.ctx) {
      const ctx = this.platform.createContext();
      const input = ctx.createGain();
      const analyser = ctx.createAnalyser();
      analyser.fftSize = 1024;
      input.connect(analyser);

      let routed = false;
      try {
        const element = this.platform.createElement();
        if (element && "srcObject" in element) {
          const destination = ctx.createMediaStreamDestination();
          analyser.connect(destination);
          element.srcObject = destination.stream;
          element.autoplay = true;
          await element.play();
          this.element = element;
          routed = true;
        }
      } catch (err) {
        console.warn("Leo voice: could not route speech through a media element, using direct output", err);
      }
      if (!routed) analyser.connect(ctx.destination);

      this.ctx = ctx;
      this.input = input;
      this.analyser = analyser;
      this.meter = new Float32Array(analyser.fftSize);
    }
    if (this.ctx.state === "suspended") await this.ctx.resume();
  }

  /**
   * Queues a chunk right after the previous one. Returns false (and plays
   * nothing) for an empty chunk or one from an old epoch.
   */
  schedule(samples: Float32Array, sampleRate: number, epoch: number): boolean {
    if (epoch !== this.currentEpoch || samples.length === 0 || !this.ctx || !this.input) return false;
    const ctx = this.ctx;

    const buffer = ctx.createBuffer(1, samples.length, sampleRate);
    buffer.copyToChannel(samples as Float32Array<ArrayBuffer>, 0);
    const source = ctx.createBufferSource();
    source.buffer = buffer;
    source.connect(this.input);

    const now = ctx.currentTime;
    const wasIdle = this.sources.size === 0 || this.nextAt < now;
    // Back to back with the last chunk; if playback ran dry, start just ahead of now.
    if (wasIdle) this.nextAt = now + START_LEAD;
    const startAt = this.nextAt;
    this.nextAt = startAt + buffer.duration;

    this.sources.add(source);
    source.onended = () => {
      this.sources.delete(source);
      source.disconnect();
      if (this.sources.size === 0) this.settle(epoch);
    };
    source.start(startAt);

    if (!this.playing && this.startTimer === null) {
      const delay = Math.max(0, (startAt - now) * 1000);
      this.startTimer = setTimeout(() => {
        this.startTimer = null;
        this.playing = true;
        this.onAudioStart?.();
      }, delay);
    }
    return true;
  }

  /**
   * Cuts everything off and moves to `epoch`: playing sources stop at once,
   * scheduled ones are discarded, waiting callbacks are dropped and the
   * playback clock starts over.
   */
  flush(epoch: number): void {
    this.currentEpoch = epoch;
    for (const source of this.sources) {
      source.onended = null;
      try {
        source.stop();
      } catch {
        /* already stopped */
      }
      source.disconnect();
    }
    this.sources.clear();
    if (this.startTimer !== null) {
      clearTimeout(this.startTimer);
      this.startTimer = null;
    }
    this.drainCallbacks = [];
    this.nextAt = 0;
    if (this.playing) {
      this.playing = false;
      this.onAudioEnd?.();
    }
  }

  /** Calls back when everything scheduled for `epoch` has finished playing. */
  whenDrained(epoch: number, callback: () => void): void {
    if (epoch !== this.currentEpoch) return;
    if (this.sources.size === 0 && this.startTimer === null) {
      callback();
      return;
    }
    this.drainCallbacks.push({ epoch, callback });
  }

  /** RMS loudness of what is actually coming out, 0..1 (for the orb). */
  level(): number {
    if (!this.analyser || !this.meter || !this.playing) return 0;
    this.analyser.getFloatTimeDomainData(this.meter);
    let sum = 0;
    for (let i = 0; i < this.meter.length; i++) sum += this.meter[i] * this.meter[i];
    return Math.min(1, Math.sqrt(sum / this.meter.length) * 3);
  }

  /** Coarse low/mid/high energy of the output, 0..1 each, for spectrum effects. */
  bands(): [number, number, number] {
    if (!this.analyser || !this.playing) return [0, 0, 0];
    const bins = new Uint8Array(this.analyser.frequencyBinCount);
    this.analyser.getByteFrequencyData(bins);
    const third = Math.floor(bins.length / 3);
    const avg = (from: number, to: number) => {
      let sum = 0;
      for (let i = from; i < to; i++) sum += bins[i];
      return sum / Math.max(1, to - from) / 255;
    };
    return [avg(0, third), avg(third, third * 2), avg(third * 2, bins.length)];
  }

  dispose(): void {
    this.flush(this.currentEpoch + 1);
    this.element?.pause();
    if (this.element) this.element.srcObject = null;
    this.element = null;
    void this.ctx?.close();
    this.ctx = null;
    this.analyser = null;
    this.input = null;
  }

  private settle(epoch: number): void {
    if (epoch !== this.currentEpoch) return;
    // Everything queued has played; later chunks (if any) start a new run.
    const ready = this.drainCallbacks.filter((c) => c.epoch === epoch);
    this.drainCallbacks = this.drainCallbacks.filter((c) => c.epoch !== epoch);
    if (this.playing) {
      this.playing = false;
      this.onAudioEnd?.();
    }
    for (const { callback } of ready) callback();
  }
}

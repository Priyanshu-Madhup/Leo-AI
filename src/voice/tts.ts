import type { PcmChunk, TTS, TTSOptions } from "./types";

/** An async queue: the worker pushes pieces, the consumer awaits them. */
class PieceQueue implements AsyncIterable<PcmChunk> {
  private items: PcmChunk[] = [];
  private waiter: (() => void) | null = null;
  private finished = false;
  private failure: Error | null = null;

  push(piece: PcmChunk) {
    this.items.push(piece);
    this.wake();
  }
  end(error?: Error) {
    this.finished = true;
    this.failure = error ?? null;
    this.wake();
  }
  private wake() {
    this.waiter?.();
    this.waiter = null;
  }

  async *[Symbol.asyncIterator](): AsyncIterator<PcmChunk> {
    for (;;) {
      const next = this.items.shift();
      if (next) {
        yield next;
        continue;
      }
      if (this.finished) {
        if (this.failure) throw this.failure;
        return;
      }
      await new Promise<void>((resolve) => (this.waiter = resolve));
    }
  }
}

/**
 * Text to speech with Kokoro in a worker. `synthesize` yields PCM as each piece
 * is produced. `cancel()` ends every running iterator at once and tells the
 * worker to drop the work it has not started.
 */
export class KokoroWorkerTTS implements TTS {
  private worker: Worker | null = null;
  private ready: Promise<void> | null = null;
  private nextJob = 1;
  private jobs = new Map<number, PieceQueue>();
  private onProgress: ((fraction: number) => void) | undefined;

  constructor(private config: { device?: "webgpu" | "wasm"; dtype?: "fp32" | "fp16" | "q8" | "q4" | "q4f16" } = {}) {}

  load(onProgress?: (fraction: number) => void): Promise<void> {
    this.onProgress = onProgress;
    this.ready ??= new Promise<void>((resolve, reject) => {
      const worker = new Worker(new URL("./tts.worker.ts", import.meta.url), { type: "module" });
      this.worker = worker;
      worker.onmessage = (event: MessageEvent) => {
        const msg = event.data;
        if (msg.type === "log") console.info(msg.message);
        else if (msg.type === "progress") this.onProgress?.(msg.fraction);
        else if (msg.type === "ready") resolve();
        else if (msg.type === "error") {
          this.ready = null;
          reject(new Error(msg.message));
        } else if (msg.type === "chunk") this.jobs.get(msg.job)?.push({ samples: msg.samples, sampleRate: msg.sampleRate });
        else if (msg.type === "done") this.settle(msg.job);
        else if (msg.type === "failed") this.settle(msg.job, new Error(msg.message));
      };
      worker.onerror = (e) => {
        this.ready = null;
        reject(new Error(e.message || "The speech worker failed."));
      };
      worker.postMessage({ type: "load", ...this.config });
    });
    return this.ready;
  }

  synthesize(text: string, options: TTSOptions = {}): AsyncIterable<PcmChunk> {
    const queue = new PieceQueue();
    const job = this.nextJob++;
    if (!this.worker) {
      queue.end(new Error("Speech is not loaded."));
      return queue;
    }
    this.jobs.set(job, queue);
    this.worker.postMessage({ type: "synthesize", job, text, voice: options.voice, speed: options.speed });
    return queue;
  }

  cancel(): void {
    this.worker?.postMessage({ type: "cancel", upTo: this.nextJob - 1 });
    for (const queue of this.jobs.values()) queue.end();
    this.jobs.clear();
  }

  dispose(): void {
    this.cancel();
    this.worker?.terminate();
    this.worker = null;
    this.ready = null;
  }

  private settle(job: number, error?: Error) {
    this.jobs.get(job)?.end(error);
    this.jobs.delete(job);
  }
}

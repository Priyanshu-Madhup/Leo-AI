import type { STT } from "./types";

interface Job {
  id: number;
  audio: Float32Array;
  final: boolean;
  resolve: (text: string | null) => void;
}

/**
 * Speech to text through Whisper in a worker. Whisper is not a streaming
 * model, so "streaming" is done the usual way: while the user talks, the audio
 * so far is transcribed again every so often and shown as the partial text; the
 * finished segment is transcribed once more for the final text.
 *
 * Only one request runs at a time. A newer partial replaces an older waiting
 * one (which resolves to null), and a final request always goes first.
 */
export class WhisperSTT implements STT {
  private worker: Worker | null = null;
  private ready: Promise<void> | null = null;
  private nextId = 1;
  private busy: Job | null = null;
  private waiting: Job[] = [];
  private onProgress: ((fraction: number) => void) | undefined;

  constructor(private config: { model?: string; device?: "webgpu" | "wasm" } = {}) {}

  load(onProgress?: (fraction: number) => void): Promise<void> {
    this.onProgress = onProgress;
    this.ready ??= new Promise<void>((resolve, reject) => {
      const worker = new Worker(new URL("./stt.worker.ts", import.meta.url), { type: "module" });
      this.worker = worker;
      worker.onmessage = (event: MessageEvent) => {
        const msg = event.data;
        if (msg.type === "log") console.info(msg.message);
        else if (msg.type === "progress") this.onProgress?.(msg.fraction);
        else if (msg.type === "ready") resolve();
        else if (msg.type === "error") {
          this.ready = null;
          reject(new Error(msg.message));
        } else if (msg.type === "result") this.finished(msg.id, msg.text, msg.error);
      };
      worker.onerror = (e) => {
        this.ready = null;
        reject(new Error(e.message || "The speech recognition worker failed."));
      };
      worker.postMessage({ type: "load", ...this.config });
    });
    return this.ready;
  }

  transcribe(audio: Float32Array, options: { final: boolean }): Promise<string | null> {
    return new Promise((resolve) => {
      const job: Job = { id: this.nextId++, audio, final: options.final, resolve };
      if (!options.final) {
        // Only the newest partial is worth computing.
        this.waiting = this.waiting.filter((j) => {
          if (j.final) return true;
          j.resolve(null);
          return false;
        });
      }
      this.waiting.push(job);
      this.pump();
    });
  }

  dispose(): void {
    for (const job of [...this.waiting, ...(this.busy ? [this.busy] : [])]) job.resolve(null);
    this.waiting = [];
    this.busy = null;
    this.worker?.terminate();
    this.worker = null;
    this.ready = null;
  }

  private pump(): void {
    if (this.busy || !this.worker) return;
    const index = this.waiting.findIndex((j) => j.final);
    const job = this.waiting.splice(index >= 0 ? index : 0, 1)[0];
    if (!job) return;
    this.busy = job;
    // The audio is copied: the caller may still need it (partials re-read the buffer).
    const copy = job.audio.slice();
    this.worker.postMessage({ type: "transcribe", id: job.id, audio: copy }, [copy.buffer]);
  }

  private finished(id: number, text: string, error?: string): void {
    const job = this.busy;
    if (!job || job.id !== id) return;
    this.busy = null;
    if (error) console.warn("Leo voice: transcription failed:", error);
    job.resolve(error ? null : cleanTranscript(text));
    this.pump();
  }
}

/** Removes the bracketed non-speech markers Whisper writes for silence and noise. */
export function cleanTranscript(text: string): string {
  return text
    .replace(/\[[^\]]*\]|\([^)]*\)|[♪♫]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

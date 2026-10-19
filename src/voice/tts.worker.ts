/// <reference lib="webworker" />
import { KokoroTTS, TextSplitterStream } from "kokoro-js";

// Kokoro runs off the main thread; each sentence comes back as PCM pieces as
// soon as they exist, so the first audio does not wait for the whole sentence.
const MODEL = "onnx-community/Kokoro-82M-v1.0-ONNX";

type Request =
  | { type: "load"; device?: "webgpu" | "wasm"; dtype?: "fp32" | "fp16" | "q8" | "q4" | "q4f16" }
  | { type: "synthesize"; job: number; text: string; voice?: string; speed?: number }
  | { type: "cancel"; upTo: number };

let tts: KokoroTTS | null = null;
let loading: Promise<void> | null = null;
let cancelledUpTo = 0;

function note(message: string) {
  report({ type: "log", message });
}

function report(message: unknown, transfer: Transferable[] = []) {
  (self as unknown as Worker).postMessage(message, transfer);
}

/**
 * kokoro-js only ends a stream when its splitter is closed, and a plain string
 * is never closed for you, so the last sentence would wait forever.
 */
function closedText(text: string): TextSplitterStream {
  const splitter = new TextSplitterStream();
  splitter.push(text);
  splitter.close();
  return splitter;
}

async function hasWebGpu(): Promise<boolean> {
  try {
    const gpu = (navigator as unknown as { gpu?: { requestAdapter(): Promise<unknown> } }).gpu;
    return !!gpu && !!(await gpu.requestAdapter());
  } catch {
    return false;
  }
}

async function load(forced?: "webgpu" | "wasm", dtype?: "fp32" | "fp16" | "q8" | "q4" | "q4f16"): Promise<void> {
  // Downloads arrive file by file; report one overall fraction.
  const files = new Map<string, { loaded: number; total: number }>();
  const progress_callback = (p: { status?: string; file?: string; loaded?: number; total?: number }) => {
    if (p.status !== "progress" || !p.file || !p.total) return;
    files.set(p.file, { loaded: p.loaded ?? 0, total: p.total });
    let loaded = 0;
    let total = 0;
    for (const f of files.values()) (loaded += f.loaded, (total += f.total));
    report({ type: "progress", fraction: Math.min(1, loaded / total) });
  };
  note("[leo voice] speech: loading");
  try {
    if (forced === "webgpu" || (!forced && (await hasWebGpu()))) {
      tts = await KokoroTTS.from_pretrained(MODEL, { dtype: dtype ?? "fp32", device: "webgpu", progress_callback });
    }
  } catch (err) {
    console.warn("Leo voice: WebGPU speech failed, falling back to wasm", err);
    tts = null;
  }
  tts ??= await KokoroTTS.from_pretrained(MODEL, { dtype: dtype ?? "q8", device: "wasm", progress_callback });
  note(`[leo voice] speech: running on ${forced ?? "auto"}`);
  note("[leo voice] speech: model loaded, warming up");
  // Warm-up so the first spoken sentence is not the one that pays for compilation.
  for await (const _ of tts.stream(closedText("Ready."), { voice: "af_heart" })) void _;
  note("[leo voice] speech: ready");
}

self.onmessage = async (event: MessageEvent<Request>) => {
  const msg = event.data;
  if (msg.type === "cancel") {
    cancelledUpTo = Math.max(cancelledUpTo, msg.upTo);
    return;
  }
  if (msg.type === "load") {
    try {
      loading ??= load(msg.device, msg.dtype);
      await loading;
      report({ type: "ready" });
    } catch (err) {
      loading = null;
      report({ type: "error", message: err instanceof Error ? err.message : String(err) });
    }
    return;
  }
  const { job } = msg;
  try {
    if (!tts) throw new Error("Speech is not loaded yet.");
    for await (const piece of tts.stream(closedText(msg.text), { voice: (msg.voice ?? "af_heart") as never, speed: msg.speed ?? 1 })) {
      if (job <= cancelledUpTo) return; // dropped: say nothing more
      const samples = new Float32Array(piece.audio.audio);
      report({ type: "chunk", job, samples, sampleRate: piece.audio.sampling_rate }, [samples.buffer]);
    }
    report({ type: "done", job });
  } catch (err) {
    report({ type: "failed", job, message: err instanceof Error ? err.message : String(err) });
  }
};

/// <reference lib="webworker" />
import { pipeline, env } from "@huggingface/transformers";

// Whisper runs off the main thread so a transcription never stalls the orb.
const WHISPER_MODEL = "onnx-community/whisper-base.en";

env.allowLocalModels = false;

type Request =
  | { type: "load"; model?: string; device?: "webgpu" | "wasm" }
  | { type: "transcribe"; id: number; audio: Float32Array };

let asr: ((audio: Float32Array, options: Record<string, unknown>) => Promise<{ text: string } | { text: string }[]>) | null = null;
let loading: Promise<void> | null = null;

function note(message: string) {
  report({ type: "log", message });
}

function report(message: unknown) {
  (self as unknown as Worker).postMessage(message);
}

async function hasWebGpu(): Promise<boolean> {
  try {
    const gpu = (navigator as unknown as { gpu?: { requestAdapter(): Promise<unknown> } }).gpu;
    return !!gpu && !!(await gpu.requestAdapter());
  } catch {
    return false;
  }
}

async function load(model: string, forced?: "webgpu" | "wasm"): Promise<void> {
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
  const build = (device: "webgpu" | "wasm") =>
    pipeline("automatic-speech-recognition", model, {
      device,
      dtype: device === "webgpu" ? { encoder_model: "fp32", decoder_model_merged: "q4" } : "q8",
      progress_callback,
    });
  const device = forced ?? ((await hasWebGpu()) ? "webgpu" : "wasm");
  note(`[leo voice] speech recognition: ${model} on ${device}`);
  try {
    asr = (await build(device)) as unknown as typeof asr;
  } catch (err) {
    console.warn("Leo voice: WebGPU speech recognition failed, falling back to wasm", err);
    asr = (await build("wasm")) as unknown as typeof asr;
  }
  // One throwaway pass compiles the kernels so the first real request is fast.
  note("[leo voice] speech recognition: model loaded, warming up");
  await asr!(new Float32Array(16000), {});
  note("[leo voice] speech recognition: ready");
}

self.onmessage = async (event: MessageEvent<Request>) => {
  const msg = event.data;
  if (msg.type === "load") {
    try {
      loading ??= load(msg.model ?? WHISPER_MODEL, msg.device);
      await loading;
      report({ type: "ready" });
    } catch (err) {
      loading = null;
      report({ type: "error", message: err instanceof Error ? err.message : String(err) });
    }
    return;
  }
  try {
    if (!asr) throw new Error("Speech recognition is not loaded yet.");
    const out = await asr(msg.audio, { chunk_length_s: 30, return_timestamps: false });
    const text = (Array.isArray(out) ? out.map((o) => o.text).join(" ") : out.text).trim();
    report({ type: "result", id: msg.id, text });
  } catch (err) {
    report({ type: "result", id: msg.id, text: "", error: err instanceof Error ? err.message : String(err) });
  }
};

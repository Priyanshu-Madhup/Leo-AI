import { MicVAD } from "@ricky0123/vad-web";
import type { Vad, VadEvents } from "./types";
// The ONNX runtime's loader and wasm, as bundled assets (a plain /public file
// cannot be imported as a module by the dev server).
import ortLoader from "../../node_modules/onnxruntime-web/dist/ort-wasm-simd-threaded.mjs?url";
import ortWasm from "../../node_modules/onnxruntime-web/dist/ort-wasm-simd-threaded.wasm?url";

/**
 * Silero voice-activity detection on the live microphone.
 *
 * The microphone is opened once when voice mode starts and stays open until it
 * ends (`pause` is the mute button). The stream asks the browser for echo
 * cancellation, noise suppression and gain control; together with the speaker
 * routing in the audio player this is what keeps Leo's own voice out of what
 * the detector hears.
 *
 * The model and the audio worklet are served from /vad/ (copied there by
 * scripts/copy-voice-assets.mjs) and the ONNX runtime is bundled with the app,
 * so starting the microphone needs no network.
 */
export class SileroVad implements Vad {
  private vad: MicVAD | null = null;
  private events: VadEvents | null = null;
  private wanted = false;
  private reconnects = 0;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;

  async start(events: VadEvents): Promise<void> {
    this.events = events;
    this.wanted = true;
    this.reconnects = 0;
    await this.create();
  }

  async stop(): Promise<void> {
    this.wanted = false;
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    await this.destroy();
  }

  async pause(): Promise<void> {
    await this.vad?.pause();
  }

  async resume(): Promise<void> {
    await this.vad?.start();
  }

  private async create(): Promise<void> {
    const events = this.events!;
    this.vad = await MicVAD.new({
      model: "v5",
      baseAssetPath: "/vad/",
      onnxWASMBasePath: "/vad/", // unused: wasmPaths below names the files outright
      ortConfig: (ort) => {
        ort.env.wasm.wasmPaths = { mjs: ortLoader, wasm: ortWasm };
        // Without cross-origin isolation the runtime cannot use threads anyway.
        ort.env.wasm.numThreads = 1;
      },
      positiveSpeechThreshold: 0.5,
      negativeSpeechThreshold: 0.35,
      redemptionMs: 600,
      preSpeechPadMs: 320,
      minSpeechMs: 250,
      getStream: async () => {
        const stream = await navigator.mediaDevices.getUserMedia({
          audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true, autoGainControl: true },
        });
        for (const track of stream.getAudioTracks()) track.addEventListener("ended", () => this.lost());
        return stream;
      },
      onFrameProcessed: (probabilities, frame) => events.onFrame(probabilities.isSpeech, frame),
      onSpeechStart: () => events.onSpeechStart(),
      onSpeechEnd: (audio) => events.onSpeechEnd(audio),
      onVADMisfire: () => events.onMisfire(),
    });
    await this.vad.start();
    this.reconnects = 0;
  }

  private async destroy(): Promise<void> {
    const vad = this.vad;
    this.vad = null;
    try {
      await vad?.destroy();
    } catch {
      /* already gone */
    }
  }

  /** The microphone vanished (unplugged, permission revoked). Try to get it back. */
  private lost(): void {
    if (!this.wanted || this.reconnectTimer) return;
    this.events?.onError(new Error("The microphone was disconnected."));
    const attempt = () => {
      this.reconnectTimer = null;
      if (!this.wanted) return;
      void (async () => {
        await this.destroy();
        try {
          await this.create();
        } catch (err) {
          if (++this.reconnects >= 5) {
            this.events?.onError(new Error("Could not get the microphone back."));
            return;
          }
          this.reconnectTimer = setTimeout(attempt, 2000);
          void err;
        }
      })();
    };
    this.reconnectTimer = setTimeout(attempt, 1500);
  }
}

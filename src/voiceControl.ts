import { AudioPlayer } from "./voice/audioPlayer";
import { VoiceRuntime, type VoiceDeps } from "./voice/runtime";
import { WhisperSTT } from "./voice/stt";
import { KokoroWorkerTTS } from "./voice/tts";
import { SileroVad } from "./voice/vad";
import type { VoicePhase } from "./voice/types";
import type { OrbState } from "./orb";

/**
 * Connects the voice runtime to Leo's window: the microphone buttons, the
 * status line under the chat, and the orb. The runtime itself stays free of
 * DOM; everything it does reaches the screen through here.
 */
export interface VoiceHooks extends VoiceDeps {
  setOrbState(state: OrbState): void;
  setOrbLevel(level: number): void;
  /** Shows a problem in the chat. */
  showError(message: string): void;
}

const STORAGE_VOICE = "leo.voice.name";

const LINE_TEXT: Partial<Record<VoicePhase, string>> = {
  listening: "Listening…",
  user_speaking: "Listening…",
  interrupted: "Listening…",
};

export class VoiceControl {
  readonly runtime: VoiceRuntime;
  private line = document.querySelector<HTMLDivElement>("#voice-line")!;
  private micBtn = document.querySelector<HTMLButtonElement>("#mic-btn")!;
  private muteBtn = document.querySelector<HTMLButtonElement>("#voice-mute-btn")!;
  private raf = 0;
  private partial = "";

  constructor(private hooks: VoiceHooks) {
    this.runtime = new VoiceRuntime(hooks, {
      vad: new SileroVad(),
      stt: new WhisperSTT(),
      tts: new KokoroWorkerTTS(),
      player: new AudioPlayer(),
      voice: readVoice(),
      debug: import.meta.env.DEV,
    });
    if (import.meta.env.DEV) (window as unknown as { __leoVoice: unknown }).__leoVoice = this.runtime;

    this.runtime.on("phase", ({ phase }) => this.onPhase(phase));
    this.runtime.on("partialTranscript", ({ text }) => {
      this.partial = text;
      this.line.textContent = text;
    });
    this.runtime.on("finalTranscript", () => {
      this.partial = "";
      this.line.textContent = "";
    });
    this.runtime.on("progress", ({ fraction }) => {
      this.line.textContent = `Getting voice ready… ${Math.round(fraction * 100)}%`;
    });
    this.runtime.on("error", ({ message, fatal }) => {
      // A voice problem is shown, never thrown: text chat keeps working.
      if (fatal || message) this.hooks.showError(`Voice: ${message}`);
    });

    this.micBtn.addEventListener("click", () => void this.toggle());
    this.muteBtn.addEventListener("click", () => {
      this.runtime.setMuted(!this.runtime.isMuted);
      this.line.textContent = this.runtime.isMuted ? "Microphone muted" : "Listening…";
      this.syncButtons();
    });
    this.runtime.on("phase", () => this.syncButtons());
  }

  /** True while Leo's voice, not the text typer, should drive the orb. */
  get ownsOrb(): boolean {
    return this.runtime.isSpeaking || this.runtime.phase === "user_speaking";
  }

  async toggle(): Promise<void> {
    if (this.runtime.phase === "loading") return;
    if (this.runtime.active) await this.runtime.stop();
    else await this.runtime.start();
  }

  private onPhase(phase: VoicePhase): void {
    if (phase === "idle") {
      this.line.textContent = "";
      this.hooks.setOrbLevel(0);
      cancelAnimationFrame(this.raf);
      this.raf = 0;
      return;
    }
    if (phase === "loading") this.line.textContent = "Getting voice ready…";
    else if (this.runtime.isMuted) this.line.textContent = "Microphone muted";
    else if (phase === "thinking" || phase === "speaking") this.line.textContent = "";
    else if (LINE_TEXT[phase] && !this.partial) this.line.textContent = LINE_TEXT[phase]!;

    if (phase === "speaking") this.hooks.setOrbState("speaking");
    else if (phase === "thinking") this.hooks.setOrbState("thinking");
    else if (phase === "listening" || phase === "user_speaking") this.hooks.setOrbState("idle");

    if (phase !== "loading" && !this.raf) this.raf = requestAnimationFrame(() => this.frame());
  }

  /** The orb follows what is actually coming out of the speaker (or going into the microphone). */
  private frame(): void {
    this.raf = 0;
    if (!this.runtime.active) return;
    if (this.runtime.phase === "speaking") this.hooks.setOrbLevel(this.runtime.outputLevel());
    else if (this.runtime.phase === "user_speaking") this.hooks.setOrbLevel(this.runtime.inputLevel() * 0.8);
    this.raf = requestAnimationFrame(() => this.frame());
  }

  private syncButtons(): void {
    const phase = this.runtime.phase;
    const on = this.runtime.active;
    document.body.classList.toggle("voice-on", on || phase === "loading");
    this.micBtn.classList.toggle("on", on);
    this.micBtn.classList.toggle("loading", phase === "loading");
    this.micBtn.setAttribute("aria-pressed", String(on));
    this.micBtn.title = on ? "Turn voice off" : "Talk to Leo";
    this.muteBtn.classList.toggle("muted", this.runtime.isMuted);
    this.muteBtn.title = this.runtime.isMuted ? "Unmute microphone" : "Mute microphone";
    if (!on) this.partial = "";
    if (on && this.runtime.isMuted) this.line.textContent = "Microphone muted";
  }
}

function readVoice(): string | undefined {
  try {
    return localStorage.getItem(STORAGE_VOICE) || undefined;
  } catch {
    return undefined;
  }
}

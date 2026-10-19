/** Shared types for the voice runtime. Nothing here knows about Leo's agent. */

export type VoicePhase =
  | "idle" // voice mode is off
  | "loading" // models and microphone are being prepared
  | "listening" // waiting for the user
  | "user_speaking" // the user is talking
  | "thinking" // the turn is with Leo, nothing is being said yet
  | "speaking" // Leo's voice is playing
  | "interrupted" // the user cut Leo off; transitional
  | "error";

/** One piece of synthesized speech. */
export interface PcmChunk {
  samples: Float32Array;
  sampleRate: number;
}

export interface TTSOptions {
  voice?: string;
  speed?: number;
}

/** Text to speech. The rest of the app never sees which engine is behind it. */
export interface TTS {
  load(onProgress?: (fraction: number) => void): Promise<void>;
  /** Yields audio as it is produced; stops yielding after `cancel()`. */
  synthesize(text: string, options?: TTSOptions): AsyncIterable<PcmChunk>;
  /** Drops everything queued or running. */
  cancel(): void;
  dispose(): void;
}

/** Speech to text for 16 kHz mono audio. */
export interface STT {
  load(onProgress?: (fraction: number) => void): Promise<void>;
  /** `final` requests take priority over partial ones; a stale partial resolves to null. */
  transcribe(audio: Float32Array, options: { final: boolean }): Promise<string | null>;
  dispose(): void;
}

/** What the voice-activity detector reports. Frames are 16 kHz mono. */
export interface VadEvents {
  onFrame(probability: number, frame: Float32Array): void;
  onSpeechStart(): void;
  /** `audio` is the whole segment (with a little lead-in) at 16 kHz. */
  onSpeechEnd(audio: Float32Array): void;
  /** Speech started but turned out to be too short to count. */
  onMisfire(): void;
  onError(error: Error): void;
}

export interface Vad {
  start(events: VadEvents): Promise<void>;
  /** Stops listening and releases the microphone. */
  stop(): Promise<void>;
  /** Temporarily stops listening (mute) and resumes again. */
  pause(): Promise<void>;
  resume(): Promise<void>;
}

/** The slice of the speaker path the runtime needs. */
export interface Player {
  ensure(): Promise<void>;
  readonly epoch: number;
  schedule(samples: Float32Array, sampleRate: number, epoch: number): boolean;
  flush(epoch: number): void;
  /** Calls back once everything scheduled for `epoch` has been played. */
  whenDrained(epoch: number, callback: () => void): void;
  onAudioStart: (() => void) | null;
  onAudioEnd: (() => void) | null;
  level(): number;
  dispose(): void;
}

export type VoiceEventMap = {
  phase: { phase: VoicePhase; previous: VoicePhase };
  speechStart: undefined;
  partialTranscript: { text: string };
  finalTranscript: { text: string };
  leoText: { text: string };
  bargeIn: undefined;
  audioStart: undefined;
  audioEnd: undefined;
  progress: { fraction: number; what: string };
  error: { message: string; source: "stt" | "tts" | "mic" | "player" | "leo" | "runtime"; fatal: boolean };
};

export type VoiceEventName = keyof VoiceEventMap;

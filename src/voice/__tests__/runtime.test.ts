import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { VoiceRuntime } from "../runtime";
import { BargeInDetector } from "../bargeIn";
import type { PcmChunk, Player, STT, TTS, Vad, VadEvents, VoicePhase } from "../types";

// ---------- fakes ----------

class FakeVad implements Vad {
  events!: VadEvents;
  async start(events: VadEvents) {
    this.events = events;
  }
  async stop() {}
  async pause() {}
  async resume() {}
  /** Frames of the given speech probability (32 ms each). */
  frames(probability: number, count: number) {
    for (let i = 0; i < count; i++) this.events.onFrame(probability, new Float32Array(512).fill(probability > 0.5 ? 0.1 : 0));
  }
  startSpeech() {
    this.events.onSpeechStart();
  }
  endSpeech() {
    this.events.onSpeechEnd(new Float32Array(16000));
  }
}

class FakeSTT implements STT {
  next: string[] = [];
  finals: Float32Array[] = [];
  async load() {}
  async transcribe(audio: Float32Array, options: { final: boolean }) {
    if (!options.final) return null;
    this.finals.push(audio);
    return this.next.shift() ?? "";
  }
  dispose() {}
}

class FakeTTS implements TTS {
  spoken: string[] = [];
  cancelled = 0;
  async load() {}
  async *synthesize(text: string): AsyncIterable<PcmChunk> {
    this.spoken.push(text);
    yield { samples: new Float32Array(2400).fill(0.1), sampleRate: 24000 };
  }
  cancel() {
    this.cancelled++;
  }
  dispose() {}
}

class FakePlayer implements Player {
  epoch = 0;
  scheduled: { epoch: number; length: number }[] = [];
  flushed: number[] = [];
  drain: (() => void) | null = null;
  onAudioStart: (() => void) | null = null;
  onAudioEnd: (() => void) | null = null;
  loud = 0;
  async ensure() {}
  schedule(samples: Float32Array, _rate: number, epoch: number) {
    if (epoch !== this.epoch || samples.length === 0) return false;
    this.scheduled.push({ epoch, length: samples.length });
    return true;
  }
  flush(epoch: number) {
    this.epoch = epoch;
    this.flushed.push(epoch);
    this.drain = null;
  }
  whenDrained(epoch: number, cb: () => void) {
    if (epoch !== this.epoch) return;
    // Like the real player: nothing scheduled means nothing to wait for.
    if (!this.scheduled.some((s) => s.epoch === epoch)) cb();
    else this.drain = cb;
  }
  level() {
    return this.loud;
  }
  dispose() {}
  /** Playback of everything scheduled finishes. */
  finishPlayback() {
    const cb = this.drain;
    this.drain = null;
    cb?.();
  }
}

// ---------- harness ----------

const tick = async (ms = 0) => {
  await vi.advanceTimersByTimeAsync(ms);
};

function harness() {
  const vad = new FakeVad();
  const stt = new FakeSTT();
  const tts = new FakeTTS();
  const player = new FakePlayer();
  const phases: VoicePhase[] = [];
  const submitted: string[] = [];
  let cancelCalls = 0;

  // Stand-in for Leo: sendMessage() does what main.ts does (begin a response).
  const runtime: VoiceRuntime = new VoiceRuntime(
    {
      submit: (text) => {
        submitted.push(text);
        runtime.beginResponse();
      },
      cancelResponse: () => {
        cancelCalls++;
      },
    },
    { vad, stt, tts, player, bargeIn: new BargeInDetector() },
  );
  runtime.on("phase", ({ phase }) => phases.push(phase));
  return { vad, stt, tts, player, runtime, phases, submitted, cancels: () => cancelCalls };
}

/** The user says something while Leo is idle. */
async function userSays(h: ReturnType<typeof harness>, text: string) {
  h.stt.next.push(text);
  h.vad.startSpeech();
  h.vad.frames(0.95, 10);
  h.vad.endSpeech();
  await tick(0);
  await tick(2000); // any hold time
}

describe("VoiceRuntime", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("listens, submits the final transcript to Leo's text path, then speaks the reply", async () => {
    const h = harness();
    await h.runtime.start();
    expect(h.runtime.phase).toBe("listening");

    await userSays(h, "Explain how RAG works.");
    expect(h.submitted).toEqual(["Explain how RAG works."]);
    expect(h.runtime.phase).toBe("thinking");

    // Leo streams: the first sentence is spoken before the reply is complete.
    h.runtime.feedLeoText("Retrieval augmented generation is a way to ");
    await tick();
    expect(h.tts.spoken).toEqual([]);
    h.runtime.feedLeoText("ground answers in documents. It works in two steps");
    await tick();
    expect(h.tts.spoken).toEqual(["Retrieval augmented generation is a way to ground answers in documents."]);
    expect(h.runtime.phase).toBe("speaking");

    h.runtime.endLeoText("Retrieval augmented generation is a way to ground answers in documents. It works in two steps");
    await tick();
    expect(h.tts.spoken).toHaveLength(2);
    h.player.finishPlayback();
    expect(h.runtime.phase).toBe("listening");
  });

  it("speaks a lead-in before a tool call without waiting for more text", async () => {
    const h = harness();
    await h.runtime.start();
    await userSays(h, "Tell me about Elon Musk please.");
    h.runtime.feedLeoText("Sure, let me look up Elon Musk online.");
    await tick();
    expect(h.tts.spoken).toEqual([]); // the stop might still be an abbreviation
    h.runtime.flushLeoText(); // a tool call is about to start
    await tick();
    expect(h.tts.spoken).toEqual(["Sure, let me look up Elon Musk online."]);
  });

  it("still speaks a reply that arrives whole after narration lines (plan route)", async () => {
    const h = harness();
    await h.runtime.start();
    await userSays(h, "Email Priya the report please.");
    h.runtime.narrate("Sure, let me find the address first.");
    await tick();
    h.runtime.narrate("Got it, now I will write the email.");
    await tick();
    h.runtime.endLeoText("Done, the email is on its way to Priya.");
    await tick();
    expect(h.tts.spoken).toEqual([
      "Sure, let me find the address first.",
      "Got it, now I will write the email.",
      "Done, the email is on its way to Priya.",
    ]);
  });

  it("speaks the whole reply when nothing streamed (plan route)", async () => {
    const h = harness();
    await h.runtime.start();
    await userSays(h, "What is the weather today?");
    h.runtime.endLeoText("It is sunny and warm in Bangalore today.");
    await tick();
    expect(h.tts.spoken).toEqual(["It is sunny and warm in Bangalore today."]);
  });

  it("does not speak markdown symbols", async () => {
    const h = harness();
    await h.runtime.start();
    await userSays(h, "List my repos please.");
    h.runtime.endLeoText("### Results\n\n1. Git Show\n2. Leo");
    await tick();
    expect(h.tts.spoken.join(" ")).not.toMatch(/[#*`]/);
  });

  describe("barge-in", () => {
    async function leoIsSpeaking() {
      const h = harness();
      await h.runtime.start();
      await userSays(h, "Explain how RAG works.");
      h.runtime.feedLeoText("Retrieval augmented generation is a way to ground answers in documents. And then");
      await tick();
      expect(h.runtime.phase).toBe("speaking");
      return h;
    }

    it("stops audio, cancels Leo and drops old speech when the user cuts in", async () => {
      const h = await leoIsSpeaking();
      const epochBefore = h.player.epoch;
      const flushesBefore = h.player.flushed.length;

      h.vad.startSpeech();
      h.vad.frames(0.95, 3);
      expect(h.runtime.phase).toBe("speaking"); // not yet: needs sustained speech
      h.vad.frames(0.95, 8);

      expect(h.player.epoch).toBeGreaterThan(epochBefore); // old audio invalidated
      expect(h.player.flushed.length).toBeGreaterThan(flushesBefore);
      expect(h.cancels()).toBe(1);
      expect(h.tts.cancelled).toBeGreaterThan(0);
      expect(h.runtime.phase).toBe("user_speaking");

      // Old generation still delivering text and old TTS chunks is ignored.
      const spokenBefore = h.tts.spoken.length;
      h.runtime.feedLeoText("And that is the second sentence of the old reply. ");
      await tick();
      expect(h.tts.spoken.length).toBe(spokenBefore);
      expect(h.player.schedule(new Float32Array(10), 24000, epochBefore)).toBe(false);
    });

    it("captures the interrupting sentence and sends it to the same Leo", async () => {
      const h = await leoIsSpeaking();
      h.stt.next.push("Wait, explain embeddings first.");
      h.vad.startSpeech();
      h.vad.frames(0.95, 12);
      h.vad.endSpeech();
      await tick(2000);

      expect(h.submitted).toEqual(["Explain how RAG works.", "Wait, explain embeddings first."]);
      expect(h.runtime.phase).toBe("thinking");

      // The new reply speaks normally.
      h.runtime.feedLeoText("Embeddings turn text into vectors that capture meaning. They are the base");
      await tick();
      expect(h.tts.spoken[h.tts.spoken.length - 1]).toBe("Embeddings turn text into vectors that capture meaning.");
    });

    it("ignores sound that is not sustained speech (echo, a click)", async () => {
      const h = await leoIsSpeaking();
      h.vad.startSpeech();
      h.vad.frames(0.95, 3);
      h.vad.frames(0.1, 5);
      h.vad.endSpeech();
      await tick(2000);
      expect(h.cancels()).toBe(0);
      expect(h.runtime.phase).toBe("speaking");
      expect(h.submitted).toHaveLength(1);
    });

    it("is harder to trigger while Leo is loud (leftover echo)", async () => {
      const h = await leoIsSpeaking();
      h.player.loud = 0.6;
      h.vad.startSpeech();
      h.vad.frames(0.75, 20);
      expect(h.cancels()).toBe(0);
      expect(h.runtime.phase).toBe("speaking");
    });

    it("returns to listening afterwards when the interruption turns out to be nothing", async () => {
      const h = await leoIsSpeaking();
      h.stt.next.push("um");
      h.vad.startSpeech();
      h.vad.frames(0.95, 12);
      h.vad.endSpeech();
      await tick(2000);
      expect(h.submitted).toHaveLength(1); // "um" is not sent
      expect(h.runtime.phase).toBe("listening");
    });
  });

  describe("turn detection", () => {
    it("does not submit empty speech or noise", async () => {
      const h = harness();
      await h.runtime.start();
      await userSays(h, "");
      await userSays(h, "[BLANK_AUDIO]");
      expect(h.submitted).toEqual([]);
      expect(h.runtime.phase).toBe("listening");
    });

    it("waits through a pause when the sentence sounds unfinished, then joins both parts", async () => {
      const h = harness();
      await h.runtime.start();

      h.stt.next.push("Explain how RAG works and");
      h.vad.startSpeech();
      h.vad.frames(0.95, 10);
      h.vad.endSpeech();
      await tick(400);
      expect(h.submitted).toEqual([]); // still holding

      h.stt.next.push("why it helps.");
      h.vad.startSpeech();
      h.vad.frames(0.95, 10);
      h.vad.endSpeech();
      await tick(2000);
      expect(h.submitted).toEqual(["Explain how RAG works and why it helps."]);
    });

    it("emits a final transcript only once per turn", async () => {
      const h = harness();
      const finals: string[] = [];
      h.runtime.on("finalTranscript", ({ text }) => finals.push(text));
      await h.runtime.start();
      await userSays(h, "Hello there.");
      expect(finals).toEqual(["Hello there."]);
    });
  });

  describe("failures", () => {
    it("keeps listening when TTS fails, and reports it once", async () => {
      const h = harness();
      h.tts.synthesize = async function* () {
        throw new Error("boom");
      };
      const errors: string[] = [];
      h.runtime.on("error", (e) => errors.push(e.source));
      await h.runtime.start();
      await userSays(h, "Say something please.");
      h.runtime.endLeoText("First sentence goes here today. Second sentence goes here too.");
      await tick();
      expect(errors).toEqual(["tts"]);
      expect(h.runtime.phase).toBe("listening");
    });

    it("goes back to listening when Leo fails", async () => {
      const h = harness();
      await h.runtime.start();
      await userSays(h, "Do the thing please.");
      h.runtime.failResponse();
      expect(h.runtime.phase).toBe("listening");
    });

    it("goes back to listening if submitting to Leo throws", async () => {
      const vad = new FakeVad();
      const stt = new FakeSTT();
      const runtime = new VoiceRuntime(
        {
          submit: () => {
            throw new Error("no key");
          },
          cancelResponse: () => {},
        },
        { vad, stt, tts: new FakeTTS(), player: new FakePlayer() },
      );
      await runtime.start();
      stt.next.push("Hello there friend.");
      vad.startSpeech();
      vad.frames(0.95, 10);
      vad.endSpeech();
      await tick(2000);
      expect(runtime.phase).toBe("listening");
    });

    it("a failed start leaves voice off and does not throw", async () => {
      const vad = new FakeVad();
      vad.start = async () => {
        throw new Error("no microphone");
      };
      const runtime = new VoiceRuntime(
        { submit: () => {}, cancelResponse: () => {} },
        { vad, stt: new FakeSTT(), tts: new FakeTTS(), player: new FakePlayer() },
      );
      const errors: string[] = [];
      runtime.on("error", (e) => errors.push(e.message));
      await runtime.start();
      expect(runtime.phase).toBe("idle");
      expect(errors).toEqual(["no microphone"]);
    });
  });
});

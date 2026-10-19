import { beforeEach, describe, expect, it, vi } from "vitest";
import { AudioPlayer, type AudioPlatform } from "../audioPlayer";

class FakeSource {
  buffer: { duration: number } | null = null;
  startedAt: number | null = null;
  stopped = false;
  onended: (() => void) | null = null;
  connect() {}
  disconnect() {}
  start(at: number) {
    this.startedAt = at;
  }
  stop() {
    this.stopped = true;
  }
  end() {
    this.onended?.();
  }
}

class FakeContext {
  currentTime = 0;
  state = "running";
  sources: FakeSource[] = [];
  destination = {};
  createGain() {
    return { connect() {} };
  }
  createAnalyser() {
    return { fftSize: 0, connect() {}, getFloatTimeDomainData() {}, frequencyBinCount: 4, getByteFrequencyData() {} };
  }
  createMediaStreamDestination() {
    return { stream: {} };
  }
  createBuffer(_channels: number, length: number, rate: number) {
    return { duration: length / rate, copyToChannel() {} };
  }
  createBufferSource() {
    const s = new FakeSource();
    this.sources.push(s);
    return s;
  }
  async resume() {}
  async close() {}
}

function setup() {
  const ctx = new FakeContext();
  const element = { srcObject: null as unknown, autoplay: false, play: async () => {}, pause() {} };
  const platform: AudioPlatform = {
    createContext: () => ctx as unknown as AudioContext,
    createElement: () => element as unknown as HTMLAudioElement,
  };
  return { ctx, element, player: new AudioPlayer(platform) };
}

const pcm = (seconds: number, rate = 1000) => new Float32Array(seconds * rate).fill(0.1);

describe("AudioPlayer", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  it("routes output through a media element so echo cancellation can see it", async () => {
    const { player, element } = setup();
    await player.ensure();
    expect(element.srcObject).not.toBeNull();
  });

  it("schedules chunks back to back with no gap", async () => {
    const { player, ctx } = setup();
    await player.ensure();
    player.schedule(pcm(0.9), 1000, 0);
    player.schedule(pcm(0.75), 1000, 0);
    player.schedule(pcm(1.05), 1000, 0);
    const [a, b, c] = ctx.sources;
    expect(b.startedAt).toBeCloseTo(a.startedAt! + 0.9, 5);
    expect(c.startedAt).toBeCloseTo(b.startedAt! + 0.75, 5);
  });

  it("restarts just ahead of now if playback ran dry", async () => {
    const { player, ctx } = setup();
    await player.ensure();
    player.schedule(pcm(0.5), 1000, 0);
    ctx.currentTime = 10;
    player.schedule(pcm(0.5), 1000, 0);
    expect(ctx.sources[1].startedAt!).toBeGreaterThanOrEqual(10);
  });

  it("ignores empty chunks", async () => {
    const { player, ctx } = setup();
    await player.ensure();
    expect(player.schedule(new Float32Array(0), 1000, 0)).toBe(false);
    expect(ctx.sources).toHaveLength(0);
  });

  it("rejects audio from an old epoch and accepts the new one", async () => {
    const { player, ctx } = setup();
    await player.ensure();
    player.flush(1);
    expect(player.schedule(pcm(0.5), 1000, 0)).toBe(false);
    expect(player.schedule(pcm(0.5), 1000, 1)).toBe(true);
    expect(ctx.sources).toHaveLength(1);
  });

  it("flush stops playing audio, clears callbacks and resets the clock", async () => {
    const { player, ctx } = setup();
    await player.ensure();
    const started = vi.fn();
    const ended = vi.fn();
    player.onAudioStart = started;
    player.onAudioEnd = ended;
    const drained = vi.fn();

    player.schedule(pcm(1), 1000, 0);
    player.schedule(pcm(1), 1000, 0);
    vi.advanceTimersByTime(100);
    expect(started).toHaveBeenCalledTimes(1);
    player.whenDrained(0, drained);

    player.flush(1);
    expect(ctx.sources.every((s) => s.stopped)).toBe(true);
    expect(ended).toHaveBeenCalledTimes(1);

    // Late "ended" events from the stopped sources and the old callback must do nothing.
    ctx.sources.forEach((s) => s.end());
    expect(drained).not.toHaveBeenCalled();

    // The clock starts over: the next chunk starts right away, not after the old audio.
    ctx.currentTime = 0.2;
    player.schedule(pcm(0.5), 1000, 1);
    expect(ctx.sources[2].startedAt!).toBeLessThan(0.3);
  });

  it("flush before audio has started cancels the pending start notification", async () => {
    const { player } = setup();
    await player.ensure();
    const started = vi.fn();
    player.onAudioStart = started;
    player.schedule(pcm(1), 1000, 0);
    player.flush(1);
    vi.advanceTimersByTime(500);
    expect(started).not.toHaveBeenCalled();
  });

  it("whenDrained fires after the last chunk ends", async () => {
    const { player, ctx } = setup();
    await player.ensure();
    const drained = vi.fn();
    player.schedule(pcm(0.5), 1000, 0);
    player.schedule(pcm(0.5), 1000, 0);
    player.whenDrained(0, drained);
    ctx.sources[0].end();
    expect(drained).not.toHaveBeenCalled();
    ctx.sources[1].end();
    expect(drained).toHaveBeenCalledTimes(1);
  });

  it("whenDrained fires at once when nothing is playing", async () => {
    const { player } = setup();
    await player.ensure();
    const drained = vi.fn();
    player.whenDrained(0, drained);
    expect(drained).toHaveBeenCalledTimes(1);
  });
});

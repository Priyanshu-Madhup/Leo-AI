import { describe, expect, it } from "vitest";
import { HeuristicTurnManager } from "../turnManager";
import { VoiceState } from "../stateMachine";
import { BargeInDetector } from "../bargeIn";
import type { VoicePhase } from "../types";

describe("HeuristicTurnManager", () => {
  const t = new HeuristicTurnManager();
  it("does not submit empty or noise transcripts", () => {
    for (const x of ["", "   ", "...", "[BLANK_AUDIO]", "(music)", "♪♪", "um", "Hmm", "you"]) {
      expect(t.isSubmittable(x), x).toBe(false);
    }
    expect(t.isSubmittable("What is RAG?")).toBe(true);
  });
  it("submits quickly after a finished sentence", () => {
    expect(t.holdMs("Explain how RAG works.")).toBe(0);
    expect(t.holdMs("What is the weather in Bangalore?")).toBe(0);
  });
  it("waits longer when the thought sounds unfinished", () => {
    expect(t.holdMs("Explain how RAG works and")).toBeGreaterThan(500);
    expect(t.holdMs("Well, I was thinking,")).toBeGreaterThan(500);
    expect(t.holdMs("So uh")).toBeGreaterThan(500);
    expect(t.holdMs("Explain how RAG works")).toBeGreaterThan(0);
    expect(t.holdMs("Explain how RAG works")).toBeLessThan(500);
  });
});

describe("VoiceState", () => {
  const walk = (...steps: VoicePhase[]) => {
    const s = new VoiceState();
    return steps.map((p) => s.go(p));
  };
  it("follows the normal conversation loop", () => {
    expect(walk("loading", "listening", "user_speaking", "thinking", "speaking", "listening")).toEqual(
      Array(6).fill(true),
    );
  });
  it("handles an interruption", () => {
    expect(walk("loading", "listening", "thinking", "speaking", "interrupted", "user_speaking", "thinking")).toEqual(
      Array(7).fill(true),
    );
  });
  it("refuses impossible moves", () => {
    expect(walk("listening")).toEqual([false]);
    expect(walk("loading", "listening", "interrupted")).toEqual([true, true, false]);
  });
});

describe("BargeInDetector", () => {
  it("ignores a short click or cough", () => {
    const d = new BargeInDetector();
    expect([0.9, 0.9, 0.1, 0.1, 0.1].map((p) => d.push(p, 0))).not.toContain(true);
  });
  it("confirms sustained confident speech", () => {
    const d = new BargeInDetector();
    const hits = Array.from({ length: 12 }, () => d.push(0.95, 0));
    expect(hits.indexOf(true)).toBeGreaterThan(5);
  });
  it("is harder to trigger while Leo is loud (leftover echo)", () => {
    const quiet = new BargeInDetector();
    const loud = new BargeInDetector();
    const p = 0.75;
    expect(Array.from({ length: 20 }, () => quiet.push(p, 0)).includes(true)).toBe(true);
    expect(Array.from({ length: 20 }, () => loud.push(p, 0.6)).includes(true)).toBe(false);
  });
  it("tolerates a brief dip in a real interruption", () => {
    const d = new BargeInDetector();
    const seq = [0.9, 0.9, 0.9, 0.3, 0.9, 0.9, 0.9, 0.9, 0.9, 0.9];
    expect(seq.map((p) => d.push(p, 0))).toContain(true);
  });
});

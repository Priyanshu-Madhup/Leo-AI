import { describe, expect, it } from "vitest";
import { SentenceChunker } from "../sentenceChunker";

function run(tokens: string[], chunker = new SentenceChunker()) {
  const out: string[] = [];
  for (const t of tokens) out.push(...chunker.push(t));
  out.push(...chunker.flush());
  return out;
}

describe("SentenceChunker", () => {
  it("waits for a full sentence instead of speaking tokens", () => {
    const c = new SentenceChunker();
    expect(c.push("Sure")).toEqual([]);
    expect(c.push(", I can help")).toEqual([]);
    expect(c.push(" you with that.")).toEqual([]); // the stop might be an abbreviation: wait to see what follows
    expect(c.push(" The first")).toEqual(["Sure, I can help you with that."]);
    expect(c.flush()).toEqual(["The first"]);
  });

  it("lets the first chunk end at a comma so speech starts sooner", () => {
    const c = new SentenceChunker();
    expect(c.push("Retrieval augmented generation lets a model look things up, ")).toEqual([
      "Retrieval augmented generation lets a model look things up,",
    ]);
    // Only the first chunk: later ones wait for a full sentence.
    expect(c.push("then it answers from what it found, and that keeps it honest. ")).toEqual([
      "then it answers from what it found, and that keeps it honest.",
    ]);
  });

  it("does not cut a short opening at its first comma", () => {
    const c = new SentenceChunker();
    expect(c.push("Sure, I can help you with that. ")).toEqual(["Sure, I can help you with that."]);
  });

  it("does not speak a one-word sentence alone", () => {
    expect(run(["Sure. ", "I can help you with that today."])).toEqual(["Sure. I can help you with that today."]);
  });

  it("splits on ? and !", () => {
    expect(run(["Is that what you meant by it? ", "Great, then we are done here!"])).toEqual([
      "Is that what you meant by it?",
      "Great, then we are done here!",
    ]);
  });

  it("does not split on abbreviations, decimals, initials or list numbers", () => {
    expect(run(["Dr. Smith measured 3.5 litres of water today. Then he left."])).toEqual([
      "Dr. Smith measured 3.5 litres of water today.",
      "Then he left.",
    ]);
    expect(run(["1. Install the package and run it\n2. Open the app and sign in\n"])).toEqual([
      "1. Install the package and run it",
      "2. Open the app and sign in",
    ]);
  });

  it("splits a very long run-on at a comma", () => {
    const long = "word ".repeat(30) + "and then, " + "more ".repeat(40);
    const out = run([long], new SentenceChunker({ maxChars: 200 }));
    expect(out.length).toBeGreaterThan(1);
    expect(out.every((s) => s.length <= 205)).toBe(true);
  });

  it("flush speaks what is left, even if short", () => {
    expect(run(["Done"])).toEqual(["Done"]);
  });

  it("skips fenced code, even when the fence marker is split", () => {
    expect(
      run(["Here is the fix for that bug. ``", "`js\nconst a = 1;\n``", "` And that is all there is to it."]),
    ).toEqual(["Here is the fix for that bug.", "And that is all there is to it."]);
  });

  it("reset forgets the unspoken text", () => {
    const c = new SentenceChunker();
    c.push("Let me look that up for");
    c.reset();
    expect(c.flush()).toEqual([]);
  });
});

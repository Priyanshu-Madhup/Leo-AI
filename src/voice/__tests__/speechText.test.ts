import { describe, expect, it } from "vitest";
import { speechTextNormalizer as n } from "../speechText";

describe("speechTextNormalizer", () => {
  it("reads a heading and a numbered list the way a person would", () => {
    expect(n("### Results\n\n1. Git Show\n2. Leo\n3. WorkFlowOS")).toBe(
      "Results. First, Git Show Second, Leo Third, WorkFlowOS",
    );
  });
  it("drops formatting characters", () => {
    expect(n("This is **bold**, *italic* and `code`.")).toBe("This is bold, italic and code.");
    expect(n("Not a ## heading and no # signs *")).not.toMatch(/[#*`]/);
  });
  it("keeps link text and replaces bare addresses", () => {
    expect(n("See [the docs](https://example.com/x) or https://example.com/y now")).toBe("See the docs or a link now");
  });
  it("turns table rows into phrases and skips dividers", () => {
    expect(n("| Name | Age |\n|---|---|\n| Ann | 30 |")).toBe("Name, Age Ann, 30");
  });
  it("removes emoji and rules", () => {
    expect(n("Done \u{1F389}\n---\nNext")).toBe("Done Next");
  });
  it("returns empty for markup with nothing to say", () => {
    expect(n("---\n\n```\n")).toBe("");
  });
});

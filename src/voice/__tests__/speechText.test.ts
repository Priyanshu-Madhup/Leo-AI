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
  it("never reads emoji or symbols out by name", () => {
    const reply =
      "All set! \u{1FA92} Your event is on your calendar:\n\u{1F4C5} Today\n⏰ 6:00 PM – 7:00 PM ✂️ Have a great trim! \u{1F44B}\u{1F3FD} ✅ ⭐ \u{1F1EE}\u{1F1F3}";
    expect(n(reply)).toBe("All set! Your event is on your calendar: Today 6:00 PM – 7:00 PM Have a great trim!");
  });
  it("drops icon names and shortcodes", () => {
    expect(n("Rain today :cloud_with_rain: meteocons:rain-fill and lucide:droplets 72%")).toBe("Rain today and 72%");
    expect(n('{"title":"Bengaluru","icon":"meteocons:rain-fill","value":"25"}')).toBe("");
  });
  it("says degrees", () => {
    expect(n("It is 25°C, or 77°F.")).toBe("It is 25 degrees Celsius, or 77 degrees Fahrenheit.");
  });
  it("returns empty for markup with nothing to say", () => {
    expect(n("---\n\n```\n")).toBe("");
  });
});

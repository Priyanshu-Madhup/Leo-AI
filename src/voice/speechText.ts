/**
 * Turns a piece of Leo's Markdown reply into text that sounds right when read
 * aloud. The chat keeps showing the original; only the voice gets this.
 */

const ORDINALS = ["First", "Second", "Third", "Fourth", "Fifth", "Sixth", "Seventh", "Eighth", "Ninth", "Tenth"];

const EMOJI = /[\u{1F000}-\u{1FAFF}\u{2600}-\u{27BF}\u{FE0F}\u{200D}]/gu;

export function speechTextNormalizer(input: string): string {
  const lines = input.replace(/\r/g, "").split("\n");
  const spoken: string[] = [];

  for (let raw of lines) {
    let line = raw.trim();
    if (!line) continue;
    if (/^([-*_]\s*){3,}$/.test(line)) continue; // horizontal rule
    if (/^\|?[\s:|-]+\|?$/.test(line) && line.includes("-")) continue; // table divider

    // Headings read as a short sentence of their own.
    const heading = /^#{1,6}\s+(.*)$/.exec(line);
    let isHeading = false;
    if (heading) {
      line = heading[1];
      isHeading = true;
    }

    line = line.replace(/^>\s?/, ""); // quote marker

    // List markers: "1. x" becomes "First, x"; bullets are dropped.
    const numbered = /^(\d{1,3})[.)]\s+(.*)$/.exec(line);
    if (numbered) {
      const n = Number(numbered[1]);
      line = `${n >= 1 && n <= ORDINALS.length ? ORDINALS[n - 1] : `Number ${n}`}, ${numbered[2]}`;
    } else {
      line = line.replace(/^[-*+•]\s+/, "").replace(/^\[[ xX]\]\s+/, "");
    }

    // Table rows: cells become comma-separated phrases.
    if (line.startsWith("|")) line = line.replace(/^\||\|$/g, "").split("|").map((c) => c.trim()).filter(Boolean).join(", ");

    line = line
      .replace(/!\[[^\]]*\]\([^)]*\)/g, "") // images
      .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1") // links keep their text
      .replace(/<https?:\/\/[^>]+>/g, "") // <autolinks>
      .replace(/https?:\/\/\S+/g, "a link") // bare addresses
      .replace(/<[^>]+>/g, "") // html tags
      .replace(/`([^`]*)`/g, "$1") // inline code keeps its text
      .replace(/(\*\*|__)(.*?)\1/g, "$2")
      .replace(/(\*|_)(.*?)\1/g, "$2")
      .replace(/~~(.*?)~~/g, "$1")
      .replace(/[*_`#]/g, "")
      .replace(EMOJI, "")
      .replace(/\s*→\s*/g, ", ")
      .replace(/\s*&\s*/g, " and ")
      .replace(/\s+/g, " ")
      .trim();
    if (!line) continue;

    if ((isHeading || numbered) && !/[.!?:,;]$/.test(line)) line += isHeading ? "." : "";
    spoken.push(line);
  }

  return spoken.join(" ").replace(/\s+([.,!?;:])/g, "$1").trim();
}

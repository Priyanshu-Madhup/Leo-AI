/**
 * Turns a piece of Leo's Markdown reply into text that sounds right when read
 * aloud. The chat keeps showing the original; only the voice gets this.
 */

const ORDINALS = ["First", "Second", "Third", "Fourth", "Fifth", "Sixth", "Seventh", "Eighth", "Ninth", "Tenth"];

// Every pictograph, not a fixed list of ranges: the voice would read each one out by name.
const EMOJI = /\p{Extended_Pictographic}|\p{Emoji_Presentation}|[\u{FE0E}\u{FE0F}\u{200D}\u{20E3}\u{1F1E6}-\u{1F1FF}\u{1F3FB}-\u{1F3FF}\u{E0020}-\u{E007F}]/gu;

// Icon names from the widget cards (meteocons:rain-fill, lucide:calendar) and
// :emoji_shortcodes: are for the screen only.
const ICON_NAME = /\b(?:meteocons|lucide|logos|mdi|ph|tabler|material-symbols|fluent|carbon|solar|heroicons|ri|bi|fa6-solid|fa6-regular|fa6-brands|simple-icons|iconoir|streamline)[:][\w-]+/gi;
const SHORTCODE = /:[a-z][a-z0-9_+-]*:/gi;

export function speechTextNormalizer(input: string): string {
  const lines = input.replace(/\r/g, "").split("\n");
  const spoken: string[] = [];

  for (let raw of lines) {
    let line = raw.trim();
    if (!line) continue;
    if (/^[{[].*[}\]]$/.test(line) && /"\s*:/.test(line)) continue; // a stray JSON line (a card's data)
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
      .replace(ICON_NAME, "")
      .replace(SHORTCODE, " ")
      .replace(/\s*°\s*C\b/g, " degrees Celsius")
      .replace(/\s*°\s*F\b/g, " degrees Fahrenheit")
      .replace(/°/g, " degrees")
      .replace(EMOJI, "")
      .replace(/[\p{So}\u{FE0F}]/gu, "") // any other symbol that would be read as its name
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

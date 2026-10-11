// What's new, shown inside the app once after an update.
//
// How to add a release: put a new entry FIRST with the version and a short
// plain-language line for each new feature or visible change. Do not list bug
// fixes; the panel always ends with one general "bug fixes" line instead.
export interface Release {
  version: string;
  features: string[];
}

export const RELEASES: Release[] = [
  {
    version: "3.2.0",
    features: [
      "Minimising to the orb is smoother",
      "The orb now moves gently when it is idle",
      "This What's new note after each update",
    ],
  },
  {
    version: "3.1.0",
    features: ["Chat history: reopen any earlier chat from the clock button, and Leo remembers where you left off"],
  },
  {
    version: "3.0.0",
    features: [
      "Leo can look at your screen: ask it to read or explain what you have open",
      "Every assistant now shares the date, time and your approximate location",
      "Every assistant can search the web",
      "Google Docs asks for approval once per document, not for every edit",
    ],
  },
];

export const BUG_FIXES_LINE = "Bug fixes and improvements";

function parts(version: string): number[] {
  return version.split(".").map((n) => parseInt(n, 10) || 0);
}

/** True when version `a` is newer than `b`. */
export function isNewer(a: string, b: string): boolean {
  const [x, y] = [parts(a), parts(b)];
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    const d = (x[i] ?? 0) - (y[i] ?? 0);
    if (d !== 0) return d > 0;
  }
  return false;
}

/** The releases after `seen` up to and including `current`, newest first. */
export function releasesSince(seen: string, current: string): Release[] {
  return RELEASES.filter((r) => isNewer(r.version, seen) && !isNewer(r.version, current));
}

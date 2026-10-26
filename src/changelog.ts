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
    version: "3.8.4",
    features: [
      "Search your chats: tap the magnifier in the Chats list and type a word; it finds chats by name or by anything said in them",
      "The history button now sits in the top left, where the status text used to be",
      "With many prompts in a chat, the markers on the right scroll smoothly, with the top and bottom ones dimmed to show there is more",
    ],
  },
  {
    version: "3.8.3",
    features: [
      "A down arrow appears above the message bar when you are not at the end of the chat; tap it to jump to the latest message",
      "Long chats get a column of small lines on the right edge, one per prompt you sent. Hover one to see the prompt, click it to jump there",
      "The message bar now grows smoothly, and long messages use the full width with the buttons moving to a row underneath",
      "The orb no longer sends out ripple rings while it works",
    ],
  },
  {
    version: "3.8.2",
    features: [
      "A new Permissions setting lets Leo add and change your calendar events without asking each time (you can also just tell Leo). It still asks before deleting an event or inviting other people",
      "Leo's voice no longer reads out emoji, icon names or symbols",
    ],
  },
  {
    version: "3.8.1",
    features: ["With voice on, Leo now says out loud what it is doing as it works, such as checking the weather or searching the web"],
  },
  {
    version: "3.8.0",
    features: [
      "Talk to Leo: tap the microphone, speak, and Leo answers out loud. Speak over Leo to interrupt it at any time",
      "Attach images to a message with the paperclip, by pasting, or by dropping them onto the chat, in almost any format",
      "Leo now says what it is about to do while it works, such as \"Sure, let me look that up\", before each step",
    ],
  },
  {
    version: "3.7.0",
    features: [
      "Weather now appears as a card with an icon, and the week's forecast opens from a button",
      "Leo can show answers as rounded cards with icons, such as scores, prices, schedules and comparisons",
      "Interactive graphs: hover or touch a chart to see values, and tap a legend name to hide or show a line",
    ],
  },
  {
    version: "3.6.0",
    features: ["Minimising is 25% faster"],
  },
  {
    version: "3.5.0",
    features: [
      "The orb widget is smaller and covers far less of your screen, with the orb itself unchanged",
      "Soft ripples leave the orb while Leo is working or writing",
      "The orb widget no longer shows what Leo is doing; the orb's animation says it",
      "Minimising is now the exact reverse of opening",
    ],
  },
  {
    version: "3.4.0",
    features: [
      "Your profile: add your name, birthday, location and interests in Settings, under Customise profile",
      "Personal greetings that fit the time of day, the day of the week and your birthday",
      "Leo uses your profile to help you more personally",
    ],
  },
  {
    version: "3.3.0",
    features: [
      "A new screen-off animation when you minimise Leo: the orb sinks, the window closes to a bright point that flies to the orb's spot and grows into the orb",
      "Opening the chat plays that backwards, then the orb floats up",
      "Chat history is now a sidebar that slides in and out",
      "Every chat has its own delete button, and Leo names each chat for you",
    ],
  },
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

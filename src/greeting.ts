// Personal greetings for the empty chat screen. They use the user's first
// name, the time of day on this computer, the day of the week and, when it is
// their birthday, a birthday wish. No name means no greeting (the plain
// "What can I do for you?" stays).
export interface GreetingProfile {
  name: string;
  /** YYYY-MM-DD or empty; only ever used here, on this device. */
  dob: string;
}

const DAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

function pick(items: string[], seed: number): string {
  return items[Math.abs(seed) % items.length];
}

function isLeap(year: number): boolean {
  return (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
}

export function isBirthday(dob: string, now: Date): boolean {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(dob.trim());
  if (!m) return false;
  let month = Number(m[2]);
  let day = Number(m[3]);
  // A 29 February birthday is marked on 28 February in years without one.
  if (month === 2 && day === 29 && !isLeap(now.getFullYear())) day = 28;
  return now.getMonth() + 1 === month && now.getDate() === day;
}

export function firstName(name: string): string {
  return name.trim().split(/\s+/)[0] ?? "";
}

/** The greeting for `now`, or null when there is no name. `seed` picks the
    variant, so it stays the same all session instead of flickering. */
export function greeting(profile: GreetingProfile, now: Date = new Date(), seed = 0): string | null {
  const name = firstName(profile.name);
  if (!name) return null;

  if (isBirthday(profile.dob, now)) {
    return pick(
      [
        `Happy birthday, ${name}! Hope today is a great one.`,
        `It's your day, ${name}. Happy birthday!`,
        `Happy birthday ${name}! What can I do to make your day easier?`,
      ],
      seed,
    );
  }

  const hour = now.getHours();
  const day = now.getDay();
  const dayName = DAYS[day];
  let pool: string[];

  if (hour < 4) {
    pool = [
      `Hey ${name}, night owl today?`,
      `Still up, ${name}? What's keeping you awake?`,
      `Burning the midnight oil, ${name}?`,
    ];
  } else if (hour < 6) {
    pool = [`Up before the sun, ${name}?`, `Early start, ${name}. What are we doing today?`];
  } else if (hour < 12) {
    pool = [
      `Good morning ${name}, how are you?`,
      `Morning, ${name}! Ready to start the day?`,
      `Good morning ${name}. What's first on the list?`,
    ];
    if (day === 1) pool.push(`Happy Monday, ${name}. Let's make it a good week.`);
    if (day === 0 || day === 6) pool.push(`Happy ${dayName}, ${name}. Taking it easy?`);
  } else if (hour < 17) {
    pool = [
      `Good afternoon ${name}, how are you?`,
      `Hey ${name}, how's your afternoon going?`,
      `Good afternoon, ${name}. What can I help with?`,
    ];
    if (day === 5) pool.push(`Happy Friday, ${name}! Almost the weekend.`);
    if (day === 0 || day === 6) pool.push(`Happy ${dayName}, ${name}. Anything fun planned?`);
  } else if (hour < 21) {
    pool = [
      `Good evening ${name}, how was your day?`,
      `Evening, ${name}! How are you doing?`,
      `Hey ${name}, winding down or still going?`,
    ];
    if (day === 5) pool.push(`It's Friday evening, ${name}. Any plans?`);
  } else {
    pool = [
      `Good evening ${name}, still at it?`,
      `Hey ${name}, a late one tonight?`,
      `Hi ${name}, what are we finishing tonight?`,
    ];
  }
  return pick(pool, seed);
}

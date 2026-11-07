/**
 * A pipeline's schedule in words (T-3259): the five cron fields the runner reads, in UTC, turned
 * into a choice a person makes ("every 15 minutes", "daily at 03:00") and back, and the next runs
 * the schedule makes.
 */

/** One field as the values it allows; `null` for `*`. */
type Field = Set<number> | null;

export interface Cron {
  minute: Field;
  hour: Field;
  day: Field;
  month: Field;
  weekday: Field;
}

const BOUNDS: [number, number][] = [
  [0, 59],
  [0, 23],
  [1, 31],
  [1, 12],
  [0, 7],
];

// One field: a star, a star with a step, `a`, `a-b`, `a-b` with a step, and lists of them;
// `undefined` when it is not one.
function fieldOf(text: string, [low, high]: [number, number]): Field | undefined {
  if (text === "*") return null;
  const values = new Set<number>();
  for (const part of text.split(",")) {
    const match = /^(\*|(\d+)(?:-(\d+))?)(?:\/(\d+))?$/.exec(part);
    if (!match) return undefined;
    const step = match[4] === undefined ? 1 : Number(match[4]);
    const from = match[1] === "*" ? low : Number(match[2]);
    const to = match[1] === "*" ? high : match[3] === undefined ? (match[4] === undefined ? from : high) : Number(match[3]);
    if (step < 1 || from < low || to > high || from > to) return undefined;
    for (let value = from; value <= to; value += step) values.add(value);
  }
  return values;
}

/** The five fields, or `undefined` for text the runner would not read as a schedule. */
export function parseCron(text: string): Cron | undefined {
  const parts = text.trim().split(/\s+/);
  if (parts.length !== 5) return undefined;
  const fields = parts.map((part, at) => fieldOf(part, BOUNDS[at]));
  if (fields.some((field) => field === undefined)) return undefined;
  const [minute, hour, day, month, weekday] = fields as Field[];
  // Sunday is 0 and 7 alike.
  if (weekday?.has(7)) weekday.add(0);
  return { minute, hour, day, month, weekday };
}

const allows = (field: Field, value: number) => field === null || field.has(value);

/** Whether a UTC date is a day the schedule runs: cron's rule, day OR weekday when both are set. */
function dayMatches(cron: Cron, at: Date): boolean {
  if (!allows(cron.month, at.getUTCMonth() + 1)) return false;
  const day = allows(cron.day, at.getUTCDate());
  const weekday = allows(cron.weekday, at.getUTCDay());
  if (cron.day !== null && cron.weekday !== null) return day || weekday;
  return day && weekday;
}

/**
 * The next `count` starts after `from`, read in UTC as the runner reads them; fewer when the
 * schedule names no time within four years (a 31 February).
 */
export function nextRuns(cron: Cron, from: Date, count = 5): Date[] {
  const runs: Date[] = [];
  const at = new Date(from.getTime());
  at.setUTCSeconds(0, 0);
  at.setUTCMinutes(at.getUTCMinutes() + 1);
  const end = from.getTime() + 4 * 366 * 86_400_000;
  while (runs.length < count && at.getTime() < end) {
    if (!dayMatches(cron, at)) {
      at.setUTCHours(24, 0, 0, 0);
    } else if (!allows(cron.hour, at.getUTCHours())) {
      at.setUTCHours(at.getUTCHours() + 1, 0, 0, 0);
    } else if (!allows(cron.minute, at.getUTCMinutes())) {
      at.setUTCMinutes(at.getUTCMinutes() + 1);
    } else {
      runs.push(new Date(at.getTime()));
      at.setUTCMinutes(at.getUTCMinutes() + 1);
    }
  }
  return runs;
}

/** What a person picks; `custom` is any cron the choices do not say. */
export type Choice =
  | { kind: "minutes"; every: number }
  | { kind: "hourly"; minute: number }
  | { kind: "hours"; every: number; minute: number }
  | { kind: "daily"; hour: number; minute: number }
  | { kind: "weekly"; weekday: number; hour: number; minute: number }
  | { kind: "custom" };

export const MINUTE_STEPS = [5, 10, 15, 30] as const;
export const HOUR_STEPS = [2, 3, 4, 6, 12] as const;

/** The cron a choice writes; a custom choice has none of its own. */
export function cronOf(choice: Choice): string | undefined {
  switch (choice.kind) {
    case "minutes":
      return `*/${choice.every} * * * *`;
    case "hourly":
      return `${choice.minute} * * * *`;
    case "hours":
      return `${choice.minute} */${choice.every} * * *`;
    case "daily":
      return `${choice.minute} ${choice.hour} * * *`;
    case "weekly":
      return `${choice.minute} ${choice.hour} * * ${choice.weekday}`;
    case "custom":
      return undefined;
  }
}

/** The choice a cron says, read from its text: anything else is `custom`. */
export function choiceOf(text: string): Choice {
  const parts = text.trim().split(/\s+/);
  if (parts.length !== 5 || parseCron(text) === undefined) return { kind: "custom" };
  const [minute, hour, day, month, weekday] = parts;
  const number = (part: string) => (/^\d+$/.test(part) ? Number(part) : undefined);
  const step = (part: string) => /^\*\/(\d+)$/.exec(part)?.[1];
  if (day !== "*" || month !== "*") return { kind: "custom" };
  const m = number(minute);
  const h = number(hour);
  const d = number(weekday);
  if (weekday === "*") {
    const everyMinutes = step(minute);
    if (everyMinutes && hour === "*" && (MINUTE_STEPS as readonly number[]).includes(Number(everyMinutes))) {
      return { kind: "minutes", every: Number(everyMinutes) };
    }
    if (m !== undefined && hour === "*") return { kind: "hourly", minute: m };
    const everyHours = step(hour);
    if (m !== undefined && everyHours && (HOUR_STEPS as readonly number[]).includes(Number(everyHours))) {
      return { kind: "hours", every: Number(everyHours), minute: m };
    }
    if (m !== undefined && h !== undefined) return { kind: "daily", hour: h, minute: m };
  } else if (m !== undefined && h !== undefined && d !== undefined) {
    return { kind: "weekly", weekday: d % 7, hour: h, minute: m };
  }
  return { kind: "custom" };
}

/** Seconds between two starts, from the next two runs; `undefined` for a schedule with fewer. */
export function intervalSeconds(cron: Cron, from: Date): number | undefined {
  const [first, second] = nextRuns(cron, from, 2);
  return first && second ? (second.getTime() - first.getTime()) / 1000 : undefined;
}

/** The seconds between two updates of a source by its EU frequency; none for an irregular one. */
export const FREQUENCY_SECONDS: Record<string, number> = {
  HOURLY: 3_600,
  DAILY: 86_400,
  WEEKLY: 7 * 86_400,
  MONTHLY: 30 * 86_400,
  QUARTERLY: 91 * 86_400,
  ANNUAL: 365 * 86_400,
};

/**
 * A cron in the words a person reads ("Every 30 minutes"), or `undefined` for a schedule the
 * choices do not say, which the caller shows as the cron itself.
 */
export function scheduleWords(
  text: string,
  t: (key: string, values?: Record<string, unknown>) => string,
  locale: string,
): string | undefined {
  const choice = choiceOf(text);
  const time = (hour: number, minute: number) => `${String(hour).padStart(2, "0")}:${String(minute).padStart(2, "0")}`;
  switch (choice.kind) {
    case "minutes":
      return t("pipelines.schedule.say.minutes", { count: choice.every });
    case "hourly":
      return t("pipelines.schedule.say.hourly", { minute: choice.minute });
    case "hours":
      return t("pipelines.schedule.say.hours", { count: choice.every, minute: choice.minute });
    case "daily":
      return t("pipelines.schedule.say.daily", { time: time(choice.hour, choice.minute) });
    case "weekly": {
      // 2023-01-01 was a Sunday: day 0 of cron's week.
      const weekday = new Intl.DateTimeFormat(locale, { weekday: "long", timeZone: "UTC" }).format(
        new Date(Date.UTC(2023, 0, 1 + choice.weekday)),
      );
      return t("pipelines.schedule.say.weekly", { weekday, time: time(choice.hour, choice.minute) });
    }
    case "custom":
      return undefined;
  }
}

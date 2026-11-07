import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

/**
 * UI-32: the 60-second units of `tasks/60-seconds-roadmap.md`, measured (T-1601).
 *
 * Every journey spec already timed the person's part and asserted it; nothing kept the wall time
 * from the first click to Live, and nothing wrote either down. Each journey now writes its entry
 * into one JSON report — a unit that has no journey yet is listed there as not measured, so the
 * report names the gap instead of hiding it. A journey over its budget is marked `over`: that
 * entry is what becomes a task, with the numbers copied from this file, never typed.
 */
const REPORT = process.env.JOURNEYS_REPORT ?? "test-results/journeys.json";
const BUDGET_SECONDS = 60;

/** The units of the roadmap and the live spec that plays each, `null` where none does yet. */
const UNITS: Record<string, string | null> = {
  Find: null,
  Share: "share.spec.ts",
  Analyse: "analyse.spec.ts",
  Load: "load.spec.ts",
  Build: null,
  Model: null,
};

export type Unit = "Share" | "Analyse" | "Load";

interface Entry {
  spec: string | null;
  budgetSeconds: number;
  /** The person's own clicks and typing, the waits for the platform left out. */
  personSeconds?: number;
  /** Wall time from the first click to the result being Live, platform waits included. */
  firstClickToLiveSeconds?: number;
  over?: boolean;
  measuredAt?: string;
  note?: string;
}

function read(): Record<string, Entry> {
  const blank = Object.fromEntries(
    Object.entries(UNITS).map(([unit, spec]) => [
      unit,
      spec ? { spec, budgetSeconds: BUDGET_SECONDS } : { spec, budgetSeconds: BUDGET_SECONDS, note: "no journey spec yet" },
    ]),
  );
  if (!existsSync(REPORT)) return blank;
  const stored = (JSON.parse(readFileSync(REPORT, "utf8")) as { units?: Record<string, Entry> }).units ?? {};
  return { ...blank, ...stored };
}

function write(unit: Unit, change: Partial<Entry>): void {
  const units = read();
  const entry = { ...units[unit], ...change, measuredAt: new Date().toISOString() } as Entry;
  entry.over = [entry.personSeconds, entry.firstClickToLiveSeconds].some(
    (seconds) => seconds !== undefined && seconds > entry.budgetSeconds,
  );
  units[unit] = entry;
  mkdirSync(dirname(REPORT), { recursive: true });
  writeFileSync(REPORT, JSON.stringify({ budgetSeconds: BUDGET_SECONDS, units }, null, 2));
}

/**
 * Starts a unit's clock at its first click. `person` is written as soon as the person's part is
 * known, before the spec asserts it, so a journey that fails its minute still leaves its number;
 * `live` stops the wall clock where the spec has seen the result Live.
 */
export function journeyClock(unit: Unit): { person: (ms: number) => void; live: () => void } {
  const firstClick = Date.now();
  write(unit, { personSeconds: undefined, firstClickToLiveSeconds: undefined });
  return {
    person: (ms) => write(unit, { personSeconds: Math.round(ms / 100) / 10 }),
    live: () => write(unit, { firstClickToLiveSeconds: Math.round((Date.now() - firstClick) / 100) / 10 }),
  };
}

/**
 * What a chart over an entity type counts (UI-93, T-3257): how many entities hold each value of an
 * attribute, how a number spreads over equal bins, and which of the charts an attribute suggests.
 * Pure, so each rule is tested without a page.
 */

export type ChartKind = "temporal-chart" | "bar-chart" | "histogram";

/** The most bars a bar chart draws; the rest are counted together as one more. */
export const MAX_BARS = 12;
/** The bins a histogram spreads a number over. */
export const BINS = 10;

type Entity = Record<string, unknown>;

/** An attribute's value, keyValues or normalized. */
export function valueOf(entity: Entity, property: string): unknown {
  const raw = entity[property];
  if (raw && typeof raw === "object" && !Array.isArray(raw) && "value" in raw) return (raw as { value: unknown }).value;
  return raw;
}

/** The unit code a normalized attribute carries, the first entity's that has one. */
export function unitCodeOf(entities: Entity[], property: string): string | undefined {
  for (const entity of entities) {
    const raw = entity[property];
    const code = raw && typeof raw === "object" ? (raw as { unitCode?: unknown }).unitCode : undefined;
    if (typeof code === "string" && code !== "") return code;
  }
  return undefined;
}

/**
 * How many entities hold each value, the most common first; values past `MAX_BARS` are one bar
 * counted as `rest`. An entity without the attribute is not a value of it and is left out.
 */
export function barsOf(entities: Entity[], property: string): { bars: { label: string; count: number }[]; rest: number } {
  const counts = new Map<string, number>();
  for (const entity of entities) {
    const value = valueOf(entity, property);
    if (value === undefined || value === null || value === "") continue;
    const label = typeof value === "object" ? JSON.stringify(value) : String(value);
    counts.set(label, (counts.get(label) ?? 0) + 1);
  }
  const sorted = [...counts].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  return {
    bars: sorted.slice(0, MAX_BARS).map(([label, count]) => ({ label, count })),
    rest: sorted.slice(MAX_BARS).reduce((sum, [, count]) => sum + count, 0),
  };
}

/** The numbers spread over `BINS` equal bins from the lowest to the highest; one value is one bin. */
export function binsOf(entities: Entity[], property: string): { from: number; to: number; count: number }[] {
  const numbers = entities.map((entity) => Number(valueOf(entity, property))).filter((n) => Number.isFinite(n));
  if (numbers.length === 0) return [];
  const low = Math.min(...numbers);
  const high = Math.max(...numbers);
  if (low === high) return [{ from: low, to: high, count: numbers.length }];
  const width = (high - low) / BINS;
  const bins = Array.from({ length: BINS }, (_, i) => ({ from: low + i * width, to: low + (i + 1) * width, count: 0 }));
  for (const n of numbers) bins[Math.min(BINS - 1, Math.floor((n - low) / width))].count += 1;
  return bins;
}

const NUMERIC = new Set(["integer", "float", "double", "decimal", "number"]);

/**
 * The chart an attribute suggests: its history when its values carry `observedAt`, a histogram
 * for a number, bars for anything else (a text, an enumeration, a boolean).
 */
export function suggestChart(slot: { range?: string; values?: string[] } | undefined, samples: Entity[], property: string): ChartKind {
  const observed = samples.some((entity) => {
    const raw = entity[property];
    return Boolean(raw && typeof raw === "object" && "observedAt" in raw);
  });
  if (observed) return "temporal-chart";
  if (slot?.values && slot.values.length > 0) return "bar-chart";
  if (slot?.range && NUMERIC.has(slot.range)) return "histogram";
  const numbers = samples.map((entity) => valueOf(entity, property)).filter((value) => value !== undefined && value !== null);
  return numbers.length > 0 && numbers.every((value) => typeof value === "number") ? "histogram" : "bar-chart";
}

/** The reader's own time zone, as a time axis states it. Only the zone is read, so the locale named
 * here formats nothing a person sees. */
export function readerTimeZone(): string {
  return new Intl.DateTimeFormat("en").resolvedOptions().timeZone || "UTC";
}

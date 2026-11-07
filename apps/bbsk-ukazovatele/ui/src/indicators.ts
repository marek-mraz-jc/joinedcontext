/**
 * One `KeyPerformanceIndicator` entity, read as a card can show it (T-2308, PF-54, PF-55).
 *
 * Everything here is decided from the entity and from the published contract, never guessed. Two
 * decisions are the reason this file is separate from the components and tested on its own:
 *
 * - **Whose number it is.** The body comes from the entity's own id — the `{space}` segment of
 *   the URN, `bbsk-kpi` or `banskabystrica-kpi`. Both bodies are projects of one Organization, so
 *   the `{orgDomain}` is the same for both (`Development/10` §1); a space's segment is unique in
 *   the Organization and only its own project's pipeline writes there, so the writing project
 *   cannot forge it. The territory comes from the `{localId}` suffix, which is
 *   where `Development/10` §6 puts it, since the published schema closes the attribute set and
 *   leaves no `territory` attribute to carry it. A region figure and a city figure differ by a
 *   factor of eight, so a card that cannot say which it is must say nothing at all.
 * - **The threshold state.** It is the reader's, not the entity's: the limits are Directive
 *   2008/50/EC's, quoted in `Development/11` §2, and an indicator with no published limit gets
 *   no state rather than a made-up one.
 */
import type { RichCell, RichRow } from "@joinedcontext/sdk";

/** The two publishers, by the `{space}` segment of an indicator's URN (`Development/10` §1). */
export type Body = "bbsk" | "banskabystrica";

export const BODY_OF_SPACE: Readonly<Record<string, Body>> = {
  "bbsk-kpi": "bbsk",
  "banskabystrica-kpi": "banskabystrica",
};

/** `green` and over: the states of an indicator that has a published limit value. */
export type State = "green" | "amber" | "red";

export interface Indicator {
  id: string;
  /** The `{localId}`, which is also `name.value`. */
  name: string;
  /** The indicator without its territory, e.g. `pm10-24h`. */
  key: string;
  /** The territory token of `Development/10` §6, e.g. `kraj`, `okres-brezno`, `mesto`. */
  territory: string;
  body: Body;
  /** A number, or `null` when the pipeline wrote "not measured" for a window it found empty. */
  value: number | null;
  /** The UN/CEFACT code beside the value; absent exactly when the value is. */
  unitCode?: string;
  /** The window the value covers, as the pipeline recorded it. */
  period?: { start: string; end: string };
  formula: string;
  /** When the pipeline ran. It is not the window: the two diverging is how stale the source is. */
  updatedAt?: string;
  /** Read against the limits below; `null` for an indicator no published limit covers. */
  state: State | null;
}

/**
 * The limit values, from Directive 2008/50/EC as `Development/11` §2 quotes them. `amber` is the
 * directive's own upper assessment threshold and `red` its limit value; neither is a number
 * chosen for this dashboard. An indicator absent from this table has no published limit and is
 * shown without a state, which is a fact about the indicator and not a gap in the application.
 */
export const LIMITS: Readonly<Record<string, { amber: number; red: number }>> = {
  "pm10-24h": { amber: 35, red: 50 },
  "pm25-rok": { amber: 17, red: 25 },
};

/** The territory suffixes of `Development/10` §6 that are not a prefixed family. */
const WHOLE_TERRITORIES = ["kraj", "mesto"];

/** True for the body's whole territory (the region, the city) rather than one of its parts. */
export function isWhole(territory: string): boolean {
  return WHOLE_TERRITORIES.includes(territory);
}

/**
 * The districts of one indicator that carry a number, largest first: the bars of its chart.
 *
 * The whole territory is left out. It is the districts' sum or their mean, so beside them it would
 * flatten every bar to a sliver; it keeps its own card instead. A district not measured has no bar
 * rather than a zero one, the same rule the cards follow.
 */
export function districtBars(rows: Indicator[]): { territory: string; value: number }[] {
  return rows
    .filter((row): row is Indicator & { value: number } => !isWhole(row.territory) && row.value !== null)
    .map((row) => ({ territory: row.territory, value: row.value }))
    .sort((a, b) => b.value - a.value || a.territory.localeCompare(b.territory, "sk"));
}
const TERRITORY_FAMILIES = ["okres", "cast"];

/**
 * Splits `{key}-{territory}` at the territory, which is the suffix and never the head.
 *
 * `emisie-tuhe-km2-okres-ziar-nad-hronom` splits at `okres-`, not at the last hyphen, so a
 * district whose name is three words keeps its name. A name whose suffix is no declared
 * territory is refused rather than shown under an invented one.
 */
export function splitName(name: string): { key: string; territory: string } | null {
  for (const territory of WHOLE_TERRITORIES) {
    if (name.endsWith(`-${territory}`)) {
      return { key: name.slice(0, -territory.length - 1), territory };
    }
  }
  for (const family of TERRITORY_FAMILIES) {
    const at = name.lastIndexOf(`-${family}-`);
    if (at > 0) {
      return { key: name.slice(0, at), territory: name.slice(at + 1) };
    }
  }
  return null;
}

/** The state a value is in, or `null` when the indicator has no published limit or no value. */
export function stateOf(key: string, value: number | null): State | null {
  const limit = LIMITS[key];
  if (!limit || value === null) return null;
  if (value >= limit.red) return "red";
  if (value >= limit.amber) return "amber";
  return "green";
}

function one(cells: Record<string, RichCell | RichCell[]>, attr: string): RichCell | undefined {
  const cell = cells[attr];
  return Array.isArray(cell) ? cell[0] : cell;
}

function text(cell: RichCell | undefined): string | undefined {
  const value = cell?.kind === "relationship" ? cell.object : cell?.value;
  return typeof value === "string" ? value : undefined;
}

/**
 * The entity as a card reads it, or `null` for an entity this application will not show.
 *
 * Refused, deliberately and in this order: anything that is not a `KeyPerformanceIndicator`; an
 * id that is not the six-segment URN of an indicator space this application knows; a `name` that disagrees
 * with the id's `{localId}`, which would let one indicator be displayed under another's heading;
 * and a name whose suffix is no declared territory. Each of those is a number nobody could place,
 * and a number nobody can place is worse on a public dashboard than a number that is missing.
 */
export function toIndicator(row: RichRow): Indicator | null {
  if (row.type !== "KeyPerformanceIndicator") return null;
  const segments = row.id.split(":");
  if (segments.length !== 6 || segments[0] !== "urn" || segments[1] !== "ngsi-ld") return null;
  const body = Object.hasOwn(BODY_OF_SPACE, segments[4]) ? BODY_OF_SPACE[segments[4]] : undefined;
  if (!body) return null;

  const name = text(one(row.cells, "name"));
  if (!name || name !== segments[5]) return null;
  const split = splitName(name);
  if (!split) return null;

  const current = one(row.cells, "currentValue");
  const value = typeof current?.value === "number" ? current.value : null;
  const period = one(row.cells, "calculationPeriod")?.value;

  return {
    id: row.id,
    name,
    key: split.key,
    territory: split.territory,
    body,
    value,
    unitCode: value === null ? undefined : current?.unitCode,
    period: isPeriod(period) ? period : undefined,
    formula: text(one(row.cells, "calculationFormula")) ?? "",
    updatedAt: text(one(row.cells, "updatedAt")),
    state: stateOf(split.key, value),
  };
}

function isPeriod(value: unknown): value is { start: string; end: string } {
  if (typeof value !== "object" || value === null) return false;
  const period = value as Record<string, unknown>;
  return typeof period.start === "string" && typeof period.end === "string";
}

/**
 * Indicators grouped by their key, in the order a reader meets them: the ones with a limit
 * first, then the rest by name, and inside a group the region's whole territory before its
 * districts. The order is the model's, so the two bodies' sections cannot drift apart.
 */
export function byKey(indicators: Indicator[]): { key: string; rows: Indicator[] }[] {
  const groups = new Map<string, Indicator[]>();
  for (const indicator of indicators) {
    const rows = groups.get(indicator.key);
    if (rows) rows.push(indicator);
    else groups.set(indicator.key, [indicator]);
  }
  return [...groups.entries()]
    .map(([key, rows]) => ({ key, rows: [...rows].sort(compareTerritory) }))
    .sort((a, b) => rank(a.key) - rank(b.key) || a.key.localeCompare(b.key));
}

const rank = (key: string) => (LIMITS[key] ? 0 : 1);

function compareTerritory(a: Indicator, b: Indicator): number {
  const whole = (i: Indicator) => (WHOLE_TERRITORIES.includes(i.territory) ? 0 : 1);
  return whole(a) - whole(b) || a.territory.localeCompare(b.territory, "sk");
}

/**
 * The UN/CEFACT code each indicator's value is written with, from `Development/11` §4.
 *
 * The codes name the numerator only — `TNE` is tonnes, `LTR` litres, `C62` a count — while three
 * of the five indicators are ratios whose denominator lives in the name and the formula. The
 * card therefore shows a written unit from the locale (`t/km²`, `l/os./deň`) rather than the bare
 * code, and this table is what earns it: a value whose `unitCode` is not the one the contract
 * fixes is shown with its own raw code instead, because the entity is then not the quantity the
 * label would claim.
 */
export const UNIT_CODE: Readonly<Record<string, string>> = {
  "pm10-24h": "GQ",
  "pm25-rok": "GQ",
  "spotreba-vody-obyvatel": "LTR",
  "emisie-tuhe-km2": "TNE",
  "obyvatelstvo-stav": "C62",
};

/** True when the value carries the unit the contract fixes for its indicator. */
export function unitAsContracted(indicator: Indicator): boolean {
  return indicator.unitCode !== undefined && indicator.unitCode === UNIT_CODE[indicator.key];
}

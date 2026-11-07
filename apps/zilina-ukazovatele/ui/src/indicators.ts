/**
 * One `KeyPerformanceIndicator` of Žilina as a card shows it (T-3140, PF-54, Development/13).
 *
 * The six indicators and the unit each is written with are the ones `Development/13` defines; an
 * entity outside them, one whose id is not the city's indicator space, or one whose `name`
 * disagrees with the id's `{localId}` is refused, because a number nobody can place is worse on a
 * public dashboard than a number that is missing. None has a threshold, so none has a state.
 */
import type { RichCell, RichRow } from "@joinedcontext/sdk";

/** The indicators of `Development/13` §2, in the order the dashboard shows them. */
export const KEYS = [
  "obyvatelstvo-stav",
  "celkovy-prirastok",
  "priemerny-vek",
  "index-starnutia",
  "uchadzaci",
  "navstevnici-rok",
] as const;
export type Key = (typeof KEYS)[number];

/** The UN/CEFACT code each is written with (`Development/13` §2). */
export const UNIT_CODE: Readonly<Record<Key, string>> = {
  "obyvatelstvo-stav": "C62",
  "celkovy-prirastok": "C62",
  "priemerny-vek": "ANN",
  "index-starnutia": "P1",
  uchadzaci: "C62",
  "navstevnici-rok": "C62",
};

const SPACE = "zilina-kpi";
const TERRITORY = "mesto";

export interface Indicator {
  id: string;
  key: Key;
  /** A number, or `null` when the pipeline wrote "not measured". */
  value: number | null;
  /** True when the value carries the unit `Development/13` fixes; a card shows the raw code otherwise. */
  unitAsDefined: boolean;
  unitCode: string | null;
  /** The window the value covers, as the pipeline recorded it. */
  period: { start: string; end: string } | null;
  formula: string;
  /** When the pipeline ran, which is not the window. */
  updatedAt: string | null;
}

function one(row: RichRow, attr: string): RichCell | undefined {
  const cell = row.cells[attr];
  return Array.isArray(cell) ? cell[0] : cell;
}

function text(cell: RichCell | undefined): string | null {
  const value = cell?.kind === "relationship" ? cell.object : cell?.value;
  if (typeof value === "string") return value;
  // `updatedAt` is a typed literal: `{ "@type": "DateTime", "@value": … }`.
  if (typeof value === "object" && value !== null && typeof (value as { "@value"?: unknown })["@value"] === "string") {
    return (value as { "@value": string })["@value"];
  }
  return null;
}

function isKey(value: string): value is Key {
  return (KEYS as readonly string[]).includes(value);
}

/** The entity as a card reads it, or `null` for one this dashboard will not show. */
export function toIndicator(row: RichRow): Indicator | null {
  if (row.type !== "KeyPerformanceIndicator") return null;
  const segments = row.id.split(":");
  if (segments.length !== 6 || segments[0] !== "urn" || segments[1] !== "ngsi-ld" || segments[4] !== SPACE) return null;
  const name = text(one(row, "name"));
  if (!name || name !== segments[5] || !name.endsWith(`-${TERRITORY}`)) return null;
  const key = name.slice(0, -TERRITORY.length - 1);
  if (!isKey(key)) return null;

  const current = one(row, "currentValue");
  const value = typeof current?.value === "number" && Number.isFinite(current.value) ? current.value : null;
  const unitCode = value !== null && typeof current?.unitCode === "string" ? current.unitCode : null;
  const period = one(row, "calculationPeriod")?.value as { start?: unknown; end?: unknown } | undefined;
  return {
    id: row.id,
    key,
    value,
    unitAsDefined: unitCode === UNIT_CODE[key],
    unitCode,
    period: typeof period?.start === "string" && typeof period.end === "string" ? { start: period.start, end: period.end } : null,
    formula: text(one(row, "calculationFormula")) ?? "",
    updatedAt: text(one(row, "updatedAt")),
  };
}

/** The window as a reader names it: a quarter, a year, or its two days. */
export function windowOf(period: { start: string; end: string }): { kind: "quarter" | "year" | "days"; label: string } {
  const start = period.start.slice(0, 10);
  const end = period.end.slice(0, 10);
  const year = start.slice(0, 4);
  if (start === `${year}-01-01` && end === `${year}-12-31`) return { kind: "year", label: year };
  const quarters: Record<string, string> = { "01-01": "Q1", "04-01": "Q2", "07-01": "Q3", "10-01": "Q4" };
  const ends: Record<string, string> = { Q1: "03-31", Q2: "06-30", Q3: "09-30", Q4: "12-31" };
  const quarter = quarters[start.slice(5)];
  if (quarter && end === `${year}-${ends[quarter]}`) return { kind: "quarter", label: `${quarter} ${year}` };
  return { kind: "days", label: `${start} – ${end}` };
}

/**
 * The region's bridges as a maintenance desk reads them (T-2784): code, road, year built, spans,
 * length, material, heritage status, manager and district from the register `bbsk-registre`. A
 * bridge's age is this year minus the year it was built, and only when that year was published;
 * what stands out is judged against the region's own bridges (the oldest and the longest tenth),
 * never against a threshold this application would have to invent.
 */
import type { RichCell, RichRow } from "@joinedcontext/sdk";

export interface Bridge {
  id: string;
  name: string | null;
  code: string | null;
  roadClass: string | null;
  roadNumber: string | null;
  yearBuilt: number | null;
  age: number | null;
  spans: number | null;
  length: number | null;
  material: string | null;
  heritage: string | null;
  manager: string | null;
  district: string | null;
}

function first(cell: RichCell | RichCell[] | undefined): RichCell | undefined {
  return Array.isArray(cell) ? cell[0] : cell;
}

function count(row: RichRow, attr: string): number | null {
  const value = first(row.cells[attr])?.value;
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;
}

function text(row: RichRow, attr: string, locale: string): string | null {
  const cell = first(row.cells[attr]);
  const map = cell?.languageMap;
  const value = map ? (map[locale] ?? map.sk ?? Object.values(map)[0]) : cell?.value;
  if (typeof value === "number") return String(value);
  return typeof value === "string" && value.trim() !== "" ? value.trim() : null;
}

export function bridgeOf(row: RichRow, locale: string, year: number): Bridge {
  const yearBuilt = count(row, "yearBuilt");
  return {
    id: row.id,
    name: text(row, "name", locale),
    code: text(row, "bridgeCode", locale),
    roadClass: text(row, "roadClass", locale),
    roadNumber: text(row, "roadNumber", locale),
    yearBuilt,
    // A year in the future is a typing error in the register, not a bridge of negative age.
    age: yearBuilt !== null && yearBuilt <= year ? year - yearBuilt : null,
    spans: count(row, "spanCount"),
    length: count(row, "bridgedLength"),
    material: text(row, "structureMaterial", locale),
    heritage: text(row, "heritageStatus", locale),
    manager: text(row, "managerName", locale),
    district: text(row, "districtName", locale),
  };
}

export interface Totals {
  bridges: number;
  /** Over the bridges that published a length. */
  length: number;
  /** The median age over the bridges with a known year, `null` without any. */
  medianAge: number | null;
  listed: number;
  /** Bridges missing the year or the length a desk sorts by. */
  incomplete: number;
}

export function totals(bridges: Bridge[]): Totals {
  const ages = bridges.map((b) => b.age).filter((a): a is number => a !== null).sort((a, b) => a - b);
  const middle = Math.floor(ages.length / 2);
  return {
    bridges: bridges.length,
    length: bridges.reduce((sum, b) => sum + (b.length ?? 0), 0),
    medianAge: ages.length === 0 ? null : ages.length % 2 === 1 ? ages[middle] : (ages[middle - 1] + ages[middle]) / 2,
    listed: bridges.filter((b) => b.heritage === "cultural" || b.heritage === "culturalAndTechnical").length,
    incomplete: bridges.filter((b) => b.age === null || b.length === null).length,
  };
}

/**
 * The value at or above which a bridge is in the highest tenth of the region. `null` with fewer
 * than ten bridges: a tenth of a handful is one bridge, and calling it an outlier would be a claim
 * the data cannot carry.
 */
export function highestTenth(values: (number | null)[]): number | null {
  const known = values.filter((v): v is number => v !== null).sort((a, b) => a - b);
  if (known.length < 10) return null;
  // A tenth is ceil(n / 10) values, never fewer: the first of the highest of them is the threshold.
  return known[known.length - Math.ceil(known.length / 10)];
}

export type SortKey = "name" | "age" | "length" | "spans";

/** Sorted by `key`; a missing value goes last whichever the direction, so it never tops a list. */
export function sorted(bridges: Bridge[], key: SortKey, ascending: boolean): Bridge[] {
  return [...bridges].sort((a, b) => {
    const x = a[key];
    const y = b[key];
    if (x === null || y === null) return x === null ? (y === null ? 0 : 1) : -1;
    const order = typeof x === "string" ? x.localeCompare(String(y), "sk") : (x as number) - (y as number);
    return ascending ? order : -order;
  });
}

/** One CSV field: quoted when it holds a separator, a quote or a line break; quotes doubled. */
function field(value: string | number | null): string {
  if (value === null) return "";
  const text = typeof value === "number" ? String(Math.round(value * 100) / 100) : value;
  // A cell a spreadsheet would run as a formula is prefixed, so an exported name never executes.
  const safe = /^[=+\-@\t\r]/.test(text) ? `'${text}` : text;
  return /[",\n\r]/.test(safe) ? `"${safe.replace(/"/g, '""')}"` : safe;
}

/** The desk's table as CSV, the computed age included, with the given header and value words. */
export function toCsv(bridges: Bridge[], header: string[], words: Record<string, string>): string {
  const word = (value: string | null) => (value === null ? null : (words[value] ?? value));
  const rows = bridges.map((b) =>
    [b.name, b.code, word(b.roadClass), b.roadNumber, b.yearBuilt, b.age, b.spans, b.length, b.material, word(b.heritage), b.manager, b.district]
      .map(field)
      .join(","),
  );
  return [header.map(field).join(","), ...rows].join("\r\n") + "\r\n";
}

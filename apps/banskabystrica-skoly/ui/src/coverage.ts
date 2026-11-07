/**
 * The schools of the city as a coverage desk reads them (T-2782): pupils, staff and budget from
 * the national school map, and the two ratios a desk compares, pupils per teacher and budget per
 * pupil. A ratio is computed only from counts the school actually published; a missing count makes
 * the ratio missing, never zero. What stands out is judged against the city's own schools (the
 * highest or lowest tenth), never against a threshold this application would have to invent.
 */
import { format } from "@joinedcontext/sdk";
import type { Cell, RichCell, RichRow } from "@joinedcontext/sdk";

export interface School {
  id: string;
  name: string | null;
  address: string | null;
  pupils: number | null;
  teachers: number | null;
  otherStaff: number | null;
  budget: number | null;
  budgetYear: number | null;
  pupilsPerTeacher: number | null;
  budgetPerPupil: number | null;
  /** How many schools publish this school's staff and budget identically; 1 = its own. */
  sharedWith: number;
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
  // A Smart Data Models PostalAddress (T-4329) reads as its one line.
  if (value && typeof value === "object" && !Array.isArray(value)) return format(value as Cell) || null;
  return typeof value === "string" && value.trim() !== "" ? value.trim() : null;
}

export function schoolOf(row: RichRow, locale: string): School {
  const pupils = count(row, "pupilCount");
  const teachers = count(row, "teachingStaff");
  const budget = count(row, "annualBudget");
  return {
    id: row.id,
    name: text(row, "name", locale),
    address: text(row, "address", locale),
    pupils,
    teachers,
    otherStaff: count(row, "nonTeachingStaff"),
    budget,
    budgetYear: count(row, "budgetYear"),
    pupilsPerTeacher: pupils !== null && teachers !== null && teachers > 0 ? pupils / teachers : null,
    budgetPerPupil: budget !== null && pupils !== null && pupils > 0 ? budget / pupils : null,
    sharedWith: 1,
  };
}

/** The staff and budget a school published, as one key; none when it published no staff. */
function figures(school: School): string | null {
  return school.teachers !== null && school.teachers > 0 && school.budget !== null
    ? `${school.teachers}|${school.otherStaff}|${school.budget}`
    : null;
}

/**
 * The national school map repeats one organization's staff and budget on each of its schools (the
 * city's kindergartens are one budget organization). Identical figures on several schools are that
 * organization's: no school gets a ratio of its own from them, and `totals` counts them once.
 */
export function withShared(schools: School[]): School[] {
  const seen = new Map<string, number>();
  for (const school of schools) {
    const key = figures(school);
    if (key) seen.set(key, (seen.get(key) ?? 0) + 1);
  }
  return schools.map((school) => {
    const key = figures(school);
    const sharedWith = key ? (seen.get(key) ?? 1) : 1;
    return sharedWith > 1 ? { ...school, sharedWith, pupilsPerTeacher: null, budgetPerPupil: null } : school;
  });
}

export interface Totals {
  schools: number;
  pupils: number;
  teachers: number;
  /** Over the schools that published both counts: sum over sum, not a mean of ratios. */
  pupilsPerTeacher: number | null;
  /** Schools missing any of the counts the ratios need. */
  incomplete: number;
}

export function totals(schools: School[]): Totals {
  // One unit per organization: a school of its own, or the schools that share one set of figures
  // (`withShared`), whose pupils meet that organization's teachers once.
  const units = new Map<string, { pupils: number | null; teachers: number | null }>();
  for (const s of schools) {
    const key = s.sharedWith > 1 ? (figures(s) ?? s.id) : s.id;
    const unit = units.get(key) ?? { pupils: null, teachers: s.teachers };
    if (s.pupils !== null) unit.pupils = (unit.pupils ?? 0) + s.pupils;
    units.set(key, unit);
  }
  const both = [...units.values()].filter((u) => u.pupils !== null && u.teachers !== null && u.teachers > 0);
  const pupils = both.reduce((sum, u) => sum + (u.pupils ?? 0), 0);
  const teachers = both.reduce((sum, u) => sum + (u.teachers ?? 0), 0);
  return {
    schools: schools.length,
    pupils: schools.reduce((sum, s) => sum + (s.pupils ?? 0), 0),
    teachers: [...units.values()].reduce((sum, u) => sum + (u.teachers ?? 0), 0),
    pupilsPerTeacher: teachers > 0 ? pupils / teachers : null,
    incomplete: schools.filter((s) => s.pupilsPerTeacher === null || s.budgetPerPupil === null).length,
  };
}

/**
 * The value at or above which a school is in the highest tenth of the city (`side: "high"`), or at
 * or below which it is in the lowest (`"low"`). `null` with fewer than ten schools: a tenth of a
 * handful is one school, and calling it an outlier would be a claim the data cannot carry.
 */
export function tenth(values: (number | null)[], side: "high" | "low"): number | null {
  const known = values.filter((v): v is number => v !== null).sort((a, b) => a - b);
  if (known.length < 10) return null;
  // A tenth is ceil(n / 10) values, never fewer; the threshold is the innermost of them.
  const size = Math.ceil(known.length / 10);
  const at = side === "high" ? known.length - size : size - 1;
  return known[at];
}

export type SortKey = "name" | "pupils" | "teachers" | "pupilsPerTeacher" | "budgetPerPupil";

/** Sorted by `key`; a missing value goes last whichever the direction, so it never tops a list. */
export function sorted(schools: School[], key: SortKey, ascending: boolean): School[] {
  return [...schools].sort((a, b) => {
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

/** The desk's table as CSV, the computed ratios included, with the given header words. */
export function toCsv(schools: School[], header: string[]): string {
  const rows = schools.map((s) =>
    [s.name, s.address, s.pupils, s.teachers, s.pupilsPerTeacher, s.otherStaff, s.budget, s.budgetYear, s.budgetPerPupil]
      .map(field)
      .join(","),
  );
  return [header.map(field).join(","), ...rows].join("\r\n") + "\r\n";
}

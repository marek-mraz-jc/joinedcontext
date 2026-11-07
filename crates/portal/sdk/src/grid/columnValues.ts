/**
 * The values a column holds across the whole answer, for its header filter's tick list (T-6328).
 *
 * The endpoint has no "distinct values" read, so the list is tallied from the answer itself: the
 * same question the grid asks (minus the column's own filter), only the one attribute, in pages of
 * a thousand. It reads through the grid's own source, so the caller's Endpoint grant decides which
 * entities and values it sees, as it does for the rows.
 */
import type { EntitySource, GridQuery } from "./source";
import type { RichCell } from "./model";

/** One value of a column and how many of the read entities hold it. */
export interface ValueCount {
  value: string;
  count: number;
}

export interface ColumnValues {
  /** Most frequent first, then by the value. */
  values: ValueCount[];
  /** How many entities were read. */
  scanned: number;
  /** The whole answer's size, when the source counts. */
  total?: number;
  /** The list covers fewer entities than the answer holds, or stopped at MAX_VALUES. */
  capped: boolean;
}

export const VALUE_PAGE = 1000;
/** ponytail: a tally over at most 5 000 entities; a broker-side distinct read is the upgrade path. */
export const VALUE_SCAN_MAX = 5000;
export const MAX_VALUES = 500;

/** A cell's value as a tick-list entry: a scalar's text, a relationship's target; nothing for a structured value. */
function valuesOf(cell: RichCell | RichCell[] | undefined): string[] {
  const cells = Array.isArray(cell) ? cell : cell ? [cell] : [];
  const out: string[] = [];
  for (const one of cells) {
    const raw = one.kind === "relationship" ? one.object : one.value;
    for (const each of Array.isArray(raw) ? raw : [raw]) {
      if (typeof each === "string" || typeof each === "number" || typeof each === "boolean") out.push(String(each));
    }
  }
  return out;
}

export async function columnValues(source: EntitySource, query: GridQuery, attr: string): Promise<ColumnValues> {
  const asked: GridQuery = { ...query, attrs: [attr], orderBy: undefined };
  const counts = new Map<string, number>();
  let scanned = 0;
  let total: number | undefined;
  for (;;) {
    const page = await source.query(asked, { offset: scanned, limit: VALUE_PAGE, count: scanned === 0 });
    if (scanned === 0) total = page.total;
    scanned += page.rows.length;
    for (const row of page.rows) {
      for (const value of valuesOf(row.cells[attr])) counts.set(value, (counts.get(value) ?? 0) + 1);
    }
    const more = total !== undefined ? scanned < total : page.rows.length === VALUE_PAGE;
    if (page.rows.length === 0 || !more || scanned >= VALUE_SCAN_MAX) {
      const values = [...counts].map(([value, count]) => ({ value, count }));
      values.sort((a, b) => b.count - a.count || a.value.localeCompare(b.value, undefined, { numeric: true }));
      return {
        values: values.slice(0, MAX_VALUES),
        scanned,
        total,
        capped: more || values.length > MAX_VALUES,
      };
    }
  }
}

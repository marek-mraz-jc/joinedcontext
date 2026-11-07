import type { RichCell, RichRow } from "./model";

/** An attribute name NGSI-LD `q` takes as written: no brackets, quotes or operators. */
const ATTR = /^[A-Za-z_][\w]*$/;
/** A text value short and plain enough to quote in a one-line hint. */
const PLAIN = /^[^"\\;|()]{1,30}$/;
/** A date reads better as a filter than as an equality: the example leaves it out. */
const DATE = /^\d{4}-\d{2}-\d{2}/;

/**
 * The example query the "Edit as text" hint shows (T-5415): built from the shown type's own columns
 * and the first row's values, so it names an attribute this type has and matches at least that row,
 * e.g. `count>=121` on a traffic counter. A number comes first, then a short text; undefined when
 * the page has no row or no plain value to quote.
 */
export function queryExample(attrs: readonly string[], row: RichRow | undefined): string | undefined {
  if (!row) return undefined;
  const single = (attr: string): RichCell | undefined => {
    const cell = row.cells[attr];
    return cell && !Array.isArray(cell) && cell.kind === "property" ? cell : undefined;
  };
  const named = attrs.filter((attr) => ATTR.test(attr));
  for (const attr of named) {
    const value = single(attr)?.value;
    if (typeof value === "number" && Number.isFinite(value)) return `${attr}>=${value}`;
  }
  for (const attr of named) {
    const value = single(attr)?.value;
    if (typeof value === "string" && PLAIN.test(value) && !DATE.test(value)) return `${attr}=="${value}"`;
  }
  return undefined;
}

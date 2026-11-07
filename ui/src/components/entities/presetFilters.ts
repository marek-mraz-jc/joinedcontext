/**
 * A default filter (DM-134) as the data view's filter row: the Portal's own reading of a `q`,
 * kept out of the SDK's public surface because the app builder's prompt carries that surface.
 */
import { opsForKind, parseQ } from "@joinedcontext/sdk";
import type { ColumnFilter, FilterColumn, FilterOp } from "@joinedcontext/sdk";

const FILLED_OPS: Record<string, FilterOp> = { "==": "equals", "!=": "notEquals", ">": "gt", ">=": "gte", "<": "lt", "<=": "lte" };

/** A `q` value as a person types it: a quoted string unquoted and unescaped, anything else as written. */
function typedValue(raw: string): string {
  return /^".*"$/.test(raw) ? raw.slice(1, -1).replace(/\\(.)/g, "$1") : raw;
}

/**
 * One input per column the `q` names, so the reader sees and edits what narrows the rows instead
 * of a query. `null` when the `q` asks something the inputs cannot hold (an `|`, a metadata path,
 * a pattern, two terms on one column that are not a range, a column the type lacks): the caller
 * then shows it as text.
 */
export function filtersFromQ(q: string, columns: readonly FilterColumn[]): Record<string, ColumnFilter> | null {
  const parsed = parseQ(q);
  if (parsed === null || parsed.length !== 1) return null;
  const filters: Record<string, ColumnFilter> = {};
  for (const term of parsed[0]) {
    if (term.path.length !== 1) return null;
    const column = columns.find((each) => each.attr === term.path[0] && !each.meta && opsForKind(each.kind).length > 0);
    if (!column) return null;
    const value = term.value === undefined ? "" : typedValue(term.value);
    const op: FilterOp | undefined = term.op === undefined ? "present" : FILLED_OPS[term.op];
    if (!op) return null;
    const before = filters[column.key];
    if (column.kind === "enum" && op === "equals") {
      if (before) return null;
      filters[column.key] = { op: "anyOf", value: "", values: term.value?.split(",").map(typedValue) ?? [] };
      continue;
    }
    if (!opsForKind(column.kind).includes(op)) return null;
    if (before) {
      // `a>=x;a<=y` is one range; any other pair on a column is not one input.
      if (before.op === "gte" && op === "lte") filters[column.key] = { op: "between", value: before.value, value2: value };
      else if (before.op === "lte" && op === "gte") filters[column.key] = { op: "between", value, value2: before.value };
      else return null;
      continue;
    }
    filters[column.key] = { op, value };
  }
  return filters;
}

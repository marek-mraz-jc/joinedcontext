/**
 * The dashboard file an App keeps in `src/dashboard.json` (T-6402), as the editor changes it: the
 * parts a person edits are typed here, every other member of the file is carried as it was. The
 * Portal checks the whole file again before it proposes it (src/api/app_dashboard.rs).
 */

export type Tone = "neutral" | "info" | "success" | "warning" | "danger";
export type Agg = "count" | "sum" | "avg" | "min" | "max";
export type Op = "==" | "!=" | ">" | ">=" | "<" | "<=";

export const TONES: Tone[] = ["danger", "warning", "success", "info", "neutral"];
export const AGGS: Agg[] = ["count", "sum", "avg", "min", "max"];
export const OPS: Op[] = ["==", "!=", ">", ">=", "<", "<="];

export interface Column {
  attr: string;
  label?: string;
  formula?: string;
  [carried: string]: unknown;
}
export interface Filter {
  attr: string;
  op: Op;
  value: string | number | boolean;
}
export interface Band {
  below?: number;
  tone: Tone;
  label: string;
}
export interface Kpi {
  label: string;
  agg: Agg;
  attr?: string;
  where?: string;
}
export interface DashboardFile {
  title?: string;
  grid: { columns?: Column[]; [carried: string]: unknown };
  filters?: Filter[];
  colour?: { attr: string; bands: Band[] };
  chart?: { x: string; y?: string; agg: Agg };
  kpis?: Kpi[];
  [carried: string]: unknown;
}

/** The server's refusals by the field they name: `config/grid/columns/0/formula: why`. */
export function errorsByPath(errors: readonly string[]): Map<string, string[]> {
  const out = new Map<string, string[]>();
  for (const error of errors) {
    const match = /^config(\/[^:]*)?: (.*)$/s.exec(error);
    const path = match?.[1] ?? "";
    const list = out.get(path) ?? [];
    list.push(match?.[2] ?? error);
    out.set(path, list);
  }
  return out;
}

/** Where a refusal points in the form: the field's element id and its words, by locale key. */
export interface RefusedField {
  id?: string;
  section: string;
  row?: number;
  field?: string;
}

const PLACES: Record<string, { prefix: string; section: string; fields: Record<string, string> }> = {
  "grid/columns": { prefix: "col", section: "columns", fields: { attr: "attribute", label: "label", formula: "formula" } },
  filters: { prefix: "filter", section: "filters", fields: { attr: "attribute", op: "op", value: "value" } },
  "colour/bands": { prefix: "band", section: "colour", fields: { below: "below", tone: "tone", label: "label" } },
  kpis: { prefix: "kpi", section: "kpis", fields: { label: "label", agg: "agg", attr: "attribute", where: "where" } },
};
const SINGLE: Record<string, RefusedField> = {
  "/colour/attr": { id: "colour-attr", section: "colour", field: "colourAttr" },
  "/colour/bands": { section: "colour" },
  "/chart/x": { id: "chart-x", section: "chart", field: "chartX" },
  "/chart/agg": { id: "chart-agg", section: "chart", field: "agg" },
  "/chart/y": { id: "chart-y", section: "chart", field: "chartY" },
};

/** The form field a refusal path names (`/kpis/0/where`), or undefined for one the form does not show. */
export function refusedField(path: string): RefusedField | undefined {
  if (SINGLE[path]) return SINGLE[path];
  const match = /^\/(grid\/columns|filters|colour\/bands|kpis)\/(\d+)(?:\/(\w+))?$/.exec(path);
  const place = match ? PLACES[match[1]] : undefined;
  if (!match || !place) return undefined;
  const row = Number(match[2]);
  const field = match[3] ? place.fields[match[3]] : undefined;
  return { id: field ? `${place.prefix}-${row}-${match[3]}` : undefined, section: place.section, row: row + 1, field };
}

/** A filter's value as typed: a number stays a number, true/false a boolean, the rest text. */
export function typedValue(text: string): string | number | boolean {
  if (text === "true" || text === "false") return text === "true";
  const number = Number(text);
  return text.trim() !== "" && Number.isFinite(number) ? number : text;
}

/** `list` with item `i` replaced, or removed when `next` is null. */
export function replaced<T>(list: readonly T[] | undefined, i: number, next: T | null): T[] {
  const out = [...(list ?? [])];
  if (next === null) out.splice(i, 1);
  else out[i] = next;
  return out;
}

/** The names a value can be read from: the stored attributes, then the computed columns. */
export function readableNames(attributes: readonly string[], file: DashboardFile): string[] {
  const computed = (file.grid.columns ?? []).filter((c) => c.formula !== undefined).map((c) => c.attr);
  return [...attributes, ...computed.filter((name) => name !== "" && !attributes.includes(name))];
}

/** The lines of `after` that `before` lacks (+) and the other way round (-), for the preview. */
export function lineDiff(before: string, after: string): { sign: "+" | "-" | " "; text: string }[] {
  const a = before.split("\n");
  const b = after.split("\n");
  // A longest-common-subsequence walk: the file is at most 64 KiB, a few hundred lines.
  const lcs: number[][] = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0));
  for (let i = a.length - 1; i >= 0; i--)
    for (let j = b.length - 1; j >= 0; j--) lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
  const out: { sign: "+" | "-" | " "; text: string }[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length || j < b.length) {
    if (i < a.length && j < b.length && a[i] === b[j]) {
      out.push({ sign: " ", text: a[i++] });
      j++;
    } else if (j < b.length && (i === a.length || lcs[i][j + 1] >= lcs[i + 1][j])) out.push({ sign: "+", text: b[j++] });
    else out.push({ sign: "-", text: a[i++] });
  }
  return out;
}

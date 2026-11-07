/**
 * A dashboard App's editable part (T-6402): one JSON file, `src/dashboard.json`, that the App
 * renders and a person changes in the Portal without code. It holds the grid (columns, computed
 * columns, map), the filters, one colour rule, one chart and the KPI tiles. The Portal checks the
 * same rules on the server (src/api/app_dashboard.rs) before it proposes the edit as a Change, so
 * an App never receives a file this parse refuses. The shared cases in
 * tests/fixtures/dashboard_cases.json hold the two halves to one answer.
 */

import { parseGridConfig, type ConfigFinding, type EntityGridConfig, type ResolvedGridConfig } from "./config";
import { evaluate, parseFormula, type Formula } from "./formula";
import type { RichCell, RichRow } from "./model";

export type DashboardTone = "neutral" | "info" | "success" | "warning" | "danger";
export type DashboardAgg = "count" | "sum" | "avg" | "min" | "max";
export type DashboardOp = "==" | "!=" | ">" | ">=" | "<" | "<=";

/** A row is in the first band whose `below` its value is under; the last band may omit `below`. */
export interface ColourBand {
  below?: number;
  tone: DashboardTone;
  label: string;
}

export interface DashboardFilter {
  attr: string;
  op: DashboardOp;
  value: number | string | boolean;
}

export interface DashboardKpi {
  label: string;
  agg: DashboardAgg;
  /** The attribute the tile adds up; `count` needs none. */
  attr?: string;
  /** A formula a row must make true to be counted, e.g. `availableBikeNumber == 0`. */
  where?: string;
}

export interface DashboardConfig {
  title?: string;
  grid: EntityGridConfig;
  filters?: DashboardFilter[];
  colour?: { attr: string; bands: ColourBand[] };
  chart?: { x: string; y: string; agg: DashboardAgg };
  kpis?: DashboardKpi[];
}

export const TONES: readonly DashboardTone[] = ["neutral", "info", "success", "warning", "danger"];
export const AGGS: readonly DashboardAgg[] = ["count", "sum", "avg", "min", "max"];
export const FILTER_OPS: readonly DashboardOp[] = ["==", "!=", ">", ">=", "<", "<="];
export const MAX_BANDS = 6;
export const MAX_KPIS = 6;
export const MAX_FILTERS = 8;
const MAX_LABEL = 60;

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

function unknownKeys(raw: Record<string, unknown>, allowed: string[], path: string, findings: ConfigFinding[]) {
  for (const key of Object.keys(raw)) if (!allowed.includes(key)) findings.push({ path: `${path}/${key}`, message: "unknown property" });
}

function label(raw: unknown, path: string, findings: ConfigFinding[]) {
  if (typeof raw !== "string" || raw.trim() === "" || raw.length > MAX_LABEL) findings.push({ path, message: `must be a text of 1 to ${MAX_LABEL} characters` });
}

/** What every section check shares: the findings so far and the names a value may come from. */
interface Checker {
  findings: ConfigFinding[];
  stored: readonly string[];
  readable: readonly string[];
  named(name: unknown, path: string, names: readonly string[]): void;
  formula(text: unknown, path: string): void;
}

function checker(findings: ConfigFinding[], known: readonly string[] | undefined, computed: readonly string[]): Checker {
  const stored = known ?? [];
  return {
    findings,
    stored,
    readable: [...stored, ...computed],
    named(name, path, names) {
      if (typeof name !== "string" || name === "") findings.push({ path, message: "must name an attribute" });
      else if (known && !names.includes(name)) findings.push({ path, message: `"${name}" is no attribute of this type you may read` });
    },
    formula(text, path) {
      if (typeof text !== "string") return void findings.push({ path, message: "must be a string" });
      const parsed = parseFormula(text, known);
      if (!parsed.ok) findings.push({ path, message: `${parsed.error} (at character ${parsed.at + 1})` });
    },
  };
}

function checkGrid(grid: Record<string, unknown>, columns: Record<string, unknown>[], c: Checker): void {
  if (isObject(grid.map) && grid.map.attr !== undefined) c.named(grid.map.attr, "/grid/map/attr", c.stored);
  if (isObject(grid.filters) && grid.filters.preset !== undefined) c.findings.push({ path: "/grid/filters/preset", message: "a dashboard keeps its filters in `filters`" });
  columns.forEach((column, i) => {
    if (column.formula === undefined) return c.named(column.attr, `/grid/columns/${i}/attr`, c.stored);
    if (typeof column.attr !== "string" || !/^[A-Za-z0-9_]+$/.test(column.attr) || c.stored.includes(column.attr)) c.findings.push({ path: `/grid/columns/${i}/attr`, message: "a computed column needs a new name of letters, digits and _, such as occupancy" });
    c.formula(column.formula, `/grid/columns/${i}/formula`);
  });
}

function checkFilter(item: unknown, path: string, c: Checker): void {
  if (!isObject(item)) return void c.findings.push({ path, message: "must be an object" });
  unknownKeys(item, ["attr", "op", "value"], path, c.findings);
  c.named(item.attr, `${path}/attr`, c.stored);
  if (!FILTER_OPS.includes(item.op as DashboardOp)) c.findings.push({ path: `${path}/op`, message: `must be one of ${FILTER_OPS.join(" ")}` });
  const v = item.value;
  const ok = (typeof v === "number" && Number.isFinite(v)) || typeof v === "boolean" || (typeof v === "string" && v.length <= 200 && !/["\\;|()\n]/.test(v));
  if (!ok) c.findings.push({ path: `${path}/value`, message: 'must be a number, true/false or a text of at most 200 characters without " \\ ; | ( )' });
}

function checkFilters(filters: unknown, c: Checker): void {
  if (!Array.isArray(filters) || filters.length > MAX_FILTERS) return void c.findings.push({ path: "/filters", message: `must be a list of at most ${MAX_FILTERS} filters` });
  filters.forEach((item: unknown, i: number) => checkFilter(item, `/filters/${i}`, c));
}

/** The bands, each `below` greater than the one before; only the last may leave it out. */
function checkBands(bands: unknown[], c: Checker): void {
  let last = -Infinity;
  bands.forEach((band: unknown, i: number) => {
    const path = `/colour/bands/${i}`;
    if (!isObject(band)) return c.findings.push({ path, message: "must be an object" });
    unknownKeys(band, ["below", "tone", "label"], path, c.findings);
    if (band.below === undefined) {
      if (i !== bands.length - 1) c.findings.push({ path: `${path}/below`, message: "only the last band may leave out `below`" });
    } else if (typeof band.below !== "number" || !Number.isFinite(band.below) || band.below <= last) {
      c.findings.push({ path: `${path}/below`, message: "must be a number greater than the band before" });
    } else last = band.below;
    if (!TONES.includes(band.tone as DashboardTone)) c.findings.push({ path: `${path}/tone`, message: `must be one of ${TONES.join(", ")}` });
    label(band.label, `${path}/label`, c.findings);
  });
}

function checkColour(colour: unknown, c: Checker): void {
  if (!isObject(colour)) return void c.findings.push({ path: "/colour", message: "must be an object" });
  unknownKeys(colour, ["attr", "bands"], "/colour", c.findings);
  c.named(colour.attr, "/colour/attr", c.readable);
  const bands = colour.bands;
  if (!Array.isArray(bands) || bands.length === 0 || bands.length > MAX_BANDS) c.findings.push({ path: "/colour/bands", message: `must be a list of 1 to ${MAX_BANDS} bands` });
  else checkBands(bands, c);
}

function checkChart(chart: unknown, c: Checker): void {
  if (!isObject(chart)) return void c.findings.push({ path: "/chart", message: "must be an object" });
  unknownKeys(chart, ["x", "y", "agg"], "/chart", c.findings);
  c.named(chart.x, "/chart/x", c.readable);
  if (!AGGS.includes(chart.agg as DashboardAgg)) c.findings.push({ path: "/chart/agg", message: `must be one of ${AGGS.join(", ")}` });
  if (chart.agg !== "count" || chart.y !== undefined) c.named(chart.y, "/chart/y", c.readable);
}

function checkKpi(kpi: unknown, path: string, c: Checker): void {
  if (!isObject(kpi)) return void c.findings.push({ path, message: "must be an object" });
  unknownKeys(kpi, ["label", "agg", "attr", "where"], path, c.findings);
  label(kpi.label, `${path}/label`, c.findings);
  if (!AGGS.includes(kpi.agg as DashboardAgg)) c.findings.push({ path: `${path}/agg`, message: `must be one of ${AGGS.join(", ")}` });
  if (kpi.agg !== "count" || kpi.attr !== undefined) c.named(kpi.attr, `${path}/attr`, c.readable);
  if (kpi.where !== undefined) c.formula(kpi.where, `${path}/where`);
}

function checkKpis(kpis: unknown, c: Checker): void {
  if (!Array.isArray(kpis) || kpis.length > MAX_KPIS) return void c.findings.push({ path: "/kpis", message: `must be a list of at most ${MAX_KPIS} tiles` });
  kpis.forEach((kpi: unknown, i: number) => checkKpi(kpi, `/kpis/${i}`, c));
}

/**
 * The dashboard file `raw` resolved, or why not, as JSON paths. With `known` (the stored attributes
 * of the grid's type) every attribute name is checked against it; a computed column's name counts
 * as known where a value is read (colour, chart, tiles) but never inside a formula.
 */
export function parseDashboardConfig(raw: unknown, known?: readonly string[]): { config?: DashboardConfig & { grid: ResolvedGridConfig }; findings: ConfigFinding[] } {
  const findings: ConfigFinding[] = [];
  if (!isObject(raw)) return { findings: [{ path: "", message: "must be an object" }] };
  unknownKeys(raw, ["title", "grid", "filters", "colour", "chart", "kpis"], "", findings);
  if (raw.title !== undefined) label(raw.title, "/title", findings);
  const grid = parseGridConfig(raw.grid);
  findings.push(...grid.findings.map((f) => ({ ...f, path: `/grid${f.path}` })));
  const rawColumns = isObject(raw.grid) && Array.isArray(raw.grid.columns) ? raw.grid.columns.filter(isObject) : [];
  const computed = rawColumns.filter((col) => col.formula !== undefined && typeof col.attr === "string").map((col) => col.attr as string);
  const c = checker(findings, known, computed);
  if (isObject(raw.grid)) checkGrid(raw.grid, rawColumns, c);
  if (raw.filters !== undefined) checkFilters(raw.filters, c);
  if (raw.colour !== undefined) checkColour(raw.colour, c);
  if (raw.chart !== undefined) checkChart(raw.chart, c);
  if (raw.kpis !== undefined) checkKpis(raw.kpis, c);
  if (findings.length > 0 || !grid.config) return { findings };
  return { config: { ...(raw as unknown as DashboardConfig), grid: grid.config }, findings };
}

/** The NGSI-LD `q` the filters ask the Endpoint for, joined by `;`. */
export function filtersQ(filters: readonly DashboardFilter[] = []): string | undefined {
  const parts = filters.map((f) => `${f.attr}${f.op}${typeof f.value === "string" ? `"${f.value}"` : String(f.value)}`);
  return parts.length > 0 ? parts.join(";") : undefined;
}

function valueOf(cell: RichCell | RichCell[] | undefined): unknown {
  return (Array.isArray(cell) ? cell[0] : cell)?.value;
}

/** The band a row's value falls in, as the grid's `rowTone` takes it; no number, no tone. */
export function toneOf(colour: DashboardConfig["colour"], row: RichRow): { tone: string; label: string } | undefined {
  if (!colour) return undefined;
  const value = valueOf(row.cells[colour.attr]);
  if (typeof value !== "number") return undefined;
  const band = colour.bands.find((b) => b.below === undefined || value < b.below);
  return band ? { tone: band.tone, label: band.label } : undefined;
}

function aggregate(values: number[], agg: DashboardAgg): number | null {
  if (agg === "count") return values.length;
  if (values.length === 0) return null;
  if (agg === "sum") return values.reduce((a, b) => a + b, 0);
  if (agg === "avg") return values.reduce((a, b) => a + b, 0) / values.length;
  return agg === "min" ? Math.min(...values) : Math.max(...values);
}

/** A tile's number over the rows shown (computed columns already on them). */
export function kpiValue(kpi: DashboardKpi, rows: readonly RichRow[]): number | null {
  let where: Formula | undefined;
  if (kpi.where) {
    const parsed = parseFormula(kpi.where);
    if (!parsed.ok) return null;
    where = parsed.formula;
  }
  const kept = where ? rows.filter((row) => evaluate(where, (name) => valueOf(row.cells[name])) === true) : rows;
  if (kpi.agg === "count" && !kpi.attr) return kept.length;
  const values = kept.map((row) => valueOf(row.cells[kpi.attr ?? ""])).filter((v): v is number => typeof v === "number");
  return aggregate(values, kpi.agg);
}

/** The chart's bars: one per distinct `x`, largest first, at most `top`. */
export function chartBars(chart: NonNullable<DashboardConfig["chart"]>, rows: readonly RichRow[], top = 12): { x: string; y: number }[] {
  const groups = new Map<string, number[]>();
  for (const row of rows) {
    const x = valueOf(row.cells[chart.x]);
    if (x === undefined || x === null || typeof x === "object") continue;
    const y = valueOf(row.cells[chart.y]);
    const list = groups.get(String(x)) ?? [];
    if (typeof y === "number") list.push(y);
    else if (chart.agg === "count") list.push(0);
    groups.set(String(x), list);
  }
  return [...groups]
    .map(([x, values]) => ({ x, y: aggregate(values, chart.agg) }))
    .filter((bar): bar is { x: string; y: number } => bar.y !== null)
    .sort((a, b) => b.y - a.y)
    .slice(0, top);
}


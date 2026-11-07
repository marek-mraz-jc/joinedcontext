/**
 * The yearly series behind the region's indicators (T-2934, owner decision 2026-10-06, b1): each
 * indicator card holds one window, and the statistics office's rows in `bbsk-kraj` hold every
 * year of the cube the indicator is read from. The series is those rows, as published, never a
 * value recomputed here; the cube, indicator and key are the ones the pipeline `ukazovatele`
 * names in each indicator's `calculationFormula`.
 */
import type { RichCell, RichRow } from "@joinedcontext/sdk";

/** The raw space whose public endpoint serves the rows: the App's second further space (AP-04). */
export const RAW_SPACE = "bbsk-kraj";

/** Where each region indicator's yearly values are: cube, indicator and the key of its cell. */
export const SERIES_SOURCE: Readonly<Record<string, { cube: string; indicator: string; keys: readonly (string | null)[] }>> = {
  // The cube is read for both sexes together; a row without a key is that cell.
  "obyvatelstvo-stav": { cube: "om7102rr", indicator: "IN010115", keys: [null, "SPOLU"] },
  "emisie-tuhe-km2": { cube: "zp3803rs", indicator: "ODPAD_TONY_KM2", keys: ["1"] },
};

/** The territory tokens of `Development/10` §6, by the code the statistics office uses. */
export const TERRITORY_OF_AREA: Readonly<Record<string, string>> = {
  SK032: "kraj",
  SK0321: "okres-banska-bystrica",
  SK0322: "okres-banska-stiavnica",
  SK0323: "okres-brezno",
  SK0324: "okres-detva",
  SK0325: "okres-krupina",
  SK0326: "okres-lucenec",
  SK0327: "okres-poltar",
  SK0328: "okres-revuca",
  SK0329: "okres-rimavska-sobota",
  SK032A: "okres-velky-krtis",
  SK032B: "okres-zvolen",
  SK032C: "okres-zarnovica",
  SK032D: "okres-ziar-nad-hronom",
};

/** The `q` that asks the endpoint for exactly these rows. */
export const SERIES_QUERY = `dataSet==${Object.values(SERIES_SOURCE).map((s) => `"${s.cube}"`).join(",")};indicator==${Object.values(SERIES_SOURCE).map((s) => `"${s.indicator}"`).join(",")}`;

export interface Point {
  /** The year, `YYYY`. */
  period: string;
  value: number;
}

function one(row: RichRow, attr: string): RichCell | undefined {
  const cell = row.cells[attr];
  return Array.isArray(cell) ? cell[0] : cell;
}

function text(row: RichRow, attr: string): string | null {
  const value = one(row, attr)?.value;
  return typeof value === "string" ? value : null;
}

/** The series key of an indicator card: `{key}|{territory}`. */
export function seriesKey(key: string, territory: string): string {
  return `${key}|${territory}`;
}

/**
 * Every indicator's series, by `seriesKey`, oldest year first. A row of another cell, an unknown
 * territory, a period that is not a year or a value that is not a number is not a point; a year
 * that came twice keeps its first value rather than drawing two points at one x.
 */
export function seriesOf(rows: RichRow[]): Map<string, Point[]> {
  const out = new Map<string, Point[]>();
  for (const row of rows) {
    if (row.type !== "KeyPerformanceIndicator") continue;
    const cube = text(row, "dataSet");
    const indicator = text(row, "indicator");
    const key = text(row, "dimensionKey");
    const entry = Object.entries(SERIES_SOURCE).find(
      ([, source]) => source.cube === cube && source.indicator === indicator && source.keys.includes(key),
    );
    const area = text(row, "refArea");
    const territory = area === null ? undefined : TERRITORY_OF_AREA[area];
    const period = text(row, "refPeriod");
    const value = one(row, "kpiValue")?.value;
    if (!entry || !territory || period === null || !/^\d{4}$/.test(period) || typeof value !== "number" || !Number.isFinite(value)) continue;
    const id = seriesKey(entry[0], territory);
    const points = out.get(id) ?? [];
    if (!points.some((point) => point.period === period)) points.push({ period, value });
    out.set(id, points);
  }
  for (const points of out.values()) points.sort((a, b) => a.period.localeCompare(b.period));
  return out;
}

/** The polyline of a series in a `width` × `height` box, or `null` for fewer than two points. */
export function linePoints(points: Point[], width: number, height: number): string | null {
  if (points.length < 2) return null;
  const values = points.map((point) => point.value);
  const low = Math.min(...values);
  const high = Math.max(...values);
  const span = high - low || 1;
  return points
    .map((point, index) => {
      const x = (index / (points.length - 1)) * width;
      // A flat series is a line through the middle, not along an edge.
      const y = high === low ? height / 2 : height - ((point.value - low) / span) * height;
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
}

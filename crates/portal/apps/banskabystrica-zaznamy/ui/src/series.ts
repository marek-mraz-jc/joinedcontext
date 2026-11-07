/**
 * The records of one cube as charts a resident can read (T-2966): one chart per indicator and key,
 * named with the statistics office's own labels instead of its codes.
 *
 * A record is one cell of the cube: an indicator, a territory, a period, and the codes of the
 * cube's other dimensions joined in `dimensionKey`. A month (`7.`) or a quarter (`3.Q.`) among
 * those codes belongs to the period, so it joins the time axis; the other codes tell the charts
 * apart. What the axis of a chart is follows from what varies: several territories are compared
 * at the latest period, several periods are a line, and one territory with more keys than
 * periods (the city's residents by age, a snapshot a day) is a column per key of the latest one.
 */
import { AREAS, CODES, DATASETS, type Named } from "./labels";

export type Shape = "line" | "areas" | "keys";

export interface Point {
  /** What the point is along the axis: a period, a territory code or a key code. */
  at: string;
  label: string;
  value: number;
}

export interface Chart {
  id: string;
  title: string;
  /** The territory, or the territories compared, and the period of a comparison. */
  subtitle: string;
  unit: string;
  shape: Shape;
  points: Point[];
}

interface Cell {
  indicator: string;
  area: string;
  period: string;
  key: string;
  unit: string;
  value: number;
}

const MONTH = /^(\d{1,2})\.$/;
const QUARTER = /^(\d)\.Q\.$/;

/** A Named in the reader's language, the other one where only that exists, else the code. */
export function named(label: Named | undefined, language: "sk" | "en", code: string): string {
  return label?.[language] ?? label?.[language === "sk" ? "en" : "sk"] ?? code;
}

function property(entity: Record<string, unknown>, attr: string): unknown {
  const held = entity[attr];
  return typeof held === "object" && held !== null && "value" in held ? (held as { value: unknown }).value : held;
}

/** One record as a cell, or `null` for one without a number or an indicator. */
function cellOf(entity: Record<string, unknown>): Cell | null {
  const value = property(entity, "kpiValue");
  const indicator = property(entity, "indicator");
  if (typeof value !== "number" || !Number.isFinite(value) || typeof indicator !== "string") return null;
  let period = String(property(entity, "refPeriod") ?? "");
  const rest: string[] = [];
  for (const code of String(property(entity, "dimensionKey") ?? "").split("-").filter(Boolean)) {
    const month = MONTH.exec(code);
    const quarter = QUARTER.exec(code);
    if (month) period = `${period}-${month[1].padStart(2, "0")}`;
    else if (quarter) period = `${period}Q${quarter[1]}`;
    else rest.push(code);
  }
  return {
    indicator,
    area: String(property(entity, "refArea") ?? ""),
    period,
    key: rest.join("-"),
    unit: String(property(entity, "unitText") ?? ""),
    value,
  };
}

/** Every chart of one cube's records, in the order of their titles. */
export function chartsOf(
  entities: Record<string, unknown>[],
  dataSet: string,
  language: "sk" | "en",
  words: { keyAxis?: string; latest: (period: string) => string; codeNames?: Record<string, string> } = {
    latest: (period) => period,
  },
): Chart[] {
  const codes = CODES[dataSet] ?? {};
  const own = words.codeNames ?? {};
  const cells = entities.map(cellOf).filter((cell): cell is Cell => cell !== null);
  const areaName = (code: string) => named(AREAS[code], language, code);
  // A name the App gives a code the office's lists do not hold (the city's own table) comes first.
  const codeName = (code: string) => own[code] ?? named(codes[code], language, code);

  const byIndicator = new Map<string, Cell[]>();
  for (const cell of cells) byIndicator.set(cell.indicator, [...(byIndicator.get(cell.indicator) ?? []), cell]);

  const charts: Chart[] = [];
  for (const [indicator, group] of byIndicator) {
    const title = codeName(indicator);
    const unit = group[0].unit;
    const areas = new Set(group.map((cell) => cell.area));
    const periods = new Set(group.map((cell) => cell.period));
    const keys = new Set(group.map((cell) => cell.key));

    // One territory and more keys than periods (the residents by age, a snapshot a day): the
    // keys of the latest period are the axis.
    if (keys.size > 1 && areas.size === 1 && keys.size > periods.size) {
      const latest = [...periods].sort().at(-1) ?? "";
      const points = group
        .filter((cell) => cell.period === latest)
        .map((cell) => ({ at: cell.key, label: codeName(cell.key), value: cell.value }))
        .sort((a, b) => compareKeys(a.at, b.at));
      const [area] = areas;
      charts.push({
        id: `${dataSet}-${indicator}`,
        title: words.keyAxis ? `${title} · ${words.keyAxis}` : title,
        subtitle: `${areaName(area)} · ${words.latest(latest)}`,
        unit,
        shape: "keys",
        points,
      });
      continue;
    }

    // Otherwise one chart per key: the territories at the latest period, or a line over time.
    for (const key of [...keys].sort(compareKeys)) {
      const cellsOfKey = group.filter((cell) => cell.key === key);
      const id = [dataSet, indicator, key].filter(Boolean).join("-");
      const heading = key ? `${title} · ${key.split("-").map(codeName).join(", ")}` : title;
      const areasOfKey = new Set(cellsOfKey.map((cell) => cell.area));
      if (areasOfKey.size > 1) {
        const latest = [...new Set(cellsOfKey.map((cell) => cell.period))].sort().at(-1) ?? "";
        const points = cellsOfKey
          .filter((cell) => cell.period === latest)
          .map((cell) => ({ at: cell.area, label: areaName(cell.area), value: cell.value }))
          .sort((a, b) => b.value - a.value);
        charts.push({ id, title: heading, subtitle: words.latest(latest), unit, shape: "areas", points });
      } else {
        const [area] = areasOfKey;
        const points = cellsOfKey
          .map((cell) => ({ at: cell.period, label: cell.period, value: cell.value }))
          .sort((a, b) => a.at.localeCompare(b.at));
        charts.push({ id, title: heading, subtitle: areaName(area), unit, shape: "line", points });
      }
    }
  }
  return charts.sort((a, b) => a.title.localeCompare(b.title, language));
}

/** Keys that are numbers (an age) in their numeric order, the others as text, a total first. */
function compareKeys(a: string, b: string): number {
  if (a === b) return 0;
  if (a === "SPOLU" || a === "") return -1;
  if (b === "SPOLU" || b === "") return 1;
  const [x, y] = [Number(a), Number(b)];
  return Number.isFinite(x) && Number.isFinite(y) ? x - y : a.localeCompare(b);
}

/** The cube's title in the reader's language. */
export function datasetName(dataSet: string, language: "sk" | "en", extra: Record<string, string> = {}): string {
  return extra[dataSet] ?? named(DATASETS[dataSet], language, dataSet);
}

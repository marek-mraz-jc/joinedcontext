import type { Row } from "@joinedcontext/sdk";

export interface Indicator {
  name: string;
  category: string;
  unit: string;
  higherIsBetter: boolean;
  /** Observations in the period, oldest first. */
  points: Row[];
  latest: number | null;
  target: number | null;
}

export type Standing =
  | { kind: "unknown" }
  | { kind: "on-target"; by: number }
  | { kind: "short"; by: number };

function numberOf(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

/** The observations of the last `months` months, counted back from the newest one in the data. */
export function inPeriod(rows: Row[], months: number): Row[] {
  const dates = rows.map((row) => String(row.dateObserved ?? "")).filter((date) => date !== "");
  if (dates.length === 0) return [];
  const newest = new Date(dates.reduce((a, b) => (a > b ? a : b)));
  const from = new Date(Date.UTC(newest.getUTCFullYear(), newest.getUTCMonth() - months + 1, 1)).toISOString();
  return rows.filter((row) => String(row.dateObserved ?? "") >= from);
}

/** One indicator per name, its observations oldest first, its latest value and target. */
export function indicators(rows: Row[]): Indicator[] {
  const byName = new Map<string, Row[]>();
  for (const row of rows) {
    const name = typeof row.name === "string" ? row.name : "";
    if (name === "") continue;
    byName.set(name, [...(byName.get(name) ?? []), row]);
  }
  return [...byName.entries()]
    .map(([name, points]) => {
      const sorted = [...points].sort((a, b) => String(a.dateObserved ?? "").localeCompare(String(b.dateObserved ?? "")));
      const last = sorted.at(-1);
      return {
        name,
        category: typeof last?.category === "string" ? last.category : "",
        unit: typeof last?.unit === "string" ? last.unit : "",
        higherIsBetter: last?.higherIsBetter !== false,
        points: sorted,
        latest: numberOf(last?.kpiValue),
        target: numberOf(last?.target),
      };
    })
    .sort((a, b) => a.category.localeCompare(b.category) || a.name.localeCompare(b.name));
}

/** Where the latest value stands against the target, in the indicator's own direction. */
export function standing(indicator: Pick<Indicator, "latest" | "target" | "higherIsBetter">): Standing {
  const { latest, target, higherIsBetter } = indicator;
  if (latest === null || target === null) return { kind: "unknown" };
  const ahead = higherIsBetter ? latest - target : target - latest;
  return ahead >= 0 ? { kind: "on-target", by: ahead } : { kind: "short", by: -ahead };
}

/** How the trend moved over the period: better, worse or level, in the indicator's direction. */
export function trend(indicator: Pick<Indicator, "points" | "higherIsBetter">): "better" | "worse" | "level" | "unknown" {
  const values = indicator.points.map((row) => numberOf(row.kpiValue)).filter((value): value is number => value !== null);
  if (values.length < 2) return "unknown";
  const change = values[values.length - 1] - values[0];
  if (Math.abs(change) < 1e-9) return "level";
  return change > 0 === indicator.higherIsBetter ? "better" : "worse";
}

import { useId, useMemo, useState } from "react";
import { Card, currentTokens, format, Grid, Header, Page, useEntities } from "@joinedcontext/sdk";
import { ChartCard } from "./components/charts";
import { Empty, Loading, Problem } from "./components/states";
import { indicators, inPeriod, standing, trend, type Indicator } from "./indicators";

const TYPE = "KeyPerformanceIndicator";
const PERIODS = [
  { months: 3, label: "Last 3 months" },
  { months: 6, label: "Last 6 months" },
  { months: 12, label: "Last 12 months" },
];

function value(amount: number | null, unit: string): string {
  if (amount === null) return "—";
  const text = format(amount, "number");
  return unit === "%" ? `${text} %` : unit ? `${text} ${unit}` : text;
}

/** The standing as a word and a glyph: a person who does not see colour reads the same thing. */
function StandingLine({ indicator }: { indicator: Indicator }): React.JSX.Element {
  const where = standing(indicator);
  if (where.kind === "unknown") return <p className="app-standing">No target set</p>;
  const text =
    where.kind === "on-target"
      ? where.by === 0
        ? "On target"
        : `Ahead by ${value(where.by, indicator.unit)}`
      : `Short by ${value(where.by, indicator.unit)}`;
  return (
    <p className="app-standing" data-standing={where.kind}>
      <span aria-hidden="true">{where.kind === "on-target" ? "▲ " : "▼ "}</span>
      {text}
    </p>
  );
}

const TREND_WORD = { better: "Getting better", worse: "Getting worse", level: "Level", unknown: "Too few points for a trend" };

/** Round bounds and a round step around the values and the target, so the target line stays in view. */
export function axisRange(values: unknown[]): { min: number; max: number; interval: number } | { scale: true } {
  const numbers = values.filter((value): value is number => typeof value === "number" && Number.isFinite(value));
  if (numbers.length === 0) return { scale: true };
  const low = Math.min(...numbers);
  const high = Math.max(...numbers);
  const rough = (high - low || Math.abs(high) || 1) / 4;
  const power = 10 ** Math.floor(Math.log10(rough));
  const interval = [1, 2, 2.5, 5, 10].map((step) => step * power).find((step) => step >= rough) ?? 10 * power;
  // toPrecision drops the binary tail a decimal step leaves (3.6000000000000005).
  const min = Number((Math.floor(low / interval) * interval).toPrecision(12));
  const max = Number((Math.ceil(high / interval) * interval).toPrecision(12));
  return { min, max: max === min ? min + interval : max, interval };
}

function trendOption(indicator: Indicator): Record<string, unknown> {
  const tokens = currentTokens();
  return {
    color: tokens.chart.palette,
    tooltip: { trigger: "axis" },
    grid: { containLabel: true, left: 4, right: 12, top: 20, bottom: 4 },
    xAxis: { type: "category", data: indicator.points.map((row) => String(row.dateObserved ?? "").slice(0, 7)) },
    yAxis: { type: "value", ...axisRange([...indicator.points.map((row) => row.kpiValue), indicator.target]) },
    series: [
      {
        type: "line",
        name: indicator.name,
        showSymbol: false,
        data: indicator.points.map((row) => (typeof row.kpiValue === "number" ? row.kpiValue : null)),
        ...(indicator.target === null
          ? {}
          : {
              markLine: {
                symbol: "none",
                lineStyle: { type: "dashed", color: tokens.color.muted },
                label: { formatter: "Target", position: "insideStartTop" },
                data: [{ yAxis: indicator.target }],
              },
            }),
      },
    ],
  };
}

/** Every indicator's latest value as a share of its target, so different units sit on one axis. */
function comparisonOption(list: Indicator[]): Record<string, unknown> | null {
  const rows = list.filter((item) => item.latest !== null && item.target !== null && item.target !== 0);
  if (rows.length === 0) return null;
  const tokens = currentTokens();
  return {
    color: tokens.chart.palette,
    tooltip: { trigger: "axis", valueFormatter: (share: number) => `${format(share, "number")} % of target` },
    // The right margin leaves room for the value label past the longest bar.
    grid: { containLabel: true, left: 8, right: 64, top: 12, bottom: 8 },
    xAxis: { type: "value", axisLabel: { formatter: "{value} %", hideOverlap: true } },
    yAxis: { type: "category", data: rows.map((item) => item.name) },
    series: [
      {
        type: "bar",
        name: "Share of target",
        barMaxWidth: 28,
        label: { show: true, position: "right", formatter: "{c} %" },
        data: rows.map((item) => {
          const share = ((item.latest ?? 0) / (item.target ?? 1)) * 100;
          // Where lower is better, reaching the target means staying under it.
          return Math.round((item.higherIsBetter ? share : 200 - share) * 10) / 10;
        }),
        markLine: { symbol: "none", lineStyle: { type: "dashed", color: tokens.color.muted }, label: { show: false }, data: [{ xAxis: 100 }] },
      },
    ],
  };
}

/** Each indicator's latest value against its target, its trend over a period, and all of them compared. */
export default function App(): React.JSX.Element {
  const { rows, loading, error, reload } = useEntities(TYPE, undefined, { all: true });
  const [months, setMonths] = useState(6);
  const picker = useId();
  const list = useMemo(() => indicators(inPeriod(rows, months)), [rows, months]);
  const attention = list.filter((item) => standing(item).kind === "short").length;

  const period = (
    <div className="app-period">
      <label htmlFor={picker}>Period</label>
      <select id={picker} value={months} onChange={(event) => setMonths(Number(event.target.value))}>
        {PERIODS.map((option) => (
          <option key={option.months} value={option.months}>
            {option.label}
          </option>
        ))}
      </select>
    </div>
  );

  return (
    <div className="jc-shell">
      <main className="jc-main">
        <Page label="City indicators">
          <Header
            level={1}
            title="City indicators"
            subtitle={list.length === 0 ? "No indicator has reported yet" : attention === 0 ? "Every indicator is on target" : `${attention} of ${list.length} indicators need attention`}
            actions={period}
          />
          <Problem error={error} onRetry={reload} />
          {loading && rows.length === 0 ? (
            <Loading label="Loading the indicators…" />
          ) : list.length === 0 ? (
            !error && <Empty>No observations in this period.</Empty>
          ) : (
            <>
              <Grid columns={4}>
                {list.map((item) => (
                  <Card key={item.name} title={item.name} label={item.name}>
                    <p className="app-category">{item.category || "Indicator"}</p>
                    <p className="app-latest">{value(item.latest, item.unit)}</p>
                    {item.target !== null && <p className="app-target">Target {value(item.target, item.unit)}</p>}
                    <StandingLine indicator={item} />
                    <p className="app-trend">{TREND_WORD[trend(item)]}</p>
                    <ChartCard option={trendOption(item)} height={140} />
                  </Card>
                ))}
              </Grid>
              <Card title="Against target" label="Against target">
                <p className="app-category">The latest value of each indicator, as a share of its target; 100 % is on target.</p>
                <ChartCard option={comparisonOption(list)} height={56 * list.length + 48} empty="No indicator has both a value and a target." />
              </Card>
            </>
          )}
        </Page>
      </main>
    </div>
  );
}

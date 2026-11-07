import { useMemo } from "react";
import { currentTokens, format, Header, Page, useEntities } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { ChartCard } from "./components/charts";
import { EntityMap } from "./components/EntityMap";
import { Empty, Loading, Problem } from "./components/states";
import { dailyMeans, GUIDELINE, readings, stationMeans, summarise } from "./story";

const TYPE = "AirQualityObserved";
const DAY = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "long", timeZone: "UTC" });
const DAY_SHORT = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "short", timeZone: "UTC" });

function day(key: string, style = DAY): string {
  return style.format(new Date(`${key}T12:00:00Z`));
}

/** An axis that ends on a round number above both the data and the guideline. */
function roundMax(range: { max: number }): number {
  return Math.ceil((Math.max(range.max, GUIDELINE) + 1) / 5) * 5;
}

function ug(value: number): string {
  return `${format(Math.round(value * 10) / 10, "number")} µg/m³`;
}

/** The newest reading of each station: what the map shows. */
function latestPerStation(rows: Row[]): Row[] {
  const latest = new Map<string, Row>();
  for (const row of rows) {
    const name = String(row.stationName ?? "");
    const before = latest.get(name);
    if (!before || String(row.dateObserved ?? "") > String(before.dateObserved ?? "")) latest.set(name, row);
  }
  return [...latest.values()];
}

function guidelineLine(axis: "xAxis" | "yAxis"): Record<string, unknown> {
  return {
    symbol: "none",
    lineStyle: { type: "dashed", color: currentTokens().color.danger },
    // The station axis is inverted, so a vertical line starts at the top: its label goes there.
    label: { formatter: "WHO guideline", position: axis === "xAxis" ? "start" : "insideEndTop" },
    data: [{ [axis]: GUIDELINE }],
  };
}

/** A read-only story about the air: each claim in the text is a number computed from the rows. */
export default function App(): React.JSX.Element {
  const { rows, loading, error, reload } = useEntities(TYPE, undefined, { all: true });
  const list = useMemo(() => readings(rows), [rows]);
  const story = useMemo(() => summarise(list), [list]);
  const daily = useMemo(() => dailyMeans(list), [list]);
  const stations = useMemo(() => stationMeans(list), [list]);
  const peak = daily.reduce<(typeof daily)[number] | null>((top, item) => (!top || item.mean > top.mean ? item : top), null);

  if (error || (loading && rows.length === 0) || !story || !peak) {
    return (
      <Shell subtitle="Fine particles (PM2.5) at the city's stations">
        <Problem error={error} onRetry={reload} />
        {loading && rows.length === 0 ? <Loading label="Loading the readings…" /> : !error && <Empty>No readings to tell a story from yet.</Empty>}
      </Shell>
    );
  }

  const tokens = currentTokens();
  const stationOption = {
    color: tokens.chart.palette,
    tooltip: { trigger: "axis", valueFormatter: (value: number) => ug(value) },
    grid: { containLabel: true, left: 8, right: 40, top: 24, bottom: 8 },
    xAxis: { type: "value", name: "µg/m³", nameLocation: "middle", nameGap: 28, max: roundMax },
    yAxis: { type: "category", inverse: true, data: stations.map((item) => item.station) },
    series: [
      {
        type: "bar",
        name: "Mean PM2.5",
        barMaxWidth: 28,
        data: stations.map((item) => Math.round(item.mean * 10) / 10),
        markLine: guidelineLine("xAxis"),
      },
    ],
  };
  const dailyOption = {
    color: tokens.chart.palette,
    tooltip: { trigger: "axis", valueFormatter: (value: number) => ug(value) },
    grid: { containLabel: true, left: 8, right: 16, top: 24, bottom: 8 },
    xAxis: { type: "category", data: daily.map((item) => day(item.day, DAY_SHORT)), axisLabel: { hideOverlap: true } },
    yAxis: { type: "value", name: "µg/m³", max: roundMax },
    series: [
      {
        type: "line",
        name: "City-wide mean",
        showSymbol: false,
        areaStyle: { opacity: 0.15 },
        data: daily.map((item) => Math.round(item.mean * 10) / 10),
        markLine: guidelineLine("yAxis"),
      },
    ],
  };

  const over =
    story.daysOver === 0
      ? `The city-wide mean stayed under the WHO daily guideline of ${ug(GUIDELINE)} on every one of the ${story.days} days.`
      : `On ${story.daysOver} of ${story.days} days the city-wide mean was above the WHO daily guideline of ${ug(GUIDELINE)}.`;
  const change =
    story.change === null
      ? null
      : Math.abs(story.change) < 0.05
        ? "The last week was as clean as the first."
        : `The last week averaged ${ug(Math.abs(story.change))} ${story.change > 0 ? "more" : "less"} than the first.`;

  return (
    <Shell subtitle={`Fine particles (PM2.5) at ${story.stations} stations, ${day(story.from)} to ${day(story.to)}`}>
      <article className="app-story">
        <p className="app-lede">
          Across {story.stations} stations, fine particles averaged <strong>{ug(story.mean)}</strong>. {over}
        </p>

        <section aria-labelledby="where">
          <h2 id="where">Where it was worst</h2>
          <p>
            {story.worst.station} had the most, {ug(story.worst.mean)} on average
            {story.worst.daysOver > 0 ? `, and was over the guideline on ${story.worst.daysOver} ${story.worst.daysOver === 1 ? "day" : "days"}` : ""}.{" "}
            {story.cleanest.station} had the least, {ug(story.cleanest.mean)}.
          </p>
          <figure>
            <EntityMap rows={latestPerStation(rows)} location="location" label="stationName" color="pm25" height={320} />
            <figcaption>Each station, coloured by its latest reading.</figcaption>
          </figure>
          <figure>
            <ChartCard option={stationOption} height={56 * stations.length + 72} />
            <figcaption>Mean PM2.5 per station over the period; the dashed line is the WHO daily guideline.</figcaption>
          </figure>
          <details>
            <summary>The numbers behind the chart</summary>
            <table>
              <caption>Mean PM2.5 per station</caption>
              <thead>
                <tr>
                  <th scope="col">Station</th>
                  <th scope="col">Mean</th>
                  <th scope="col">Days over the guideline</th>
                </tr>
              </thead>
              <tbody>
                {stations.map((item) => (
                  <tr key={item.station}>
                    <th scope="row">{item.station}</th>
                    <td>{ug(item.mean)}</td>
                    <td>{item.daysOver}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </details>
        </section>

        <section aria-labelledby="when">
          <h2 id="when">How it changed</h2>
          <p>
            The worst day was {day(peak.day)}, when the city-wide mean reached {ug(peak.mean)}. {change}
          </p>
          <figure>
            <ChartCard option={dailyOption} height={300} />
            <figcaption>The city-wide mean, day by day; the dashed line is the WHO daily guideline.</figcaption>
          </figure>
          <details>
            <summary>The numbers behind the chart</summary>
            <table>
              <caption>City-wide mean PM2.5 per day</caption>
              <thead>
                <tr>
                  <th scope="col">Day</th>
                  <th scope="col">Mean</th>
                </tr>
              </thead>
              <tbody>
                {daily.map((item) => (
                  <tr key={item.day}>
                    <th scope="row">{day(item.day)}</th>
                    <td>{ug(item.mean)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </details>
        </section>

        <section aria-labelledby="about">
          <h2 id="about">About these numbers</h2>
          <p>
            Daily PM2.5 readings from the city's air quality stations, read through this application's endpoint. A day's
            city-wide figure is the mean of the stations that reported that day; a reading without a station, a date or a
            value is left out, not guessed. The guideline is the World Health Organization's 2021 air quality guideline for
            PM2.5 over 24 hours. The newest reading is from {day(story.to)}.
          </p>
        </section>
      </article>
    </Shell>
  );
}

function Shell({ subtitle, children }: { subtitle: string; children: React.ReactNode }): React.JSX.Element {
  return (
    <div className="jc-shell">
      <main className="jc-main">
        <Page label="How clean was the air">
          <Header level={1} title="How clean was the air?" subtitle={subtitle} />
          {children}
        </Page>
      </main>
    </div>
  );
}

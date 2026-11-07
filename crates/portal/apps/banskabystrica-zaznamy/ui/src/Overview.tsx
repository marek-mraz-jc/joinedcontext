/**
 * The records as charts, one cube at a time (T-2966, AP-56, AP-123): the cubes this body's
 * pipelines publish as a row of buttons, and the picked one's indicators as cards, each named,
 * coloured from the design tokens' chart palette and drawn over its periods or territories.
 *
 * The records come from the same narrowed source the table reads, filtered by the endpoint's own
 * `q`, so the charts never read a space the table cannot.
 */
import { useEffect, useMemo, useState, type CSSProperties } from "react";
import { rechartsPalette } from "@joinedcontext/sdk";
import type { EntitySource } from "@joinedcontext/sdk";
import { BODY_DATASETS } from "./labels";
import type { Body, Strings } from "./locales";
import { chartsOf, datasetName, type Chart } from "./series";
import { SeriesChart } from "./SeriesChart";

/** A page as large as the broker answers; a cube larger than `PAGES` pages is drawn from those. */
const LIMIT = 1000;
// ponytail: 20 pages (20 000 records) per cube at most; raise PAGES when a cube outgrows it.
const PAGES = 20;

/** Tables a body publishes that are not a statistics office cube, so not in `labels.ts`. */
const OWN_DATASETS: Record<Body, string[]> = {
  // The city's residents by age, from its own register (pipeline-obyvatelia).
  banskabystrica: ["mesto-obyvatelia-vek"],
  bbsk: [],
};

type Load = { state: "loading" } | { state: "failed" } | { state: "ready"; charts: Chart[] };

async function recordsOf(source: EntitySource, dataSet: string): Promise<Record<string, unknown>[]> {
  const rows: Record<string, unknown>[] = [];
  for (let page = 0; page < PAGES; page++) {
    const answer = await source.query(
      { type: "KeyPerformanceIndicator", q: `dataSet==${JSON.stringify(dataSet)}` },
      { offset: page * LIMIT, limit: LIMIT },
    );
    rows.push(...answer.rows.map((row) => row.raw));
    if (answer.rows.length < LIMIT) break;
  }
  return rows;
}

export function Overview({ body, source, s }: { body: Body; source: EntitySource; s: Strings }) {
  const language = s.locale.startsWith("sk") ? "sk" : "en";
  const datasets = useMemo(() => [...(BODY_DATASETS[body] ?? []), ...OWN_DATASETS[body]], [body]);
  const [picked, setPicked] = useState(datasets[0] ?? "");
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const palette = useMemo(() => rechartsPalette(), []);

  useEffect(() => {
    if (!picked) return;
    let current = true;
    setLoad({ state: "loading" });
    recordsOf(source, picked)
      .then((records) => {
        if (!current) return;
        const charts = chartsOf(records, picked, language, {
          keyAxis: s.keyAxis[picked],
          latest: s.latestOf,
          codeNames: s.codeNames[picked],
        });
        setLoad({ state: "ready", charts });
      })
      .catch(() => current && setLoad({ state: "failed" }));
    return () => {
      current = false;
    };
  }, [source, picked, language, s]);

  if (datasets.length === 0) return null;
  return (
    <section className="overview" aria-labelledby="overview-title">
      <h2 id="overview-title">{s.overview}</h2>
      <div className="datasets" role="group" aria-label={s.pickDataset}>
        {datasets.map((dataSet, i) => (
          <button
            key={dataSet}
            type="button"
            className="dataset"
            style={{ "--series": palette[i % palette.length] } as CSSProperties}
            aria-pressed={dataSet === picked}
            onClick={() => setPicked(dataSet)}
          >
            {datasetName(dataSet, language, s.datasetNames)}
          </button>
        ))}
      </div>
      {load.state === "loading" ? <p role="status">{s.chartsLoading}</p> : null}
      {load.state === "failed" ? <p role="alert">{s.chartsFailed}</p> : null}
      {load.state === "ready" && load.charts.length === 0 ? <p>{s.chartsEmpty}</p> : null}
      {load.state === "ready" && load.charts.length > 0 ? (
        <div className="series-grid">
          {load.charts.map((chart, i) => (
            <SeriesChart key={chart.id} chart={chart} colour={palette[i % palette.length]} words={s.chart} />
          ))}
        </div>
      ) : null}
    </section>
  );
}

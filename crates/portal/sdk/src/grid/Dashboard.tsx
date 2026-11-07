import React, { useCallback, useMemo, useState } from "react";

import { EntityGrid, type EntityGridProps } from "./EntityGrid";
import type { ResolvedGridConfig } from "./config";
import { chartBars, filtersQ, kpiValue, toneOf, type DashboardConfig } from "./dashboard";
import { andQ } from "./filters";
import type { RichRow } from "./model";

export interface DashboardProps extends Omit<EntityGridProps, "config" | "rowTone" | "onRows"> {
  /** The App's `src/dashboard.json` as `parseDashboardConfig` resolved it (T-6402). */
  config: DashboardConfig & { grid: ResolvedGridConfig };
  /** The words around the numbers; English defaults. */
  words?: { tiles?: string; chart?: string; empty?: string };
}

/**
 * A dashboard from its config (T-6402): the KPI tiles over the rows shown, the grid with the
 * colour rule as each row's tone (and the map the grid config asks for), and one bar chart. The
 * filters are the Endpoint's `q`, so the tiles and the chart count what the grid shows.
 */
export function Dashboard({ config, words, locale, ...grid }: DashboardProps): React.JSX.Element {
  const [rows, setRows] = useState<RichRow[]>([]);
  const onRows = useCallback((page: RichRow[]) => setRows(page), []);
  const gridConfig = useMemo<ResolvedGridConfig>(() => {
    const q = andQ(config.grid.filters.preset?.q, filtersQ(config.filters));
    return { ...config.grid, filters: { ...config.grid.filters, preset: { ...config.grid.filters.preset, q } } };
  }, [config.grid, config.filters]);
  const rowTone = useCallback((row: RichRow) => toneOf(config.colour, row), [config.colour]);
  const number = (value: number | null) => (value === null ? "–" : value.toLocaleString(locale, { maximumFractionDigits: 1 }));
  const bars = config.chart ? chartBars(config.chart, rows) : [];
  const most = Math.max(1, ...bars.map((bar) => Math.abs(bar.y)));
  return (
    <div className="jc-dashboard">
      {config.title ? <h2 className="jc-dashboard-title">{config.title}</h2> : null}
      {config.kpis && config.kpis.length > 0 ? (
        <ul className="jc-dashboard-tiles" aria-label={words?.tiles ?? "Key figures"}>
          {config.kpis.map((kpi) => (
            <li key={kpi.label} className="jc-dashboard-tile">
              <span className="jc-dashboard-tile-value">{number(kpiValue(kpi, rows))}</span>
              <span className="jc-dashboard-tile-label">{kpi.label}</span>
            </li>
          ))}
        </ul>
      ) : null}
      {config.colour ? (
        <ul className="jc-dashboard-legend" aria-label={config.colour.attr}>
          {config.colour.bands.map((band) => (
            <li key={band.label} className={`jc-grid-tr--${band.tone}`}>
              <span className="jc-grid-tone" aria-hidden="true" />
              {band.label}
            </li>
          ))}
        </ul>
      ) : null}
      <EntityGrid {...grid} locale={locale} config={gridConfig} rowTone={rowTone} onRows={onRows} />
      {config.chart ? (
        <figure className="jc-dashboard-chart">
          <figcaption>{words?.chart ?? `${config.chart.agg} of ${config.chart.y} by ${config.chart.x}`}</figcaption>
          {bars.length === 0 ? (
            <p>{words?.empty ?? "Nothing to chart for the rows shown."}</p>
          ) : (
            <dl>
              {bars.map((bar) => (
                <div key={bar.x} className="jc-dashboard-bar">
                  <dt>{bar.x}</dt>
                  <dd>
                    <span style={{ inlineSize: `${(Math.abs(bar.y) / most) * 100}%` }} aria-hidden="true" />
                    {number(bar.y)}
                  </dd>
                </div>
              ))}
            </dl>
          )}
        </figure>
      ) : null}
    </div>
  );
}

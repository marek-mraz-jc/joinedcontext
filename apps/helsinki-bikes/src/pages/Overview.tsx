import { useMemo } from "react";
import { Card, currentTokens, extent, Grid, Page, Split, useEntities } from "@joinedcontext/sdk";
import { histogramOption, rankedOption } from "../charts";
import { ChartCard } from "../components/ChartCard";
import { EntityMap } from "../components/EntityMap";
import { Problem, useEntitySelection } from "@joinedcontext/sdk";
import { SHARE, STATION, totals, withShare } from "../stations";

const TILES = [
  { key: "stations", label: "Stations", tone: "accent" },
  { key: "bikes", label: "Bikes available", tone: "success" },
  { key: "empty", label: "Stations empty", tone: "danger" },
] as const;

/**
 * The city at a glance (T-2924): the three numbers in colour, every station on the map coloured by
 * the share of its docks holding a bike, how the stations spread over bikes available, and the ten
 * fullest and emptiest.
 */
export function Overview() {
  const { selected, select } = useEntitySelection();
  const { rows, loading, error, reload } = useEntities(STATION);
  const sum = totals(rows);
  const shaded = useMemo(() => withShare(rows), [rows]);
  const range = useMemo(() => extent(shaded, SHARE), [shaded]);
  const histogram = useMemo(() => histogramOption(rows), [rows]);
  const fullest = useMemo(() => rankedOption(rows, "fullest"), [rows]);
  const emptiest = useMemo(() => rankedOption(rows, "emptiest"), [rows]);
  const number = new Intl.NumberFormat();
  const tokens = currentTokens();

  return (
    <Page label="Overview">
      <Problem error={error} onRetry={reload} />
      <section className="jc-tiles">
        {TILES.map((tile) => (
          <div className="jc-tile app-tile" data-tone={tile.tone} key={tile.key}>
            <span className="jc-tile-label">{tile.label}</span>
            <strong className="jc-tile-value">{loading ? "…" : number.format(sum[tile.key])}</strong>
          </div>
        ))}
      </section>
      <Split ratio="2:1">
        <Card title="Stations now">
          <EntityMap rows={shaded} location="location" label="name" color={SHARE} height={420} selected={selected?.id ?? null} onSelect={(row) => select({ id: row.id, type: STATION })} />
          <div className="app-legend" aria-label="Map colour: share of docks with a bike">
            <span>{range ? `${range[0]} %` : "–"}</span>
            <span
              className="app-legend-ramp"
              aria-hidden="true"
              style={{ background: `linear-gradient(to right, ${tokens.map.low}, ${tokens.map.high})` }}
            />
            <span>{range ? `${range[1]} %` : "–"}</span>
            <small>of docks with a bike</small>
          </div>
        </Card>
        <ChartCard title="Stations by bikes available" option={histogram} loading={loading && rows.length === 0} error={error} />
      </Split>
      <Grid columns={2}>
        <ChartCard title="Fullest stations" option={fullest} height={320} loading={loading && rows.length === 0} error={error} empty="No station reports its docks yet." />
        <ChartCard title="Emptiest stations" option={emptiest} height={320} loading={loading && rows.length === 0} error={error} empty="No station reports its docks yet." />
      </Grid>
    </Page>
  );
}

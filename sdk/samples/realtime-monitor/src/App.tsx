import { useEffect, useRef, useState } from "react";
import { Card, format, Grid, Header, Page, Split, useClient, useEntities } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { Empty, Loading, Problem } from "./components/states";
import { ALERT_LEVEL, crossings, KEEP, merge, record, seed, silentFor, sparkline, type Alert, type History, type Point } from "./monitor";

const TYPE = "AirQualityObserved";
const ATTR = "pm25";
const REFRESH_MS = 10_000;
const TIME = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });
const SPARK_W = 160;
const SPARK_H = 36;

function ug(value: number): string {
  return `${format(Math.round(value * 10) / 10, "number")} µg/m³`;
}

function time(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? "—" : TIME.format(date);
}

/** A sparkline a screen reader hears as its range and span, not as a picture. */
function Spark({ points }: { points: Point[] }): React.JSX.Element {
  if (points.length < 2) return <p className="app-quiet">Collecting readings…</p>;
  const values = points.map((point) => point.value);
  const low = Math.min(...values);
  const high = Math.max(...values);
  const label = `PM2.5, ${points.length} readings from ${time(points[0].at)} to ${time(points[points.length - 1].at)}: lowest ${ug(low)}, highest ${ug(high)}`;
  // The alert level as a line, when the sparkline's range reaches it.
  const levelY = low <= ALERT_LEVEL && ALERT_LEVEL <= high ? SPARK_H - ((ALERT_LEVEL - low) / (high - low || 1)) * SPARK_H : null;
  return (
    <svg className="app-spark" viewBox={`0 0 ${SPARK_W} ${SPARK_H}`} preserveAspectRatio="none" role="img" aria-label={label}>
      {levelY !== null && <line className="app-spark-level" x1={0} x2={SPARK_W} y1={levelY} y2={levelY} />}
      <polyline points={sparkline(points, SPARK_W, SPARK_H)} />
    </svg>
  );
}

function Station({ row, points, now }: { row: Row; points: Point[]; now: Date }): React.JSX.Element {
  const name = typeof row.stationName === "string" ? row.stationName : row.id;
  const value = typeof row[ATTR] === "number" ? (row[ATTR] as number) : null;
  const silent = silentFor(row, now);
  const state = silent !== null ? "silent" : value === null ? "unknown" : value > ALERT_LEVEL ? "over" : "normal";
  return (
    <Card title={name} label={name}>
      <p className="app-value" data-state={state}>
        {value === null ? "—" : ug(value)}
      </p>
      <p className="app-state" data-state={state}>
        {state === "silent" ? (
          <>
            <span aria-hidden="true">◌ </span>No reading for {silent} min
          </>
        ) : state === "over" ? (
          <>
            <span aria-hidden="true">▲ </span>Over the alert level
          </>
        ) : state === "unknown" ? (
          "No PM2.5 value"
        ) : (
          <>
            <span aria-hidden="true">● </span>Normal
          </>
        )}
      </p>
      <Spark points={points} />
      <p className="app-quiet">
        {typeof row.no2 === "number" ? `NO₂ ${ug(row.no2)} · ` : ""}read at {time(String(row.dateObserved ?? ""))}
      </p>
    </Card>
  );
}

/**
 * Every station's latest reading, its last hour as a sparkline and the crossings of the alert
 * level as they happen; the person can pause the updates and read at their own pace.
 */
export default function App(): React.JSX.Element {
  const client = useClient();
  const [live, setLive] = useState(true);
  const { rows, loading, error, reload } = useEntities(TYPE, undefined, { refreshMs: live ? REFRESH_MS : undefined });
  const [history, setHistory] = useState<History>({});
  const [historyProblem, setHistoryProblem] = useState<string | null>(null);
  const [alerts, setAlerts] = useState<Alert[]>([]);
  const [updated, setUpdated] = useState<Date | null>(null);
  const before = useRef<Row[] | null>(null);

  // The last hour once, so a sparkline has a shape before the first poll adds to it.
  useEffect(() => {
    let current = true;
    client.temporal
      .list(TYPE, { attrs: [ATTR], timerel: "after", timeAt: new Date(Date.now() - 60 * 60 * 1000).toISOString(), lastN: KEEP })
      .then((found) => current && setHistory((live) => merge(seed(found, ATTR), live)))
      .catch((err: unknown) => current && setHistoryProblem(err instanceof Error ? err.message : String(err)));
    return () => {
      current = false;
    };
  }, [client]);

  useEffect(() => {
    if (loading || error) return;
    setHistory((past) => record(past, rows, ATTR));
    const crossed = crossings(before.current, rows, ATTR);
    if (crossed.length > 0) setAlerts((past) => [...crossed, ...past].slice(0, 50));
    before.current = rows;
    setUpdated(new Date());
  }, [rows, loading, error]);

  const now = new Date();
  const ordered = [...rows].sort((a, b) => String(a.stationName ?? "").localeCompare(String(b.stationName ?? "")));
  const newest = alerts[0];

  const actions = (
    <>
      <span className="app-quiet">
        {updated ? `Updated ${TIME.format(updated)}` : ""}
      </span>
      <button type="button" aria-pressed={!live} onClick={() => setLive((on) => !on)}>
        {live ? "Pause updates" : "Resume updates"}
      </button>
      <button type="button" onClick={reload}>
        Refresh now
      </button>
    </>
  );

  return (
    <div className="jc-shell">
      <main className="jc-main">
        <Page label="Air quality now">
          <Header level={1} title="Air quality now" subtitle={`PM2.5 at every station, updated every ${REFRESH_MS / 1000} seconds; alerts above ${ug(ALERT_LEVEL)}`} actions={actions} />
          <div className="app-alert-slot" role="alert">
            {newest && (
              <p className="app-alert">
                <span aria-hidden="true">▲ </span>
                {newest.station} crossed the alert level: {ug(newest.value)} at {time(newest.at)}.
              </p>
            )}
          </div>
          <Problem error={error} onRetry={reload} />
          {historyProblem && <p className="app-quiet">The last hour could not be read ({historyProblem}); the sparklines start now.</p>}
          {loading && rows.length === 0 ? (
            <Loading label="Loading the stations…" />
          ) : rows.length === 0 ? (
            !error && <Empty>No station reports yet.</Empty>
          ) : (
            <Split ratio="2:1">
              <Grid columns={3}>
                {ordered.map((row) => (
                  <Station key={row.id} row={row} points={history[row.id] ?? []} now={now} />
                ))}
              </Grid>
              <Card title="Alerts since you opened this page" label="Alerts since you opened this page">
                {alerts.length === 0 ? (
                  <p className="app-quiet">None yet. A station crossing {ug(ALERT_LEVEL)} is listed here and read out.</p>
                ) : (
                  <ol className="app-log">
                    {alerts.map((alert) => (
                      <li key={alert.id}>
                        <time dateTime={alert.at}>{time(alert.at)}</time> {alert.station}, {ug(alert.value)}
                      </li>
                    ))}
                  </ol>
                )}
              </Card>
            </Split>
          )}
        </Page>
      </main>
    </div>
  );
}

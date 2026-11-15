import { useEffect, useMemo, useState } from "react";
import { Card, currentTokens, Empty, Loading, Page, Problem, ProblemError, Split, useEntities, useEntitySelection } from "@joinedcontext/sdk";
import { ChartCard } from "../components/ChartCard";
import { useAccess } from "../access";
import { SavedPlans } from "../components/SavedPlans";
import { MapView } from "../components/MapView";
import type { MapPoint } from "../components/MapView";
import { localeOf, number, useLang, ZONE } from "../i18n";
import type { Lang } from "../i18n";
import { computePlan } from "../planner";
import type { Level, Need, Plan, Station } from "../planner";
import { apiBase, plansApi } from "../plans";
import type { PlansApi, SavedPlan } from "../plans";
import { ATTRS, levelColour, newest, STATION, stationsOf } from "../stations";
import { t } from "../texts";
import { listOf, useParam } from "../url";

/** The counts change every few minutes; the page reads them again each minute. */
const REFRESH_MS = 60_000;
/** The van sizes an operator may enter. */
const VAN_MIN = 1;
const VAN_MAX = 200;
const DEFAULT_VAN = "20";
/** How many stations the table lists, most out of balance first. */
const WATCHED = 25;
/** The fill bands of the chart, in tenths. */
const BANDS = 10;

/** The van capacity in the address, when it is a whole number in range; the default else. */
export function vanOf(value: string): number {
  const parsed = Number(value);
  return Number.isInteger(parsed) && parsed >= VAN_MIN && parsed <= VAN_MAX ? parsed : Number(DEFAULT_VAN);
}

/** How many stations fall in each tenth of fill, the unknown ones aside. */
export function fillBands(needs: Need[]): number[] {
  const bands = Array.from({ length: BANDS }, () => 0);
  for (const need of needs) {
    if (need.fill === null) continue;
    bands[Math.min(BANDS - 1, Math.floor(need.fill * BANDS))] += 1;
  }
  return bands;
}

/** The stations most out of balance, the ones the operator touched first. */
export function watched(needs: Need[], touched: Set<string>, limit = WATCHED): Need[] {
  return [...needs]
    .filter((need) => need.level !== "unknown")
    .sort(
      (a, b) =>
        Number(touched.has(b.id)) - Number(touched.has(a.id)) || Math.abs(b.surplus) - Math.abs(a.surplus) || a.id.localeCompare(b.id),
    )
    .slice(0, limit);
}

/**
 * What a station's choice button does, and so what it says: a station left out goes back; one added,
 * on the route, or about to run empty or full (which the planner routes unless it is left out)
 * is left out; any other is added.
 */
export function choiceOf(
  id: string,
  critical: boolean,
  { add, skip, route }: { add: string[]; skip: string[]; route: Set<string> },
): "putBack" | "leaveOut" | "addIn" {
  if (skip.includes(id)) return "putBack";
  if (add.includes(id) || route.has(id) || critical) return "leaveOut";
  return "addIn";
}

function fillChart(needs: Need[], lang: Lang): Record<string, unknown> | null {
  const bands = fillBands(needs);
  if (bands.every((count) => count === 0)) return null;
  const tokens = currentTokens();
  const colourOf = (index: number): string =>
    index === 0 ? tokens.color.danger : index === BANDS - 1 ? tokens.color.accent : index < 2 ? tokens.color.warning : index > 7 ? tokens.map.high : tokens.color.success;
  return {
    tooltip: { trigger: "axis", axisPointer: { type: "shadow" } },
    grid: { containLabel: true, left: 8, right: 16, top: 24, bottom: 8 },
    xAxis: { type: "category", data: bands.map((_, i) => `${i * 10}–${(i + 1) * 10} %`) },
    yAxis: { type: "value", name: t(lang, "stationsAxis"), minInterval: 1 },
    series: [{ type: "bar", name: t(lang, "stationsAxis"), data: bands.map((value, i) => ({ value, itemStyle: { color: colourOf(i) } })) }],
  };
}

function unreadable(error: Error, lang: Lang): string {
  return error instanceof ProblemError && error.status > 0 ? t(lang, "unreadable", { status: error.status }) : t(lang, "offline");
}

/**
 * The operator's question, answered on opening (T-3328): which stations are about to run empty
 * or full, and the van's route that moves bikes from the one to the other. The plan is computed
 * by the WebAssembly planner in a worker, again whenever the counts change or the operator takes
 * a station in or out of the route; every choice is kept in the address. A station's name, in the
 * route or the table, and a station on the map open it in the SDK's entity panel (SDK-40); the
 * App writes nothing, so the panel links it to the Portal (README).
 */
export function Rebalance({ plans }: { plans?: PlansApi } = {}) {
  const lang = useLang();
  const api = useMemo(() => plans ?? plansApi(apiBase()), [plans]);
  const query = useMemo(() => ({ attrs: ATTRS }), []);
  const { rows, loading, error, reload } = useEntities(STATION, query, { refreshMs: REFRESH_MS });
  const [vanText, setVanText] = useParam("van", DEFAULT_VAN);
  const [start, setStart] = useParam("start");
  const [addText, setAdd] = useParam("add");
  const [skipText, setSkip] = useParam("skip");
  const add = useMemo(() => listOf(addText), [addText]);
  const skip = useMemo(() => listOf(skipText), [skipText]);
  const van = vanOf(vanText);

  const stations = useMemo(() => stationsOf(rows), [rows]);
  const byId = useMemo(() => new Map(stations.map((s) => [s.id, s])), [stations]);
  const startAt = byId.get(start);
  const [plan, setPlan] = useState<Plan | null>(null);
  const [planning, setPlanning] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  const { select } = useEntitySelection();
  const { levels } = useAccess();
  const open = (id: string) => select({ id, type: STATION });

  useEffect(() => {
    if (stations.length === 0) {
      setPlan(null);
      return;
    }
    let current = true;
    setPlanning(true);
    const settings = {
      ...levels,
      vanCapacity: van,
      include: add,
      exclude: skip,
      start: startAt && startAt.lon !== null && startAt.lat !== null ? ([startAt.lon, startAt.lat] as [number, number]) : null,
    };
    computePlan({ stations, settings })
      .then((answer) => {
        if (!current) return;
        setPlan(answer);
        setFailed(null);
      })
      .catch((why: unknown) => {
        if (current) setFailed(why instanceof Error ? why.message : String(why));
      })
      .finally(() => {
        if (current) setPlanning(false);
      });
    return () => {
      current = false;
    };
  }, [stations, van, add, skip, startAt, levels]);

  // Read once: the tokens are set when the app starts and never change under it.
  const tokens = useMemo(() => currentTokens(), []);
  const needs = useMemo(() => plan?.needs ?? [], [plan]);
  const needOf = useMemo(() => new Map(needs.map((n) => [n.id, n])), [needs]);
  const count = (levels: Level[]) => needs.filter((n) => levels.includes(n.level)).length;
  const route = plan?.route;
  const order = useMemo(() => new Map((route?.stops ?? []).map((stop, i) => [stop.id, i + 1])), [route]);
  const touched = useMemo(() => new Set([...add, ...skip]), [add, skip]);
  const waiting = loading && rows.length === 0;
  const updated = newest(rows);

  const critical = (id: string) => ["empty", "low", "high", "full"].includes(needOf.get(id)?.level ?? "unknown");
  const actionKey = (id: string) => choiceOf(id, critical(id), { add, skip, route: new Set(order.keys()) });
  // The button says what its click does: both read `choiceOf`.
  const toggle = (id: string) => {
    const action = actionKey(id);
    if (action === "putBack") setSkip(skip.filter((x) => x !== id).join(","));
    else if (add.includes(id)) setAdd(add.filter((x) => x !== id).join(","));
    else if (action === "leaveOut") setSkip([...skip, id].join(","));
    else setAdd([...add, id].join(","));
  };
  const nowOf = (id: string): string => (skip.includes(id) ? t(lang, "excluded") : add.includes(id) ? t(lang, "included") : t(lang, "auto"));
  const actionOf = (id: string): string => t(lang, actionKey(id));

  const points: MapPoint[] = useMemo(
    () =>
      stations
        .filter((s): s is Station & { lon: number; lat: number } => s.lon !== null && s.lat !== null)
        .map((s) => {
          const need = needOf.get(s.id);
          const level = need?.level ?? "unknown";
          const stop = order.get(s.id);
          return {
            id: s.id,
            at: [s.lon, s.lat] as [number, number],
            color: levelColour(level, tokens),
            lines: [
              stop ? `${stop}. ${s.name}` : s.name,
              s.bikes !== null && s.capacity !== null ? `${s.bikes} / ${s.capacity}` : t(lang, "unknown"),
              t(lang, level),
            ],
          };
        }),
    [stations, needOf, order, tokens, lang],
  );
  const line = useMemo(() => {
    const stops = route?.stops ?? [];
    const first = startAt && startAt.lon !== null && startAt.lat !== null ? [[startAt.lon, startAt.lat] as [number, number]] : [];
    return [...first, ...stops.map((stop) => stop.at)];
  }, [route, startAt]);
  const chart = useMemo(() => fillChart(needs, lang), [needs, lang]);
  const list = useMemo(() => watched(needs, touched), [needs, touched]);

  // A station on the map goes into or out of the route, as it always did, and opens in the panel.
  const pick = (id: string) => {
    toggle(id);
    open(id);
  };

  const reset = () => {
    setVanText(DEFAULT_VAN);
    setStart("");
    setAdd("");
    setSkip("");
  };
  const reopen = (saved: SavedPlan) => {
    setVanText(String(saved.vanCapacity));
    setStart(saved.start ?? "");
    setAdd(saved.include.join(","));
    setSkip(saved.exclude.join(","));
  };
  const current = useMemo(() => ({ vanCapacity: van, start: startAt ? start : null, include: add, exclude: skip }), [van, startAt, start, add, skip]);

  return (
    <Page label={t(lang, "page")}>
      {error && <Problem error={new Error(unreadable(error, lang))} onRetry={reload} />}
      {failed && <Problem error={new Error(t(lang, "plannerFailed", { why: failed }))} />}
      <ul className="app-summary" aria-label={t(lang, "page")}>
        <li>
          <span className="app-summary-label">{t(lang, "soonEmpty")}</span>
          <strong style={{ color: tokens.color.danger }}>{waiting ? "–" : number(count(["empty", "low"]))}</strong>
        </li>
        <li>
          <span className="app-summary-label">{t(lang, "soonFull")}</span>
          <strong style={{ color: tokens.color.accent }}>{waiting ? "–" : number(count(["full", "high"]))}</strong>
        </li>
        <li>
          <span className="app-summary-label">{t(lang, "moved")}</span>
          <strong>{route ? number(route.moved) : "–"}</strong>
          {route && (
            <span className="app-summary-label">
              {t(lang, "bikesOver", { km: number(route.km, 1), stops: number(route.stops.length) })}
            </span>
          )}
        </li>
      </ul>
      {updated && (
        <p className="app-updated">
          {t(lang, "updated", {
            at: updated.toLocaleString(localeOf(lang), { dateStyle: "medium", timeStyle: "short", timeZone: ZONE }),
          })}
        </p>
      )}
      <form className="app-filters" aria-label={t(lang, "route")} onSubmit={(event) => event.preventDefault()}>
        <label>
          <span>{t(lang, "van")}</span>
          <input
            type="number"
            inputMode="numeric"
            min={VAN_MIN}
            max={VAN_MAX}
            step={1}
            value={vanText}
            onChange={(event) => setVanText(event.target.value)}
          />
        </label>
        <label>
          <span>{t(lang, "start")}</span>
          <select value={startAt ? start : ""} onChange={(event) => setStart(event.target.value)}>
            <option value="">{t(lang, "startAuto")}</option>
            {[...stations]
              .filter((s) => s.lon !== null)
              .sort((a, b) => a.name.localeCompare(b.name, localeOf(lang)))
              .map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name}
                </option>
              ))}
          </select>
        </label>
        <button type="button" className="jc-button" onClick={reset}>
          {t(lang, "reset")}
        </button>
      </form>
      <Split ratio="1:1">
        <Card title={t(lang, "map")}>
          <MapView points={points} line={line} label={t(lang, "mapLabel")} onPick={pick} />
          <ul className="app-legend" aria-label={t(lang, "state")}>
            {(["empty", "low", "balanced", "high", "full", "unknown"] as Level[]).map((level) => (
              <li key={level}>
                <span className="app-swatch" aria-hidden="true" style={{ background: levelColour(level, tokens) }} />
                {t(lang, level)}
              </li>
            ))}
          </ul>
        </Card>
        <Card title={t(lang, "route")}>
          {waiting ? (
            <Loading label={t(lang, "reading")} />
          ) : rows.length === 0 && !error ? (
            <Empty>{t(lang, "noStations")}</Empty>
          ) : !route ? (
            <Loading label={t(lang, "planning")} />
          ) : route.stops.length === 0 ? (
            <Empty>{t(lang, "routeEmpty")}</Empty>
          ) : (
            <ol className="app-stops" aria-label={t(lang, "route")} aria-busy={planning}>
              {route.stops.map((stop) => (
                <li key={stop.id} className="app-stop" data-action={stop.action}>
                  <div>
                    <button type="button" className="app-open" onClick={() => open(stop.id)}>
                      <strong>{stop.name}</strong>
                    </button>
                    <p>
                      <span className="app-chip" data-action={stop.action}>
                        {t(lang, stop.action, { n: number(stop.bikes) })}
                      </span>{" "}
                      {t(lang, "load", { n: number(stop.load) })} · {t(lang, "leg", { km: number(stop.legKm, 1) })}
                    </p>
                  </div>
                  <button type="button" className="jc-button" onClick={() => toggle(stop.id)} aria-label={`${t(lang, "leaveOut")}: ${stop.name}`}>
                    {t(lang, "leaveOut")}
                  </button>
                </li>
              ))}
            </ol>
          )}
        </Card>
      </Split>
      <Split ratio="2:1">
        <Card title={t(lang, "watch")}>
          {list.length === 0 ? (
            <Empty>{waiting ? t(lang, "reading") : t(lang, "noStations")}</Empty>
          ) : (
            <div className="jc-table-wrap">
              <table className="jc-table">
                <caption className="app-sr">{t(lang, "watch")}</caption>
                <thead>
                  <tr>
                    <th scope="col">{t(lang, "station")}</th>
                    <th scope="col" className="jc-num">
                      {t(lang, "bikes")}
                    </th>
                    <th scope="col" className="jc-num">
                      {t(lang, "fill")}
                    </th>
                    <th scope="col">{t(lang, "state")}</th>
                    <th scope="col">{t(lang, "choice")}</th>
                  </tr>
                </thead>
                <tbody>
                  {list.map((need) => {
                    const s = byId.get(need.id);
                    if (!s) return null;
                    return (
                      <tr key={need.id}>
                        <th scope="row">
                          {order.has(need.id) ? `${order.get(need.id)}. ` : ""}
                          <button type="button" className="app-open" onClick={() => open(need.id)}>
                            {s.name}
                          </button>
                        </th>
                        <td className="jc-num">
                          {s.bikes ?? "–"} / {s.capacity ?? "–"}
                        </td>
                        <td className="jc-num">{need.fill === null ? "–" : `${number(need.fill * 100)} %`}</td>
                        <td>
                          <span className="app-tag">
                            <span className="app-swatch" aria-hidden="true" style={{ background: levelColour(need.level, tokens) }} />
                            {t(lang, need.level)}
                          </span>
                        </td>
                        <td>
                          <button type="button" className="jc-button app-choice" onClick={() => toggle(need.id)} aria-label={`${actionOf(need.id)}: ${s.name}`}>
                            {actionOf(need.id)}
                          </button>
                          <span className="app-choice-now">{nowOf(need.id)}</span>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </Card>
        <ChartCard
          title={t(lang, "fillChart")}
          option={error ? null : chart}
          loading={waiting}
          loadingLabel={t(lang, "reading")}
          empty={t(lang, "noStations")}
        />
      </Split>
      <SavedPlans lang={lang} api={api} current={current} onOpen={reopen} />
    </Page>
  );
}

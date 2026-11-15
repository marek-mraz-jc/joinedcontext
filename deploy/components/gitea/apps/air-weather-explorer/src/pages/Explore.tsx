import { useEffect, useMemo, useState } from "react";
import { Card, currentTokens, Empty, Loading, Page, ProblemError, Split, useEntities, useEntitySelection } from "@joinedcontext/sdk";
import { computeAnalysis, strength, strongest } from "../analysis";
import type { Analysis, Pair } from "../analysis";
import { ChartCard } from "../components/ChartCard";
import { FaultsCard } from "../components/FaultsCard";
import { Keep, problemText } from "../components/Keep";
import { MapView } from "../components/MapView";
import type { MapPoint } from "../components/MapView";
import { useFaults, withoutFaults } from "../faults";
import { localeOf, number, useLang, ZONE } from "../i18n";
import type { Lang } from "../i18n";
import { airApi, apiBase, isCode, ServerProblem } from "../server";
import type { AirApi, Series } from "../server";
import { AIR, localOf, nearest, PLACE_ATTRS, POLLUTANTS, stationsOf, VARIABLES, WEATHER } from "../stations";
import type { Pollutant, Variable } from "../stations";
import { t } from "../texts";
import { useParam } from "../url";

/** The periods a reader may compare over, in days; the data need's history window is 7 days. */
export const PERIODS = [1, 3, 7] as const;
/** The rolling windows a reader may smooth with, in hours. */
export const WINDOWS = [1, 3, 6, 12] as const;

/** A whole number of the allowed ones from the address, else the fallback. */
export function choiceOf<T extends number>(value: string, allowed: readonly T[], fallback: T): T {
  const parsed = Number(value) as T;
  return allowed.includes(parsed) ? parsed : fallback;
}

function isPollutant(value: string): value is Pollutant {
  return (POLLUTANTS as readonly string[]).includes(value);
}
function isVariable(value: string): value is Variable {
  return (VARIABLES as readonly string[]).includes(value);
}

/** The answer in one sentence: which way the pollutant moves with the weather, and how strongly. */
export function sentence(pair: Pair, lang: Lang): { claim: string; evidence: string } | null {
  if (pair.spearman === null) return null;
  const air = t(lang, pair.air as Pollutant);
  const weather = t(lang, pair.weather as Variable);
  const how = strength(pair.spearman);
  const claim =
    how === "none"
      ? t(lang, "none", { air, weather })
      : `${t(lang, pair.spearman > 0 ? "rises" : "falls", { air, weather })}, ${t(lang, how)}`;
  const evidence = t(lang, "evidence", {
    rho: number(pair.spearman, 2),
    r: pair.pearson === null ? "–" : number(pair.pearson, 2),
    n: number(pair.n),
  });
  return { claim: claim.charAt(0).toUpperCase() + claim.slice(1), evidence };
}

function heatmap(analysis: Analysis, lang: Lang): Record<string, unknown> | null {
  const cells = analysis.pairs.filter((pair) => pair.spearman !== null);
  if (cells.length === 0) return null;
  const tokens = currentTokens();
  const airs = analysis.air.map((s) => s.name);
  const weathers = analysis.weather.map((s) => s.name);
  return {
    tooltip: { position: "top" },
    grid: { containLabel: true, left: 8, right: 16, top: 16, bottom: 56 },
    xAxis: { type: "category", data: weathers.map((w) => t(lang, w as Variable)), axisLabel: { interval: 0, rotate: 20 } },
    yAxis: { type: "category", data: airs.map((a) => t(lang, a as Pollutant)) },
    visualMap: {
      min: -1,
      max: 1,
      calculable: false,
      orient: "horizontal",
      left: "center",
      bottom: 0,
      inRange: { color: [tokens.color.accent, tokens.color.surface, tokens.color.danger] },
    },
    series: [
      {
        type: "heatmap",
        label: { show: true, formatter: (p: { value: [number, number, number] }) => number(p.value[2], 2) },
        data: cells.map((pair) => ({
          name: `${pair.air}|${pair.weather}`,
          value: [weathers.indexOf(pair.weather), airs.indexOf(pair.air), pair.spearman],
        })),
      },
    ],
  };
}

function timeChart(analysis: Analysis, air: string, weather: string, lang: Lang): Record<string, unknown> | null {
  const a = analysis.air.find((s) => s.name === air);
  const w = analysis.weather.find((s) => s.name === weather);
  if (!a || !w || analysis.hours.length === 0) return null;
  if (a.hourly.every((v) => v === null) && w.hourly.every((v) => v === null)) return null;
  const tokens = currentTokens();
  const labels = analysis.hours.map((h) =>
    new Date(h).toLocaleString(localeOf(lang), { day: "numeric", month: "numeric", hour: "2-digit", minute: "2-digit", timeZone: ZONE }),
  );
  const airName = t(lang, air as Pollutant);
  const weatherName = t(lang, weather as Variable);
  return {
    tooltip: { trigger: "axis" },
    legend: { top: 0 },
    grid: { containLabel: true, left: 8, right: 8, top: 40, bottom: 8 },
    xAxis: { type: "category", data: labels },
    // The legend names both lines; an axis name would be cut at a phone's width.
    yAxis: [
      { type: "value", scale: true },
      { type: "value", scale: true },
    ],
    series: [
      { type: "line", name: airName, showSymbol: false, connectNulls: false, itemStyle: { color: tokens.color.danger }, data: a.smooth },
      { type: "line", name: weatherName, yAxisIndex: 1, showSymbol: false, connectNulls: false, itemStyle: { color: tokens.color.accent }, data: w.smooth },
      {
        type: "scatter",
        name: t(lang, "outlier"),
        symbolSize: 10,
        itemStyle: { color: tokens.color.warning },
        data: a.outliers.map((i) => [labels[i], a.hourly[i]]),
      },
    ],
  };
}

function unreadable(error: Error, lang: Lang): string {
  return (error instanceof ProblemError || error instanceof ServerProblem) && error.status > 0 ? t(lang, "unreadable", { status: error.status }) : t(lang, "offline");
}

/**
 * The reader's question, answered on opening (T-3330): how does the weather move the air here?
 * The first air quality station is compared with the nearest weather station over three days:
 * the strongest relation in a sentence, every pollutant against every weather variable, and the
 * pair's hours on one chart with its outliers. The statistics run in the WebAssembly module in a
 * worker, again on every change of station, period or smoothing; every choice is in the address.
 * The hours a steward marked faulty are left out, and the marks are shown (T-3827).
 */
export function Explore({ server }: { server?: AirApi } = {}) {
  const lang = useLang();
  const api = useMemo(() => server ?? airApi(apiBase()), [server]);
  const airRows = useEntities(AIR, useMemo(() => ({ attrs: PLACE_ATTRS }), []));
  const weatherRows = useEntities(WEATHER, useMemo(() => ({ attrs: PLACE_ATTRS }), []));
  const airStations = useMemo(() => stationsOf(airRows.rows), [airRows.rows]);
  const weatherStations = useMemo(() => stationsOf(weatherRows.rows), [weatherRows.rows]);

  const [stationText, setStation] = useParam("station");
  const [weatherText, setWeather] = useParam("weather");
  const [daysText, setDays] = useParam("days", "3");
  const [windowText, setWindow] = useParam("window", "3");
  const [airText, setAir] = useParam("air");
  const [variableText, setVariable] = useParam("w");
  const days = choiceOf(daysText, PERIODS, 3);
  const smoothing = choiceOf(windowText, WINDOWS, 3);
  const station = airStations.find((s) => s.local === stationText) ?? airStations[0];
  const near = nearest(station, weatherStations);
  const weather = weatherStations.find((s) => s.local === weatherText) ?? near?.station;
  const stationId = station?.id;
  const weatherId = weather?.id;

  // A saved comparison, `?compare=<code>`: its choices put into the address, then the code
  // dropped, so a reload keeps them and the reader's own changes stay theirs (T-3348).
  const [compareCode, setCompare] = useParam("compare");
  const [compareNote, setCompareNote] = useState<{ text: string; error: boolean } | null>(null);
  useEffect(() => {
    if (!compareCode) return;
    if (!isCode(compareCode)) {
      setCompareNote({ text: t(lang, "compareGone"), error: true });
      setCompare("");
      return;
    }
    let current = true;
    setCompareNote({ text: t(lang, "opening"), error: false });
    api
      .open(compareCode)
      .then((saved) => {
        if (!current) return;
        setStation(localOf(saved.station));
        setWeather(localOf(saved.weather));
        setDays(String(saved.days));
        setWindow(String(saved.smoothing));
        setAir(saved.air ?? "");
        setVariable(saved.variable ?? "");
        setCompareNote({ text: saved.name ? t(lang, "opened", { name: saved.name }) : t(lang, "openedUnnamed"), error: false });
      })
      .catch((error: unknown) => {
        if (current) setCompareNote({ text: problemText(lang, error, "openFailed"), error: true });
      })
      .finally(() => {
        if (current) setCompare("");
      });
    return () => {
      current = false;
    };
  }, [compareCode, api, lang, setCompare, setStation, setWeather, setDays, setWindow, setAir, setVariable]);

  // The two stations' hourly means, from the App's server, which keeps them (T-3348).
  const [history, setHistory] = useState<Series | null>(null);
  const [historyError, setHistoryError] = useState<Error | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    if (!stationId || !weatherId) return;
    let current = true;
    setHistory(null);
    api
      .series(stationId, weatherId, days)
      .then((series) => {
        if (!current) return;
        setHistory(series);
        setHistoryError(null);
      })
      .catch((error: unknown) => {
        if (current) setHistoryError(error instanceof Error ? error : new Error(String(error)));
      });
    return () => {
      current = false;
    };
  }, [api, stationId, weatherId, days, attempt]);

  // The air station's hours a steward marked faulty are left out of every statistic.
  const faults = useFaults(api, stationId);
  const cleaned = useMemo(() => (history ? withoutFaults(history.air, faults.list ?? []) : null), [history, faults.list]);

  const [analysis, setAnalysis] = useState<Analysis | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  useEffect(() => {
    if (!history || !cleaned) {
      setAnalysis(null);
      return;
    }
    let current = true;
    computeAnalysis({ air: cleaned.hourly, weather: history.weather, settings: { window: smoothing } })
      .then((result) => {
        if (!current) return;
        setAnalysis(result);
        setFailed(null);
      })
      .catch((why: unknown) => {
        if (current) setFailed(why instanceof Error ? why.message : String(why));
      });
    return () => {
      current = false;
    };
  }, [history, cleaned, smoothing]);

  const best = analysis ? strongest(analysis.pairs) : null;
  const air = isPollutant(airText) && analysis?.air.some((s) => s.name === airText) ? airText : (best?.air ?? analysis?.air[0]?.name ?? "");
  const variable =
    isVariable(variableText) && analysis?.weather.some((s) => s.name === variableText) ? variableText : (best?.weather ?? analysis?.weather[0]?.name ?? "");
  const shown = analysis?.pairs.find((pair) => pair.air === air && pair.weather === variable) ?? null;
  const said = shown ? sentence(shown, lang) : null;
  const matrix = useMemo(() => (analysis ? heatmap(analysis, lang) : null), [analysis, lang]);
  const series = useMemo(() => (analysis ? timeChart(analysis, air, variable, lang) : null), [analysis, air, variable, lang]);
  const outlierCount = analysis?.air.find((s) => s.name === air)?.outliers.length ?? 0;

  const tokens = useMemo(() => currentTokens(), []);
  const points: MapPoint[] = useMemo(
    () => [
      ...airStations.flatMap((s) =>
        s.at
          ? [
              {
                id: `air:${s.local}`,
                at: s.at,
                color: s.id === station?.id ? tokens.map.selected : tokens.color.danger,
                lines: [s.name, t(lang, "airKind"), ...(s.id === station?.id ? [t(lang, "chosen")] : [])],
              },
            ]
          : [],
      ),
      ...weatherStations.flatMap((s) =>
        s.at
          ? [
              {
                id: `weather:${s.local}`,
                at: s.at,
                color: s.id === weather?.id ? tokens.map.selected : tokens.color.success,
                lines: [s.name, t(lang, "weatherKind"), ...(s.id === weather?.id ? [t(lang, "chosen")] : [])],
              },
            ]
          : [],
      ),
    ],
    [airStations, weatherStations, station, weather, tokens, lang],
  );
  // A station on the map is picked for the comparison and opened in the SDK's panel (SDK-40).
  const { select } = useEntitySelection();
  const pick = (id: string) => {
    const [kind, local] = id.split(":");
    const picked = (kind === "air" ? airStations : weatherStations).find((s) => s.local === local);
    if (!picked) return;
    if (kind === "air") setStation(local);
    else setWeather(local);
    select({ id: picked.id, type: kind === "air" ? AIR : WEATHER });
  };

  const error = airRows.error ?? weatherRows.error ?? historyError;
  const reading =
    (airRows.loading && airRows.rows.length === 0) || (weatherRows.loading && weatherRows.rows.length === 0) || (Boolean(stationId && weatherId) && !history && !error);
  const retry = () => {
    airRows.reload();
    weatherRows.reload();
    setAttempt((n) => n + 1);
  };
  // Either station without a reading in the period leaves nothing to compare.
  const emptyHistory = analysis !== null && (analysis.air.length === 0 || analysis.weather.length === 0);

  return (
    <Page label={t(lang, "page")}>
      {compareNote && (
        <p role={compareNote.error ? "alert" : "status"} className={compareNote.error ? "app-error" : "app-lead"}>
          {compareNote.text}
        </p>
      )}
      {error && (
        <div className="jc-problem" role="alert">
          <strong>{unreadable(error, lang)}</strong>
          <button type="button" className="jc-button" onClick={retry}>
            {t(lang, "retry")}
          </button>
        </div>
      )}
      {failed && (
        <div className="jc-problem" role="alert">
          <strong>{t(lang, "failed", { why: failed })}</strong>
        </div>
      )}
      <section className="app-answer" aria-label={t(lang, "answer")} aria-live="polite" aria-busy={reading}>
        {reading ? (
          <Loading label={t(lang, "reading")} />
        ) : airStations.length === 0 && !error ? (
          <Empty>{t(lang, "noStations")}</Empty>
        ) : emptyHistory ? (
          <Empty>{t(lang, "noHistory")}</Empty>
        ) : said ? (
          <>
            <p className="app-claim">
              {station?.name}: {said.claim}.
            </p>
            <p className="app-lead">{said.evidence}</p>
          </>
        ) : analysis ? (
          <p className="app-lead">{t(lang, "notEnough")}</p>
        ) : !error ? (
          <Loading label={t(lang, "computing")} />
        ) : null}
      </section>
      <form className="app-filters" aria-label={t(lang, "page")} onSubmit={(event) => event.preventDefault()}>
        <label>
          <span>{t(lang, "station")}</span>
          <select value={station?.local ?? ""} onChange={(event) => setStation(event.target.value)}>
            {airStations.map((s) => (
              <option key={s.id} value={s.local}>
                {s.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>{t(lang, "weatherStation")}</span>
          <select value={weather?.local ?? ""} onChange={(event) => setWeather(event.target.value)}>
            {weatherStations.map((s) => (
              <option key={s.id} value={s.local}>
                {s.id === near?.station.id && near.km !== null ? t(lang, "nearest", { name: s.name, km: number(near.km, 1) }) : s.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>{t(lang, "period")}</span>
          <select value={String(days)} onChange={(event) => setDays(event.target.value)}>
            {PERIODS.map((d) => (
              <option key={d} value={d}>
                {d === 1 ? t(lang, "day1") : t(lang, "days", { n: d })}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>{t(lang, "window")}</span>
          <select value={String(smoothing)} onChange={(event) => setWindow(event.target.value)}>
            {WINDOWS.map((h) => (
              <option key={h} value={h}>
                {t(lang, "hours", { n: h })}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>{t(lang, "pollutant")}</span>
          <select value={air} onChange={(event) => setAir(event.target.value)} disabled={!analysis}>
            {(analysis?.air ?? []).map((s) => (
              <option key={s.name} value={s.name}>
                {t(lang, s.name as Pollutant)}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>{t(lang, "variable")}</span>
          <select value={variable} onChange={(event) => setVariable(event.target.value)} disabled={!analysis}>
            {(analysis?.weather ?? []).map((s) => (
              <option key={s.name} value={s.name}>
                {t(lang, s.name as Variable)}
              </option>
            ))}
          </select>
        </label>
      </form>
      <div className="app-open">
        {station && (
          <button type="button" className="jc-button" onClick={() => select({ id: station.id, type: AIR })}>
            {t(lang, "open", { name: station.name })}
          </button>
        )}
        {weather && (
          <button type="button" className="jc-button" onClick={() => select({ id: weather.id, type: WEATHER })}>
            {t(lang, "open", { name: weather.name })}
          </button>
        )}
      </div>
      <Keep
        lang={lang}
        api={api}
        comparison={
          stationId && weatherId
            ? { station: stationId, weather: weatherId, days, smoothing, air: isPollutant(air) ? air : null, variable: isVariable(variable) ? variable : null }
            : null
        }
      />
      <FaultsCard lang={lang} faults={faults} station={station ? { id: station.id, name: station.name } : null} leftOut={cleaned?.leftOut ?? 0} />
      <ChartCard
        title={t(lang, "series", { air: air ? t(lang, air as Pollutant) : "–", weather: variable ? t(lang, variable as Variable) : "–" })}
        option={error ? null : series}
        loading={reading}
        loadingLabel={t(lang, "reading")}
        empty={t(lang, "noHistory")}
        height={320}
      />
      {outlierCount > 0 && <p className="app-lead">{t(lang, "outliers", { n: number(outlierCount), air: t(lang, air as Pollutant) })}</p>}
      <Split ratio="1:1">
        <ChartCard
          title={t(lang, "matrix")}
          option={error ? null : matrix}
          loading={reading}
          loadingLabel={t(lang, "reading")}
          empty={t(lang, "notEnough")}
          height={300}
          onSelect={(name) => {
            const [a, w] = name.split("|");
            if (a && w) {
              setAir(a);
              setVariable(w);
            }
          }}
        />
        <Card title={t(lang, "map")}>
          <MapView points={points} label={t(lang, "mapLabel")} onPick={pick} height={300} />
          <ul className="app-legend" aria-label={t(lang, "map")}>
            <li>
              <span className="app-swatch" aria-hidden="true" style={{ background: tokens.color.danger }} />
              {t(lang, "airKind")}
            </li>
            <li>
              <span className="app-swatch" aria-hidden="true" style={{ background: tokens.color.success }} />
              {t(lang, "weatherKind")}
            </li>
            <li>
              <span className="app-swatch" aria-hidden="true" style={{ background: tokens.map.selected }} />
              {t(lang, "chosen")}
            </li>
          </ul>
        </Card>
      </Split>
      {analysis && analysis.pairs.length > 0 && (
        <Card title={t(lang, "table")}>
          <div className="jc-table-wrap">
            <table className="jc-table">
              <caption className="app-sr">{t(lang, "table")}</caption>
              <thead>
                <tr>
                  <th scope="col">{t(lang, "pair")}</th>
                  <th scope="col" className="jc-num">
                    Spearman
                  </th>
                  <th scope="col" className="jc-num">
                    Pearson
                  </th>
                  <th scope="col" className="jc-num">
                    n
                  </th>
                </tr>
              </thead>
              <tbody>
                {analysis.pairs.map((pair) => (
                  <tr key={`${pair.air}|${pair.weather}`} aria-selected={pair.air === air && pair.weather === variable}>
                    <th scope="row">
                      <button
                        type="button"
                        className="app-link"
                        onClick={() => {
                          setAir(pair.air);
                          setVariable(pair.weather);
                        }}
                      >
                        {t(lang, pair.air as Pollutant)} · {t(lang, pair.weather as Variable)}
                      </button>
                    </th>
                    <td className="jc-num">{pair.spearman === null ? "–" : number(pair.spearman, 2)}</td>
                    <td className="jc-num">{pair.pearson === null ? "–" : number(pair.pearson, 2)}</td>
                    <td className="jc-num">{number(pair.n)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Card>
      )}
    </Page>
  );
}

import { useEffect, useMemo, useState } from "react";
import { Card, Empty, Loading, Page, Problem, useClient, useEntities, useEntitySelection } from "@joinedcontext/sdk";
import type { Row, TemporalPoint } from "@joinedcontext/sdk";
import { ChartCard } from "../components/ChartCard";
import { StationMap } from "../components/StationMap";
import { ModelHistory } from "../components/ModelHistory";
import { ModelInputs } from "../components/ModelInputs";
import { useServer } from "../server";
import type { Inputs } from "../server";
import type { WeatherHistoryPoint } from "../bikes";
import {
  defaultStationId,
  nearestWeatherStation,
  daysAgoIso,
  readHash,
  stationBikes,
  stationFreeSlots,
  stationName,
  stationSlots,
  toInput,
  writeHash,
} from "../bikes";
import { useEstimate } from "../estimate";
import type { Lang } from "../i18n";
import { DAYS, formatHelsinkiTime, number, t } from "../i18n";

const BIKES_QUERY = {
  attrs: ["name", "location", "availableBikeNumber", "freeSlotNumber", "totalSlotNumber", "status", "dateModified"],
};
const WEATHER_QUERY = {
  attrs: ["name", "location", "temperature", "precipitation", "dateObserved"],
};

function formatWeekdayHour(hour: number): string {
  return `${String(hour).padStart(2, "0")}:00`;
}

function lineChartOption(
  bikeHistory: TemporalPoint[],
  output: ReturnType<typeof useEstimate>["output"],
  lang: Lang,
): Record<string, unknown> | null {
  if (bikeHistory.length === 0) return null;

  const sortedHistory = [...bikeHistory]
    .map((p) => {
      const timeMs = new Date(p.observedAt).getTime();
      const val = typeof p.value === "number" ? p.value : Number(p.value);
      return { timeMs, val: Number.isFinite(val) ? val : null };
    })
    .filter((p): p is { timeMs: number; val: number } => !Number.isNaN(p.timeMs) && p.val !== null)
    .sort((a, b) => a.timeMs - b.timeMs);

  if (sortedHistory.length === 0) return null;

  const historyData = sortedHistory.map((p) => [p.timeMs, p.val]);
  const lastPoint = sortedHistory[sortedHistory.length - 1];

  const series: Record<string, unknown>[] = [
    {
      name: t(lang, "bikesHistorySeries"),
      type: "line",
      data: historyData,
      showSymbol: false,
      lineStyle: { width: 2 },
    },
  ];

  if (output?.estimate && output.estimate.length > 0) {
    const estimatePoints = output.estimate
      .map((e) => ({
        timeMs: new Date(e.at).getTime(),
        mean: Math.round(e.mean * 10) / 10,
        low: Math.round(e.low * 10) / 10,
        high: Math.round(e.high * 10) / 10,
      }))
      .filter((e) => !Number.isNaN(e.timeMs));

    if (estimatePoints.length > 0) {
      const estLineData = [[lastPoint.timeMs, lastPoint.val], ...estimatePoints.map((e) => [e.timeMs, e.mean])];
      const lowBandData = [[lastPoint.timeMs, lastPoint.val], ...estimatePoints.map((e) => [e.timeMs, e.low])];
      const bandDiffData = [
        [lastPoint.timeMs, 0],
        ...estimatePoints.map((e) => [e.timeMs, Math.max(0, e.high - e.low)]),
      ];

      series.push(
        {
          name: t(lang, "estimateSeries"),
          type: "line",
          data: estLineData,
          showSymbol: true,
          symbolSize: 6,
          lineStyle: { width: 2, type: "dashed" },
        },
        {
          name: "band-lower",
          type: "line",
          data: lowBandData,
          stack: "confidence-band",
          lineStyle: { opacity: 0 },
          symbol: "none",
        },
        {
          name: t(lang, "estimateBand"),
          type: "line",
          data: bandDiffData,
          stack: "confidence-band",
          lineStyle: { opacity: 0 },
          symbol: "none",
          areaStyle: { opacity: 0.2 },
        },
      );
    }
  }

  return {
    grid: { left: 48, right: 24, top: 24, bottom: 44 },
    tooltip: {
      trigger: "axis",
    },
    xAxis: {
      type: "time",
    },
    yAxis: {
      type: "value",
      min: 0,
    },
    series,
  };
}

/**
 * Main station screen: station picker, live availability tiles, map, 7-day + 6-hour chart,
 * natural language weather effects, and 7x24 hour-of-week profile heat grid table. A station picked
 * on the map, and the chosen station and its weather station by their Details buttons, open in the
 * SDK's entity panel (SDK-40); the App is public, so the panel links them to the Portal (AP-140).
 */
export function Station({ lang }: { lang: Lang }): React.JSX.Element {
  const client = useClient();
  const server = useServer(client.config.appName);
  const stationsRes = useEntities("BikeHireDockingStation", BIKES_QUERY);
  const weatherRes = useEntities("WeatherObserved", WEATHER_QUERY);

  const [selectedId, setSelectedId] = useState<string | null>(() => {
    const hash = typeof window !== "undefined" ? window.location.hash : "";
    return readHash(hash).stationId;
  });
  const [stationSearch, setStationSearch] = useState("");
  const { select } = useEntitySelection();

  const [bikeHistory, setBikeHistory] = useState<TemporalPoint[]>([]);
  const [weatherHistory, setWeatherHistory] = useState<WeatherHistoryPoint[]>([]);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [historyError, setHistoryError] = useState<Error | null>(null);
  const [weatherFailed, setWeatherFailed] = useState(false);
  // Retry reads the history again: picking the same station would change nothing to re-run on.
  const [attempt, setAttempt] = useState(0);
  // The station's model inputs, a steward's or the defaults (T-3827); the page reads with them too.
  const [inputs, setInputs] = useState<Inputs | null>(null);
  const [inputsFailed, setInputsFailed] = useState<string | null>(null);
  // A new version reads the kept models again, after a steward changed the inputs.
  const [version, setVersion] = useState(0);

  const stations = stationsRes.rows;
  const weatherStations = weatherRes.rows;

  useEffect(() => {
    const onHashChange = () => {
      const next = readHash(window.location.hash).stationId;
      // The same station again changes nothing: React keeps the state.
      if (next) setSelectedId(next);
    };
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  // The default station only while none of the stations is chosen. An updater, not this render's
  // selectedId: a station the address named before the effect ran must not be overwritten (T-4788).
  useEffect(() => {
    if (stations.length === 0) return;
    setSelectedId((current) =>
      current && stations.some((s) => s.id === current) ? current : defaultStationId(stations),
    );
    // selectedId too: an address naming a station the list does not hold falls back to the default.
  }, [stations, selectedId]);

  // The address names the chosen station, for a reload or a shared link.
  useEffect(() => {
    if (!selectedId || readHash(window.location.hash).stationId === selectedId) return;
    try {
      window.history.replaceState(null, "", writeHash(selectedId));
    } catch {
      // sandboxed preview may refuse
    }
  }, [selectedId]);

  const selectStation = (id: string) => setSelectedId(id);

  const chosenStation: Row | undefined = useMemo(() => {
    return stations.find((s) => s.id === selectedId);
  }, [stations, selectedId]);

  useEffect(() => {
    setInputs(null);
    setInputsFailed(null);
    if (!selectedId) return;
    let current = true;
    server.inputs(selectedId).then(
      (kept) => current && setInputs(kept),
      (error: unknown) => current && setInputsFailed(error instanceof Error ? error.message : String(error)),
    );
    return () => {
      current = false;
    };
  }, [server, selectedId]);

  // Inputs that could not be read leave the defaults, as the server trains without any.
  const used = inputs?.station === selectedId ? inputs : null;
  const inputsKnown = used !== null || inputsFailed !== null;
  const radiusKm = used?.radiusKm ?? 10;
  const days = used?.days ?? 7;
  const chosenWeather = used?.weatherStation ?? null;

  const nearestWeather = useMemo(() => {
    if (!chosenStation || weatherStations.length === 0) return null;
    if (chosenWeather) {
      const chosen = weatherStations.filter((w) => w.id === chosenWeather);
      return nearestWeatherStation(chosenStation, chosen, Number.POSITIVE_INFINITY);
    }
    return nearestWeatherStation(chosenStation, weatherStations, radiusKm);
  }, [chosenStation, weatherStations, chosenWeather, radiusKm]);

  useEffect(() => {
    if (!selectedId) {
      setBikeHistory([]);
      setWeatherHistory([]);
      return;
    }
    if (!inputsKnown) return;

    let active = true;
    setHistoryLoading(true);
    setHistoryError(null);
    setWeatherFailed(false);

    const now7DaysAgo = daysAgoIso(days);

    const fetchHistories = async () => {
      let bPoints: TemporalPoint[] = [];
      try {
        const bRows = await client.temporal.list("BikeHireDockingStation", {
          id: [selectedId],
          attrs: ["availableBikeNumber"],
          timerel: "after",
          timeAt: now7DaysAgo,
        });
        // Only the station asked for: another entity's series is never shown as this one's.
        const matched = bRows.find((r) => r.id === selectedId);
        bPoints = matched?.series?.availableBikeNumber ?? [];
      } catch (err) {
        if (active) {
          setHistoryError(err instanceof Error ? err : new Error(String(err)));
        }
      }

      let wPoints: WeatherHistoryPoint[] = [];
      if (nearestWeather) {
        try {
          const wRows = await client.temporal.list("WeatherObserved", {
            id: [nearestWeather.station.id],
            attrs: ["temperature", "precipitation"],
            timerel: "after",
            timeAt: now7DaysAgo,
          });
          const matchedW = wRows.find((r) => r.id === nearestWeather.station.id);
          const tempPts = matchedW?.series?.temperature ?? [];
          const precPts = matchedW?.series?.precipitation ?? [];

          const map = new Map<string, { temperature: number | null; precipitation: number | null }>();
          for (const p of tempPts) {
            const v = typeof p.value === "number" ? p.value : Number(p.value);
            const entry = map.get(p.observedAt) ?? { temperature: null, precipitation: null };
            entry.temperature = Number.isFinite(v) ? v : null;
            map.set(p.observedAt, entry);
          }
          for (const p of precPts) {
            const v = typeof p.value === "number" ? p.value : Number(p.value);
            const entry = map.get(p.observedAt) ?? { temperature: null, precipitation: null };
            entry.precipitation = Number.isFinite(v) ? v : null;
            map.set(p.observedAt, entry);
          }
          wPoints = Array.from(map.entries())
            .map(([at, data]) => ({ at, temperature: data.temperature, precipitation: data.precipitation }))
            .sort((a, b) => a.at.localeCompare(b.at));
        } catch {
          if (active) setWeatherFailed(true);
        }
      }

      if (active) {
        setBikeHistory(bPoints);
        setWeatherHistory(wPoints);
        setHistoryLoading(false);
      }
    };

    void fetchHistories();

    return () => {
      active = false;
    };
  }, [client, selectedId, nearestWeather, attempt, inputsKnown, days]);

  const estimateInput = useMemo(() => {
    if (!chosenStation || bikeHistory.length === 0) return null;
    return toInput({
      totalSlots: stationSlots(chosenStation),
      bikePoints: bikeHistory,
      weatherPoints: weatherHistory,
    });
  }, [chosenStation, bikeHistory, weatherHistory]);

  const { output, running: estimateRunning, error: estimateError } = useEstimate(estimateInput);

  const filteredStations = useMemo(() => {
    if (!stationSearch.trim()) return stations;
    const q = stationSearch.toLowerCase();
    return stations.filter((s) => stationName(s).toLowerCase().includes(q));
  }, [stations, stationSearch]);

  const profileMap = useMemo(() => {
    const map = new Map<number, number | null>();
    if (!output) return map;
    for (const slot of output.profile) {
      map.set(slot.slot, slot.mean);
    }
    return map;
  }, [output]);

  // The grid's shades span the profile's own range: a station between 19 and 26 bikes all week
  // would otherwise sit in the top two of six shades and show nothing.
  const profileRange = useMemo(() => {
    const vals = (output?.profile ?? []).map((p) => p.mean).filter((v): v is number => v !== null);
    return vals.length > 0 ? { low: Math.min(...vals), high: Math.max(...vals) } : null;
  }, [output]);

  const getHeatStep = (mean: number | null): number => {
    if (mean === null || !profileRange) return -1;
    const span = profileRange.high - profileRange.low;
    const ratio = span > 0 ? (mean - profileRange.low) / span : 0.5;
    return Math.min(5, Math.floor(Math.min(1, Math.max(0, ratio)) * 5.99));
  };

  const chartOption = useMemo(() => lineChartOption(bikeHistory, output, lang), [bikeHistory, output, lang]);

  const latestWeatherTime = useMemo(() => {
    if (weatherHistory.length > 0) return weatherHistory[weatherHistory.length - 1].at;
    if (nearestWeather?.station.dateObserved) return String(nearestWeather.station.dateObserved);
    return null;
  }, [weatherHistory, nearestWeather]);

  const weatherReadFailed = weatherRes.error !== null || weatherFailed;

  return (
    <Page label={t(lang, "title")}>
      <Problem error={stationsRes.error} onRetry={stationsRes.reload} />
      <Problem error={historyError} onRetry={() => setAttempt((n) => n + 1)} />
      {estimateError && <Problem error={new Error(t(lang, "estimationFailed", { reason: estimateError.message }))} />}

      {weatherReadFailed && !stationsRes.error && <Problem error={new Error(t(lang, "weatherUnreadable"))} />}

      {stationsRes.loading && stations.length === 0 ? (
        <Loading label={t(lang, "loading")} />
      ) : stations.length === 0 ? (
        <Empty>{t(lang, "emptyStations")}</Empty>
      ) : (
        <>
          <div className="app-filter-bar">
            <label className="app-filter-field" htmlFor="station-search-input">
              <span>{t(lang, "searchStation")}</span>
              <input
                id="station-search-input"
                type="search"
                value={stationSearch}
                onChange={(e) => setStationSearch(e.target.value)}
                placeholder={t(lang, "searchStation")}
              />
            </label>

            <label className="app-filter-field" htmlFor="station-select">
              <span>{t(lang, "station")}</span>
              <select
                id="station-select"
                aria-label={t(lang, "station")}
                value={selectedId ?? ""}
                onChange={(e) => selectStation(e.target.value)}
              >
                {filteredStations.map((s) => (
                  <option key={s.id} value={s.id}>
                    {stationName(s)} ({stationBikes(s)} / {stationSlots(s)})
                  </option>
                ))}
              </select>
            </label>

            {nearestWeather && (
              <span className="app-note">
                {t(lang, chosenWeather ? "chosenWeather" : "nearestWeather", {
                  name: stationName(nearestWeather.station),
                  dist: number(lang, nearestWeather.distanceKm, 1),
                })}
              </span>
            )}
          </div>

          <Card title={t(lang, "mapTitle")}>
            <StationMap
              stations={stations}
              selectedId={selectedId}
              onSelect={(id) => {
                selectStation(id);
                select({ id, type: "BikeHireDockingStation" });
              }}
              label={t(lang, "mapTitle")}
            />
            <p className="app-legend-text">{t(lang, "mapNotice")}</p>
          </Card>

          <div className="app-open">
            {chosenStation && (
              <button type="button" className="jc-button" onClick={() => select({ id: chosenStation.id, type: "BikeHireDockingStation" })}>
                {t(lang, "details", { name: stationName(chosenStation) })}
              </button>
            )}
            {nearestWeather && (
              <button type="button" className="jc-button" onClick={() => select({ id: nearestWeather.station.id, type: "WeatherObserved" })}>
                {t(lang, "details", { name: stationName(nearestWeather.station) })}
              </button>
            )}
          </div>

          {chosenStation && (
            <div className="jc-tiles">
              <div className="jc-tile">
                <span className="jc-tile-label">{t(lang, "bikesNow")}</span>
                <span className="jc-tile-value">{stationBikes(chosenStation)}</span>
              </div>
              <div className="jc-tile">
                <span className="jc-tile-label">{t(lang, "freeSlots")}</span>
                <span className="jc-tile-value">{stationFreeSlots(chosenStation)}</span>
              </div>
              <div className="jc-tile">
                <span className="jc-tile-label">{t(lang, "totalSlots")}</span>
                <span className="jc-tile-value">{stationSlots(chosenStation)}</span>
              </div>
            </div>
          )}

          <Card title={t(lang, "chartTitle")}>
            <ChartCard
              title={t(lang, "chartTitle")}
              option={chartOption}
              loading={historyLoading || (estimateRunning && !output)}
              empty={bikeHistory.length === 0 ? t(lang, "noHistory") : t(lang, "chartEmpty")}
              height={320}
            />

            {output?.weather && (
              <p className="app-weather-effect">
                {t(lang, "weatherEffect", {
                  perDegree:
                    (output.weather.perDegree >= 0 ? "+" : "−") + number(lang, Math.abs(output.weather.perDegree), 1),
                  rain: (output.weather.rain >= 0 ? "+" : "−") + number(lang, Math.abs(output.weather.rain), 1),
                  hours: output.weather.hours,
                })}
              </p>
            )}

            {output?.assumption && (
              <p className="app-weather-assumption">
                {t(lang, "weatherAssumption", {
                  time: formatHelsinkiTime(latestWeatherTime ?? new Date(), lang),
                  temp: number(lang, output.assumption.temperature, Number.isInteger(output.assumption.temperature) ? 0 : 1),
                  rainState: output.assumption.raining ? t(lang, "rain") : t(lang, "noRain"),
                })}
              </p>
            )}

            {output && !output.enough && <p className="app-note">{t(lang, "tooLittleHistory")}</p>}
            {output && output.enough && !output.weather && !nearestWeather && (
              <p className="app-note">{t(lang, "noWeatherTerms", { km: number(lang, radiusKm, 1) })}</p>
            )}
          </Card>

          {/* 24 columns scroll sideways on a phone: the box takes the focus, so the keyboard scrolls
              it too (axe scrollable-region-focusable). */}
          <Card title={t(lang, "profileTitle")}>
            {output ? (
              <div className="app-heat-wrap" tabIndex={0} role="region" aria-label={t(lang, "profileTable")}>
                <table className="app-heat-table" aria-label={t(lang, "profileTable")}>
                  <thead>
                    <tr>
                      <th scope="col">{t(lang, "day")}</th>
                      {Array.from({ length: 24 }, (_, h) => (
                        <th key={h} scope="col">
                          {formatWeekdayHour(h)}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {DAYS.map((dayKey, dayIdx) => (
                      <tr key={dayKey}>
                        <th scope="row">{t(lang, dayKey)}</th>
                        {Array.from({ length: 24 }, (_, h) => {
                          const slot = dayIdx * 24 + h;
                          const meanVal = profileMap.get(slot) ?? null;
                          const step = getHeatStep(meanVal);
                          const heatCls = step < 0 ? "app-heat-empty" : `app-heat-${step}`;
                          return (
                            <td key={h} className={`app-heat-cell ${heatCls}`} data-label={`${t(lang, dayKey)} ${h}:00`}>
                              {meanVal !== null ? number(lang, meanVal, 1) : "—"}
                            </td>
                          );
                        })}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            ) : historyLoading ? (
              <Loading label={t(lang, "analysing")} />
            ) : (
              <Empty>{t(lang, "noHistory")}</Empty>
            )}
          </Card>

          {selectedId && (
            <ModelInputs
              lang={lang}
              server={server}
              station={selectedId}
              weatherStations={weatherStations}
              inputs={used}
              failed={inputsFailed}
              onInputs={setInputs}
              onSaved={() => setVersion((n) => n + 1)}
            />
          )}

          {selectedId && <ModelHistory lang={lang} server={server} station={selectedId} version={version} />}
        </>
      )}
    </Page>
  );
}

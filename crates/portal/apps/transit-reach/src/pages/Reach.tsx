import { useEffect, useMemo, useState } from "react";
import { Card, Loading, Page, useEntitySelection } from "@joinedcontext/sdk";
import { useAnalysis } from "../analysis";
import type { AnalysisOutput, StopOut } from "../analysis";
import { KeptReach } from "../components/KeptReach";
import { ReachMap } from "../components/ReachMap";
import { SaveOrigin } from "../components/SaveOrigin";
import { useHistory } from "../history";
import { pointKey, STOP, useNetwork } from "../network";
import { decimal, moment, number, t } from "../i18n";
import type { Lang } from "../i18n";
import { apiBase, reachApi } from "../server";
import type { ReachApi } from "../server";
import { reachColours } from "../theme";
import { CENTRE, HOURS, readView, toVehicles, writeView } from "../vehicles";
import type { Hours, ViewState } from "../vehicles";

const BANDS = [10, 20, 30];
const WAIT = 5;
/** The reached stops listed by name, nearest first; the map shows them all. */
const LISTED = 30;

/** The sentence that answers the page's question before anything is touched. */
export function summaryOf(lang: Lang, out: AnalysisOutput): string {
  const limit = out.bands[out.bands.length - 1];
  return t(lang, "summary", {
    bands: out.bands
      .map((b) =>
        t(lang, "bandPhrase", {
          minutes: number(lang, b.minutes),
          area: decimal(lang, b.areaKm2),
        }),
      )
      .join(", "),
    limit: number(lang, limit?.minutes ?? 0),
    reached: number(lang, limit?.stops ?? 0),
    all: number(lang, out.stops.length),
  });
}

function routesOf(lang: Lang, stop: StopOut): string {
  return stop.routes.length > 0 ? stop.routes.join(", ") : t(lang, "noRoutes");
}

/** A stop as a rider knows it: its name and sign code from HSL's register, else its number here. */
export function stopLabel(lang: Lang, stop: StopOut, index: number): string {
  if (!stop.name)
    return t(lang, "stopOption", {
      n: number(lang, index + 1),
      routes: routesOf(lang, stop),
    });
  return t(lang, "stopNamed", {
    name: stop.code ? `${stop.name} (${stop.code})` : stop.name,
    routes: routesOf(lang, stop),
  });
}

/**
 * How far one gets by transit from a point of Helsinki (T-3331, T-3356): one sentence with the area
 * reached in 10, 20 and 30 minutes, the map of it (a click starts from there), the stops reached in
 * time, and what the answer rests on: HSL's stops and lines when the space holds them, else stops
 * derived from where the vehicles stood still. The point and the history's hours are in the
 * address. An HSL stop reached opens in the SDK's entity panel, which links to it in the Portal
 * (SDK-40, AP-140); the city's data is only read. A signed-in visitor keeps the point with its
 * areas, to compare on the Origins page (T-3826).
 */
export function Reach({ lang, server }: { lang: Lang; server?: ReachApi }) {
  const api = useMemo(() => server ?? reachApi(apiBase()), [server]);
  const [view, setView] = useState<ViewState>(() => readView(window.location.search));
  useEffect(() => {
    try {
      window.history.replaceState(null, "", `${window.location.pathname}${writeView(window.location.search, view)}${window.location.hash}`);
    } catch {
      // A sandboxed preview may refuse; the view still holds on the page.
    }
  }, [view]);

  // HSL's network first; the vehicles' history only when the space holds no network or refuses it.
  const hsl = useNetwork();
  const { network, entities, loading: networkLoading, error: networkError } = hsl;
  const { select } = useEntitySelection();
  const onNetwork = !networkLoading && network.stops.length > 0 && network.routes.length > 0;
  const history = useHistory(view.hours, !networkLoading && !onNetwork);
  const vehicles = useMemo(() => toVehicles(history.history), [history.history]);
  const input = useMemo(() => {
    if (networkLoading) return null;
    if (onNetwork) return { vehicles: [], network, origin: view.at, bands: BANDS, wait: WAIT };
    return history.loading ? null : { vehicles, origin: view.at, bands: BANDS, wait: WAIT };
  }, [networkLoading, onNetwork, network, history.loading, vehicles, view.at]);
  const { output, running, error: failed } = useAnalysis(input);
  const limit = BANDS[BANDS.length - 1];
  const reached = useMemo(
    () =>
      (output?.stops ?? [])
        .map((stop, index) => ({ stop, index }))
        .filter(({ stop }) => stop.minutes !== null)
        .sort((a, b) => (a.stop.minutes ?? 0) - (b.stop.minutes ?? 0)),
    [output],
  );
  // The legend's colours are the map's, for the scheme in force (the page is drawn again per scheme).
  const colours = reachColours();
  const bandColours = [colours.near, colours.mid, colours.far];
  const pickedStop = output?.stops.findIndex((s) => s.lon === view.at.lon && s.lat === view.at.lat) ?? -1;
  // Thousands of HSL stops make no list: the ones reached, nearest first (and the one started
  // from); the few derived stops are all offered.
  const offered =
    output?.source === "network"
      ? reached
          .slice(0, LISTED)
          .concat(
            pickedStop >= 0 && !reached.slice(0, LISTED).some((r) => r.index === pickedStop) ? [{ stop: output.stops[pickedStop], index: pickedStop }] : [],
          )
      : (output?.stops ?? []).map((stop, index) => ({ stop, index }));

  // The HSL stop the view starts from, when it starts from one: its areas can be kept on the server.
  const startEntity = onNetwork ? entities.get(pointKey(view.at)) : undefined;
  const keptStop = startEntity && output && pickedStop >= 0 ? { id: startEntity, label: stopLabel(lang, output.stops[pickedStop], pickedStop) } : null;

  return (
    <Page label={t(lang, "page")}>
      {networkError && (
        <div className="jc-problem" role="alert">
          <strong>{t(lang, "networkUnreadable", { status: networkError.status })}</strong>
          <button type="button" className="jc-button" onClick={hsl.reload}>
            {t(lang, "retry")}
          </button>
        </div>
      )}
      {history.error && (
        <div className="jc-problem" role="alert">
          <strong>{t(lang, "historyUnreadable", { status: history.error.status })}</strong>
          <button type="button" className="jc-button" onClick={history.reload}>
            {t(lang, "retry")}
          </button>
        </div>
      )}
      {failed && (
        <div className="jc-problem" role="alert">
          <strong>{t(lang, "analysisFailed", { reason: failed.message })}</strong>
        </div>
      )}
      {output ? (
        <p className="app-summary" aria-live="polite">
          {summaryOf(lang, output)}
        </p>
      ) : networkLoading || history.loading ? (
        <Loading label={t(lang, "loading")} />
      ) : running ? (
        <Loading label={t(lang, "analysing")} />
      ) : null}
      {output && (
        <p className="app-note">
          {output.source === "network"
            ? t(lang, "network", {
                stops: number(lang, network.stops.length),
                lines: number(lang, network.routes.length),
                wait: number(lang, WAIT),
              })
            : output.readings === 0 || output.first === null || output.last === null
              ? t(lang, "walkOnly")
              : t(lang, "derived", {
                  vehicles: number(lang, output.vehicles),
                  from: moment(lang, output.first),
                  to: moment(lang, output.last),
                  wait: number(lang, WAIT),
                })}
        </p>
      )}

      {onNetwork && <KeptReach lang={lang} api={api} stop={keptStop} />}

      <SaveOrigin
        lang={lang}
        api={api}
        at={view.at}
        stopName={output && pickedStop >= 0 ? output.stops[pickedStop].name || null : null}
        reach={output ? { source: output.source === "network" ? "network" : "derived", bands: output.bands } : null}
      />

      <form className="app-filters" aria-label={t(lang, "controls")} onSubmit={(event) => event.preventDefault()}>
        {output?.source !== "network" && (
          <label>
            <span>{t(lang, "hours")}</span>
            <select value={view.hours} onChange={(event) => setView({ ...view, hours: Number(event.target.value) as Hours })}>
              {HOURS.map((hours) => (
                <option key={hours} value={hours}>
                  {t(lang, "hoursOption", { n: number(lang, hours) })}
                </option>
              ))}
            </select>
          </label>
        )}
        {output && offered.length > 0 && (
          <label>
            <span>{t(lang, "startStop")}</span>
            <select
              value={pickedStop >= 0 ? String(pickedStop) : ""}
              onChange={(event) => {
                const stop = output.stops[Number(event.target.value)];
                if (stop) setView({ ...view, at: { lon: stop.lon, lat: stop.lat } });
              }}
            >
              <option value="">{t(lang, "pickedPoint")}</option>
              {offered.map(({ stop, index }) => (
                <option key={`${stop.lon},${stop.lat}`} value={index}>
                  {stopLabel(lang, stop, index)}
                </option>
              ))}
            </select>
          </label>
        )}
        {(view.at.lon !== CENTRE.lon || view.at.lat !== CENTRE.lat) && (
          <button type="button" className="jc-button" onClick={() => setView({ ...view, at: CENTRE })}>
            {t(lang, "centre")}
          </button>
        )}
      </form>

      <Card title={t(lang, "map")}>
        <ReachMap
          cells={output?.cells ?? []}
          stops={output?.stops ?? []}
          origin={view.at}
          bands={BANDS}
          label={t(lang, "map")}
          describe={(stop, index) => [
            stopLabel(lang, stop, index),
            stop.minutes === null
              ? t(lang, "stopFar", { limit: number(lang, limit) })
              : t(lang, "stopReached", {
                  minutes: decimal(lang, stop.minutes),
                }),
          ]}
          onPick={(at) =>
            setView((current) => ({
              ...current,
              at: {
                lon: Number(at.lon.toFixed(5)),
                lat: Number(at.lat.toFixed(5)),
              },
            }))
          }
        />
        <ul className="app-legend" aria-label={t(lang, "legend")}>
          {BANDS.map((minutes, i) => (
            <li key={minutes}>
              <span className="app-swatch" style={{ background: bandColours[Math.min(i, 2)] }} aria-hidden="true" />
              {t(lang, "bandLegend", { minutes: number(lang, minutes) })}
            </li>
          ))}
          <li>
            <span className="app-swatch app-dot" style={{ background: colours.origin }} aria-hidden="true" />
            {t(lang, "start")}
          </li>
          <li>
            <span className="app-swatch app-dot" style={{ background: colours.stop }} aria-hidden="true" />
            {t(lang, output?.source === "network" ? "stopLegendNetwork" : "stopLegend")}
          </li>
        </ul>
      </Card>

      <Card title={t(lang, "reached", { limit: number(lang, limit) })}>
        {!output ? (
          networkLoading || history.loading ? (
            <Loading label={t(lang, "loading")} />
          ) : null
        ) : reached.length === 0 ? (
          <p className="app-note">{t(lang, "noReached", { limit: number(lang, limit) })}</p>
        ) : (
          <ol className="app-places" aria-label={t(lang, "reached", { limit: number(lang, limit) })}>
            {reached.slice(0, LISTED).map(({ stop, index }) => (
              <li key={index}>
                <button
                  type="button"
                  className="app-link"
                  aria-label={t(lang, "startFrom", {
                    stop: stopLabel(lang, stop, index),
                  })}
                  onClick={() => setView({ ...view, at: { lon: stop.lon, lat: stop.lat } })}
                >
                  {stopLabel(lang, stop, index)}
                </button>
                <span>
                  {t(lang, "stopReached", {
                    minutes: decimal(lang, stop.minutes ?? 0),
                  })}
                </span>
                {entities.has(pointKey(stop)) && (
                  <button
                    type="button"
                    className="jc-button app-details"
                    aria-label={t(lang, "detailsOf", { stop: stopLabel(lang, stop, index) })}
                    onClick={() => select({ id: entities.get(pointKey(stop)) as string, type: STOP })}
                  >
                    {t(lang, "details")}
                  </button>
                )}
              </li>
            ))}
          </ol>
        )}
        {reached.length > LISTED && <p className="app-note">{t(lang, "more", { n: number(lang, reached.length - LISTED) })}</p>}
      </Card>
    </Page>
  );
}

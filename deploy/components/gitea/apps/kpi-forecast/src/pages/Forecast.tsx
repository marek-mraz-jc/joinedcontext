import { useEffect, useMemo, useState } from "react";
import { Card, Page, ProblemError, displayName, format, useEntities, useEntitySelection } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { useAnalysis } from "../analysis";
import type { AnalysisOutput, SeriesResult } from "../analysis";
import { detailOption } from "../charts";
import { ChartCard } from "../components/ChartCard";
import { Earlier } from "../components/Earlier";
import { FollowButton } from "../components/FollowButton";
import { Published } from "../components/Published";
import { Report } from "../components/Report";
import { Empty, Loading } from "@joinedcontext/sdk";
import { useHistory } from "../history";
import { duration, moment, number, percent, t, value } from "../i18n";
import type { Lang } from "../i18n";
import { ATTRS, EMPTY, KPI, WINDOWS, numberOf, readView, shortId, toSeries, writeView } from "../kpis";
import type { ViewState, Window } from "../kpis";
import { apiBase, kpiApi } from "../server";
import type { KpiApi } from "../server";

const QUERY = { attrs: ATTRS };

/** The sentence that answers the page's question before anything is touched. */
export function summaryOf(lang: Lang, out: AnalysisOutput, days: number): string {
  const sentence = t(lang, "summary", {
    total: number(lang, out.results.length),
    days: number(lang, days),
    rising: number(lang, out.rising),
    falling: number(lang, out.falling),
    flat: number(lang, out.flat),
    anomalies: number(lang, out.anomalies),
  });
  return out.short > 0 ? `${sentence} ${t(lang, "summaryShort", { short: number(lang, out.short) })}` : sentence;
}

/** How an indicator is doing, in a few words: its direction and weekly change, or why there is none. */
function stateOf(lang: Lang, result: SeriesResult | undefined, days: number): string {
  if (!result || result.status === "empty") return t(lang, "noHistory", { days: number(lang, days) });
  if (result.status === "short") return t(lang, "shortHistory", { n: number(lang, result.n) });
  const trend = result.trend;
  if (!trend) return "";
  const words = t(lang, trend.direction);
  return trend.changePerWeek === null || trend.direction === "flat" ? words : `${words}, ${t(lang, "perWeek", { change: percent(lang, trend.changePerWeek) })}`;
}

function nameOf(row: Row): string {
  return displayName(row) || shortId(row.id);
}

/**
 * Every Helsinki KPI with its trend, a short forecast and the points that look wrong (T-3332): one
 * sentence that answers how the indicators are moving, the list of them, and the one chosen in
 * full: its history, the model's forecast with its band, and its odd points. The view is in the
 * address.
 */
export function Forecast({ lang, server }: { lang: Lang; server?: KpiApi }) {
  const api = useMemo(() => server ?? kpiApi(apiBase()), [server]);
  const [now] = useState(() => Date.now());
  const { rows, loading, error, reload } = useEntities(KPI, QUERY);
  const { select } = useEntitySelection();
  const [view, setView] = useState<ViewState>(() => readView(window.location.search));
  useEffect(() => {
    try {
      window.history.replaceState(null, "", `${window.location.pathname}${writeView(window.location.search, view)}${window.location.hash}`);
    } catch {
      // A sandboxed preview may refuse; the view still holds on the page.
    }
  }, [view]);

  const history = useHistory(view.days, rows.length > 0);
  // Asks whether the server's daily job recorded the day's forecasts, which later visits set
  // against what came (T-3350, T-6197: the job records them, a visit reads no history).
  // Bookkeeping no reader acts on: a refusal or a 202 changes nothing here.
  useEffect(() => {
    api.record(view.days).catch(() => undefined);
  }, [api, view.days]);
  const sorted = useMemo(() => [...rows].sort((a, b) => nameOf(a).localeCompare(nameOf(b), lang)), [rows, lang]);
  const series = useMemo(() => toSeries(sorted, history.history), [sorted, history.history]);
  const input = useMemo(() => (sorted.length === 0 || history.loading ? null : { series }), [sorted.length, history.loading, series]);
  const { output, running, error: failed } = useAnalysis(input);
  const results = useMemo(() => new Map((output?.results ?? []).map((r) => [r.id, r])), [output]);

  const shown = view.odd ? sorted.filter((row) => (results.get(row.id)?.anomalies.length ?? 0) > 0) : sorted;
  const chosen = shown.find((row) => row.id === view.kpi) ?? shown[0];
  const result = chosen ? results.get(chosen.id) : undefined;
  const option = useMemo(() => (result ? detailOption(result, lang) : null), [result, lang]);
  const waiting = loading && rows.length === 0;

  const unreadable =
    error instanceof ProblemError && error.status > 0 ? t(lang, "unreadableStatus", { status: error.status }) : t(lang, "unreadable");

  return (
    <Page label={t(lang, "page")}>
      {error && (
        <div className="jc-problem" role="alert">
          <strong>{unreadable}</strong>
          <button type="button" className="jc-button" onClick={reload}>
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
      {waiting ? (
        <Loading label={t(lang, "loading")} />
      ) : rows.length === 0 && !error ? (
        <Empty>{t(lang, "none")}</Empty>
      ) : output ? (
        <p className="app-summary" aria-live="polite">
          {summaryOf(lang, output, view.days)}
        </p>
      ) : running || history.loading ? (
        <Loading label={t(lang, "analysing")} />
      ) : null}

      <form className="app-filters" aria-label={t(lang, "filters")} onSubmit={(event) => event.preventDefault()}>
        <label>
          <span>{t(lang, "period")}</span>
          <select value={view.days} onChange={(event) => setView({ ...view, days: Number(event.target.value) as Window })}>
            {WINDOWS.map((days) => (
              <option key={days} value={days}>
                {t(lang, "days", { n: number(lang, days) })}
              </option>
            ))}
          </select>
        </label>
        <label className="app-check">
          <input type="checkbox" checked={view.odd} onChange={(event) => setView({ ...view, odd: event.target.checked })} />
          {t(lang, "odd")}
        </label>
        {(view.odd || view.days !== EMPTY.days) && (
          <button type="button" className="jc-button" onClick={() => setView({ ...EMPTY, kpi: view.kpi })}>
            {t(lang, "clear")}
          </button>
        )}
      </form>

      {rows.length > 0 && (
        <div className="app-layout">
          <Card title={t(lang, "list")}>
            {shown.length === 0 ? (
              <Empty>{t(lang, "nothingOdd")}</Empty>
            ) : (
              <ul className="app-kpis" aria-label={t(lang, "list")}>
                {shown.map((row) => {
                  const r = results.get(row.id);
                  const current = numberOf(row.currentValue);
                  const odd = r?.anomalies.length ?? 0;
                  return (
                    <li key={row.id}>
                      <button
                        type="button"
                        className="app-kpi"
                        aria-pressed={row.id === chosen?.id}
                        // Named by the indicator; how it is doing is its description, so the name
                        // stays the same while the numbers and the language change.
                        aria-label={nameOf(row)}
                        aria-describedby={`state-${shortId(row.id)}`}
                        onClick={() => setView({ ...view, kpi: row.id })}
                      >
                        <strong>{nameOf(row)}</strong>
                        <span id={`state-${shortId(row.id)}`}>
                          {current !== null && <span className="app-now">{t(lang, "now", { value: value(lang, current) })}</span>}
                          <span className={r?.trend ? `app-trend app-${r.trend.direction}` : "app-trend"}>{output ? stateOf(lang, r, view.days) : ""}</span>
                          {odd > 0 && <span className="app-odd">{t(lang, "oddCount", { n: number(lang, odd) })}</span>}
                        </span>
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}
          </Card>
          {chosen && (
            <div className="app-detail">
              <ChartCard
                title={t(lang, "chart", { name: nameOf(chosen) })}
                option={output ? option : null}
                loading={!output && (running || history.loading)}
                empty={t(lang, "chartEmpty")}
                height={320}
              />
              <Card title={nameOf(chosen)}>
                <div className="app-actions">
                  <button type="button" className="jc-button app-open" onClick={() => select({ id: chosen.id, type: KPI })}>
                    {t(lang, "details")}
                  </button>
                  <FollowButton lang={lang} kpi={chosen.id} name={nameOf(chosen)} />
                </div>
                <dl className="app-facts">
                  {result && result.forecast.length > 0 && (
                    <>
                      <dt>{t(lang, "forecast")}</dt>
                      <dd>
                        {(() => {
                          const last = result.forecast[result.forecast.length - 1];
                          return t(lang, "forecastLine", { when: moment(lang, last.t), value: value(lang, last.v), lo: value(lang, last.lo), hi: value(lang, last.hi) });
                        })()}
                      </dd>
                      <dt>{t(lang, "model")}</dt>
                      <dd>
                        {t(lang, "modelLine", {
                          n: number(lang, result.n),
                          step: duration(lang, result.step ?? 0),
                          season: result.season > 0 ? t(lang, "seasonSteps", { n: number(lang, result.season) }) : t(lang, "noSeason"),
                        })}
                      </dd>
                    </>
                  )}
                  {result && result.status !== "ok" && (
                    <>
                      <dt>{t(lang, "model")}</dt>
                      <dd>{stateOf(lang, result, view.days)}</dd>
                    </>
                  )}
                  {format(chosen.calculationFormula) && (
                    <>
                      <dt>{t(lang, "formula")}</dt>
                      <dd>
                        <code>{format(chosen.calculationFormula)}</code>
                      </dd>
                    </>
                  )}
                  {format(chosen.source) && (
                    <>
                      <dt>{t(lang, "source")}</dt>
                      <dd className="app-source">{format(chosen.source)}</dd>
                    </>
                  )}
                </dl>
                {result && result.status === "ok" && (
                  <>
                    <h3 className="app-subhead">{t(lang, "anomalies")}</h3>
                    {result.anomalies.length === 0 ? (
                      <p className="app-note">{t(lang, "noAnomalies")}</p>
                    ) : (
                      <ol className="app-anomalies" aria-label={t(lang, "anomalies")}>
                        {result.anomalies.map((a) => (
                          <li key={a.t}>{t(lang, "anomalyLine", { when: moment(lang, a.t), value: value(lang, a.v), expected: value(lang, a.expected) })}</li>
                        ))}
                      </ol>
                    )}
                  </>
                )}
                <Published lang={lang} api={api} kpi={chosen.id} days={view.days} />
                <Earlier lang={lang} api={api} kpi={chosen.id} history={result?.history ?? []} now={now} />
              </Card>
            </div>
          )}
          <Report lang={lang} api={api} now={now} />
        </div>
      )}
    </Page>
  );
}

import { useEffect, useState } from "react";
import { dueAgainst } from "../earlier";
import type { Due } from "../earlier";
import { moment, number, t, value } from "../i18n";
import type { Lang, Word } from "../i18n";
import { ServerProblem } from "../server";
import type { KeptForecast, KpiApi } from "../server";

/** What the server's refusal means to the reader, in their language. */
export function problemText(lang: Lang, error: unknown): string {
  if (!(error instanceof ServerProblem)) return t(lang, "serverDown");
  const words: Partial<Record<number, Word>> = { 0: "serverOffline", 401: "signInAgain", 403: "noRight", 507: "storageFull" };
  const word = words[error.status];
  if (word) return t(lang, word);
  if (error.status === 400 || error.status === 409) return t(lang, "serverRefused", { why: error.message });
  if (error.status === 404) return t(lang, "gone");
  return t(lang, "serverDown");
}

interface Props {
  lang: Lang;
  api: KpiApi;
  kpi: string;
  /** The indicator's readings as the page read them, `[t, v]`. */
  history: ReadonlyArray<readonly [number, number]>;
  now: number;
}

/** The indicator's earlier forecasts that have come due, against what was measured. */
export function Earlier({ lang, api, kpi, history, now }: Props) {
  const [kept, setKept] = useState<{ kpi: string; forecasts: KeptForecast[] } | null>(null);
  const [error, setError] = useState<unknown>(null);
  useEffect(() => {
    let current = true;
    setError(null);
    api
      .list(kpi)
      .then((forecasts) => current && setKept({ kpi, forecasts }))
      .catch((why: unknown) => current && setError(why));
    return () => {
      current = false;
    };
  }, [api, kpi]);

  const due: Due[] = kept && kept.kpi === kpi ? dueAgainst(kept.forecasts, history, now) : [];
  const measured = due.filter((d) => d.within !== null);
  const inside = measured.filter((d) => d.within).length;

  return (
    <section className="app-earlier" aria-label={t(lang, "earlier")}>
      <h3 className="app-subhead">{t(lang, "earlier")}</h3>
      {error !== null ? (
        <p className="app-error" role="alert">
          {problemText(lang, error)}
        </p>
      ) : !kept || kept.kpi !== kpi ? (
        <p className="app-note">{t(lang, "earlierLoading")}</p>
      ) : due.length === 0 ? (
        <p className="app-note">{t(lang, "earlierNone")}</p>
      ) : (
        <>
          {measured.length > 0 && <p className="app-note">{t(lang, "earlierHits", { inside: number(lang, inside), all: number(lang, measured.length) })}</p>}
          <div className="jc-table-wrap">
            <table className="jc-table">
              <caption className="app-sr">{t(lang, "earlier")}</caption>
              <thead>
                <tr>
                  <th scope="col">{t(lang, "earlierFor")}</th>
                  <th scope="col">{t(lang, "earlierMade")}</th>
                  <th scope="col" className="jc-num">
                    {t(lang, "earlierExpected")}
                  </th>
                  <th scope="col" className="jc-num">
                    {t(lang, "earlierMeasured")}
                  </th>
                </tr>
              </thead>
              <tbody>
                {due.map((d) => (
                  <tr key={`${d.madeOn}-${d.t}`}>
                    <th scope="row">{moment(lang, d.t)}</th>
                    <td>{d.madeOn}</td>
                    <td className="jc-num">{t(lang, "earlierRange", { v: value(lang, d.v), lo: value(lang, d.lo), hi: value(lang, d.hi) })}</td>
                    <td className="jc-num">
                      {d.measured === null ? t(lang, "earlierNoReading") : `${value(lang, d.measured)} · ${t(lang, d.within ? "earlierInside" : "earlierOutside")}`}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
    </section>
  );
}

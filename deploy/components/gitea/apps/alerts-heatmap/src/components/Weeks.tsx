import { useEffect, useState } from "react";
import { Card, Empty, Loading } from "@joinedcontext/sdk";
import { day, number, t } from "../i18n";
import type { Lang } from "../i18n";
import type { Server, Weeks as Kept } from "../server";

/** Noon of a `YYYY-MM-DD` day, so the day is the same in every zone the page is read in. */
const noon = (date: string) => Date.parse(`${date}T12:00:00Z`);

/**
 * The repeat places of every week the App's server has kept (T-3351), the newest first: the feed
 * holds only today's alerts, the server keeps each week's result after the feed has dropped it.
 */
export function Weeks({ lang, server }: { lang: Lang; server: Server }) {
  const [kept, setKept] = useState<Kept | null>(null);
  const [failed, setFailed] = useState<string | null>(null);

  useEffect(() => {
    let gone = false;
    server.weeks().then(
      (answer) => !gone && setKept(answer),
      (error: unknown) => !gone && setFailed(error instanceof Error ? error.message : String(error)),
    );
    return () => {
      gone = true;
    };
  }, [server]);

  return (
    <Card title={t(lang, "weeks")}>
      <p className="app-note">{t(lang, "weeksNote")}</p>
      {failed ? (
        <p className="jc-problem" role="alert">
          {t(lang, "weeksFailed", { reason: failed })}
        </p>
      ) : !kept ? (
        <Loading label={t(lang, "weeksLoading")} />
      ) : kept.weeks.length === 0 ? (
        <Empty>{t(lang, "weeksEmpty")}</Empty>
      ) : (
        <>
          {kept.stale && (
            <p className="app-note" role="status">
              {t(lang, "weeksStale")}
            </p>
          )}
          {/* Scrolls sideways on a phone: a keyboard reaches it as a named region (WCAG 2.1.1). */}
          <div className="app-table-scroll" tabIndex={0} role="region" aria-label={t(lang, "weeks")}>
            <table className="app-table">
              <caption className="app-visually-hidden">{t(lang, "weeks")}</caption>
              <thead>
                <tr>
                  <th scope="col">{t(lang, "weekOf")}</th>
                  <th scope="col">{t(lang, "alertsColumn")}</th>
                  <th scope="col">{t(lang, "spotsColumn")}</th>
                  <th scope="col">{t(lang, "topColumn")}</th>
                </tr>
              </thead>
              <tbody>
                {kept.weeks.map((week) => {
                  const top = week.places[0];
                  return (
                    <tr key={week.week}>
                      <th scope="row">{day(lang, noon(week.week))}</th>
                      <td>{number(lang, week.alerts)}</td>
                      <td>{number(lang, week.places.length)}</td>
                      <td>{top ? `${top.name || `${top.lat.toFixed(4)}, ${top.lon.toFixed(4)}`} (${number(lang, top.count)})` : "–"}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </>
      )}
    </Card>
  );
}

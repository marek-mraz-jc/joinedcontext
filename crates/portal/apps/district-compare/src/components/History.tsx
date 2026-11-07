import { useEffect, useState } from "react";
import { Card, Empty } from "@joinedcontext/sdk";
import type { Measure } from "../districts";
import { locale, measureTitle, number, t } from "../i18n";
import type { Lang } from "../i18n";
import type { DayMetric, History as Kept, Server } from "../server";

const reason = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** Days shown at most, the newest first. */
const SHOWN = 30;

/** A measure's figure of a kept day; `null` where the district had no station. */
export function valueOf(metric: DayMetric, measure: Measure): number | null {
  switch (measure) {
    case "events":
      return metric.events;
    case "bikes":
      return metric.bikes;
    case "bikeSlots":
      return metric.bike_slots;
    case "alerts":
      return metric.alerts;
    case "pm25":
      return metric.pm25;
    case "aqi":
      return metric.aqi;
  }
}

/** Noon of a `YYYY-MM-DD` day, so the day is the same in every zone the page is read in. */
function dayOf(lang: Lang, date: string): string {
  return new Intl.DateTimeFormat(locale(lang), { timeZone: "Europe/Helsinki", dateStyle: "medium" }).format(Date.parse(`${date}T12:00:00Z`));
}

/**
 * The selected districts day by day as the App's server kept them (T-3353), one measure at a
 * time, and the boundaries it compared against with their licence as files.
 */
export function History({ lang, server, codes, measure }: { lang: Lang; server: Server; codes: string[]; measure: Measure }) {
  const [kept, setKept] = useState<Kept | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [download, setDownload] = useState<string | null>(null);
  const asked = codes.slice(0, 6).join(",");

  useEffect(() => {
    setKept(null);
    setFailed(null);
    if (!asked) return;
    let gone = false;
    server.history(asked.split(",")).then(
      (answer) => !gone && setKept(answer),
      (error: unknown) => !gone && setFailed(reason(error)),
    );
    return () => {
      gone = true;
    };
  }, [server, asked]);

  const open = async (which: "geojson" | "licence") => {
    setDownload(null);
    try {
      window.open((await server.files())[which], "_blank", "noopener");
    } catch (error) {
      setDownload(t(lang, "filesFailed", { reason: reason(error) }));
    }
  };

  const columns = asked ? asked.split(",") : [];
  const names = new Map((kept?.days ?? []).map((m) => [m.code, m.name]));
  const days = [...new Set((kept?.days ?? []).map((m) => m.day))].slice(0, SHOWN);
  const cell = (day: string, code: string) => {
    const metric = kept?.days.find((m) => m.day === day && m.code === code);
    const value = metric ? valueOf(metric, measure) : null;
    return value === null ? "–" : number(lang, value, measure === "pm25" || measure === "aqi" ? 1 : 0);
  };

  return (
    <Card title={t(lang, "history", { measure: measureTitle(lang, measure) })}>
      <p className="app-note">{t(lang, "historyNote")}</p>
      <div className="app-files">
        <button type="button" className="jc-button" aria-label={t(lang, "boundariesOf")} onClick={() => void open("geojson")}>
          {t(lang, "boundaries")}
        </button>
        <button type="button" className="jc-button" aria-label={t(lang, "licenceOf")} onClick={() => void open("licence")}>
          {t(lang, "licence")}
        </button>
      </div>
      {download && (
        <p className="jc-problem" role="alert">
          {download}
        </p>
      )}
      {!asked ? (
        <Empty>{t(lang, "historyPick")}</Empty>
      ) : failed ? (
        <p className="jc-problem" role="alert">
          {t(lang, "historyFailed", { reason: failed })}
        </p>
      ) : !kept ? (
        <p className="app-note">{t(lang, "historyReading")}</p>
      ) : days.length === 0 ? (
        <Empty>{t(lang, "historyEmpty")}</Empty>
      ) : (
        <>
          {kept.stale && <p className="app-note">{t(lang, "historyStale")}</p>}
          {/* Scrolls sideways on a phone: a keyboard reaches it as a named region (WCAG 2.1.1). */}
          <div className="jc-table-wrap" tabIndex={0} role="region" aria-label={t(lang, "history", { measure: measureTitle(lang, measure) })}>
            <table className="jc-table" aria-label={t(lang, "history", { measure: measureTitle(lang, measure) })}>
              <thead>
                <tr>
                  <th scope="col">{t(lang, "day")}</th>
                  {columns.map((code) => (
                    <th key={code} scope="col">
                      {names.get(code) ?? code}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {days.map((day) => (
                  <tr key={day}>
                    <th scope="row">{dayOf(lang, day)}</th>
                    {columns.map((code) => (
                      <td key={code} className="jc-num">
                        {cell(day, code)}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
    </Card>
  );
}

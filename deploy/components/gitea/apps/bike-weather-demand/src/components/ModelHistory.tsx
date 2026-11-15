import { useEffect, useState } from "react";
import { Card, Empty } from "@joinedcontext/sdk";
import { locale, number, t } from "../i18n";
import type { Lang } from "../i18n";
import type { Models, Server } from "../server";

const reason = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** A kept day, `YYYY-MM-DD`, in the language's way; noon so every zone reads the same day. */
function dayOf(lang: Lang, date: string): string {
  return new Intl.DateTimeFormat(locale(lang), { timeZone: "Europe/Helsinki", dateStyle: "medium" }).format(Date.parse(`${date}T12:00:00Z`));
}

/** A coefficient with its sign, or a dash where the model has none. */
const signed = (lang: Lang, value: number | null) => (value === null ? "–" : `${value > 0 ? "+" : ""}${number(lang, value, 2)}`);

/**
 * The selected station's model day by day as the App's server kept it (T-3355): its weather
 * coefficients and spread, and the data each day's model was trained on as a file.
 */
export function ModelHistory({ lang, server, station, version = 0 }: { lang: Lang; server: Server; station: string; version?: number }) {
  const [kept, setKept] = useState<Models | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [download, setDownload] = useState<string | null>(null);

  useEffect(() => {
    setKept(null);
    setFailed(null);
    let gone = false;
    server.models(station).then(
      (answer) => !gone && setKept(answer),
      (error: unknown) => !gone && setFailed(reason(error)),
    );
    return () => {
      gone = true;
    };
  }, [server, station, version]);

  const open = async (id: number) => {
    setDownload(null);
    try {
      window.open(await server.snapshotUrl(id), "_blank", "noopener");
    } catch (error) {
      setDownload(t(lang, "trainingFailed", { reason: reason(error) }));
    }
  };

  return (
    <Card title={t(lang, "models")}>
      <p className="app-note">{t(lang, "modelsNote")}</p>
      {download && (
        <p className="jc-problem" role="alert">
          {download}
        </p>
      )}
      {failed ? (
        <p className="jc-problem" role="alert">
          {t(lang, "modelsFailed", { reason: failed })}
        </p>
      ) : !kept ? (
        <p className="app-note">{t(lang, "modelsReading")}</p>
      ) : kept.models.length === 0 ? (
        <Empty>{t(lang, "modelsEmpty")}</Empty>
      ) : (
        <>
          {kept.stale && <p className="app-note">{t(lang, "modelsStale")}</p>}
          {/* Scrolls sideways on a phone: a keyboard reaches it as a named region (WCAG 2.1.1). */}
          <div className="jc-table-wrap" tabIndex={0} role="region" aria-label={t(lang, "models")}>
            <table className="jc-table" aria-label={t(lang, "models")}>
              <thead>
                <tr>
                  <th scope="col">{t(lang, "trainedOn")}</th>
                  <th scope="col">{t(lang, "trainedDays")}</th>
                  <th scope="col">{t(lang, "trainedHours")}</th>
                  <th scope="col">{t(lang, "perDegree")}</th>
                  <th scope="col">{t(lang, "rainEffect")}</th>
                  <th scope="col">{t(lang, "spread")}</th>
                  <th scope="col">{t(lang, "trainingData")}</th>
                </tr>
              </thead>
              <tbody>
                {kept.models.map((model) => (
                  <tr key={model.id}>
                    <th scope="row">{dayOf(lang, model.trained_on)}</th>
                    <td className="jc-num">{model.days ?? 7}</td>
                    <td className="jc-num">{number(lang, model.hours, 0)}</td>
                    <td className="jc-num">{signed(lang, model.per_degree)}</td>
                    <td className="jc-num">{signed(lang, model.rain)}</td>
                    <td className="jc-num">{model.sigma === null ? "–" : number(lang, model.sigma, 2)}</td>
                    <td>
                      <button
                        type="button"
                        className="jc-button"
                        aria-label={t(lang, "trainingDataOf", { day: dayOf(lang, model.trained_on) })}
                        onClick={() => void open(model.id)}
                      >
                        {t(lang, "trainingData")}
                      </button>
                    </td>
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

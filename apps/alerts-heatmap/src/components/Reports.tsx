import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import { Card, Empty, Loading } from "@joinedcontext/sdk";
import type { AnalysisOutput, Place } from "../analysis";
import { day, number, t } from "../i18n";
import type { Lang } from "../i18n";
import type { Report, Server } from "../server";

const reason = (error: unknown) => (error instanceof Error ? error.message : String(error));

/**
 * Hotspot reports (T-3351): the view on screen saved on the App's server with its top repeat
 * places and a picture of the map, and the reports saved before, each shown again by a click.
 */
export function Reports({
  lang,
  server,
  output,
  nameOf,
  snapshot,
  onOpen,
}: {
  lang: Lang;
  server: Server;
  /** The analysis on screen; nothing to save until there is one. */
  output: AnalysisOutput | null;
  /** A repeat place in words. */
  nameOf: (place: Place) => string;
  /** Takes a picture of the map, when the map can. */
  snapshot: { current: (() => Promise<Blob | null>) | null };
  /** Shows a saved view: the address after `?`. */
  onOpen: (view: string) => void;
}) {
  const [reports, setReports] = useState<Report[] | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [title, setTitle] = useState("");
  const [saving, setSaving] = useState(false);
  const [said, setSaid] = useState<{ text: string; problem: boolean } | null>(null);

  useEffect(() => {
    let gone = false;
    server.reports().then(
      (answer) => !gone && setReports(answer),
      (error: unknown) => !gone && setFailed(reason(error)),
    );
    return () => {
      gone = true;
    };
  }, [server]);

  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (!output) return;
    const name = title.trim();
    if (!name) {
      setSaid({ text: t(lang, "nameNeeded"), problem: true });
      return;
    }
    setSaving(true);
    setSaid(null);
    try {
      const report = await server.save({
        title: name,
        view: window.location.search.replace(/^\?/, ""),
        kept: output.kept,
        places: output.places.slice(0, 10).map((place) => ({ name: nameOf(place), lon: place.lon, lat: place.lat, count: place.count })),
      });
      let pictured = false;
      try {
        const png = await snapshot.current?.();
        if (png) {
          await server.attach(report.id, png);
          pictured = true;
        }
      } catch {
        // The report stands without its picture, and the message says so.
      }
      setReports((current) => [{ ...report, has_snapshot: pictured }, ...(current ?? [])]);
      setTitle("");
      setSaid({ text: t(lang, pictured ? "saved" : "savedNoPicture", { title: report.title }), problem: false });
    } catch (error) {
      setSaid({ text: t(lang, "saveFailed", { reason: reason(error) }), problem: true });
    } finally {
      setSaving(false);
    }
  };

  const picture = async (report: Report) => {
    try {
      window.open(await server.snapshotUrl(report.id), "_blank", "noopener");
    } catch (error) {
      setSaid({ text: t(lang, "pictureFailed", { reason: reason(error) }), problem: true });
    }
  };

  return (
    <Card title={t(lang, "reports")}>
      <form className="app-save" onSubmit={save} aria-label={t(lang, "save")}>
        <label>
          <span>{t(lang, "reportName")}</span>
          <input value={title} maxLength={120} onChange={(event) => setTitle(event.target.value)} />
        </label>
        <button type="submit" className="jc-button" disabled={!output || saving}>
          {saving ? t(lang, "saving") : t(lang, "save")}
        </button>
      </form>
      {said && (
        <p className={said.problem ? "jc-problem" : "app-note"} role={said.problem ? "alert" : "status"}>
          {said.text}
        </p>
      )}
      {failed ? (
        <p className="jc-problem" role="alert">
          {t(lang, "reportsFailed", { reason: failed })}
        </p>
      ) : !reports ? (
        <Loading label={t(lang, "loading")} />
      ) : reports.length === 0 ? (
        <Empty>{t(lang, "reportsEmpty")}</Empty>
      ) : (
        <ul className="app-reports" aria-label={t(lang, "reports")}>
          {reports.map((report) => (
            <li key={report.id}>
              <button
                type="button"
                className="app-place"
                aria-label={t(lang, "openReport", { title: report.title })}
                onClick={() => {
                  onOpen(report.view);
                  setSaid({ text: t(lang, "showing", { title: report.title }), problem: false });
                }}
              >
                {report.title}
              </button>
              <span>{t(lang, "reportLine", { kept: number(lang, report.kept), when: day(lang, Date.parse(report.created_at)) })}</span>
              {report.has_snapshot && (
                <button type="button" className="app-pick" aria-label={t(lang, "pictureOf", { title: report.title })} onClick={() => void picture(report)}>
                  {t(lang, "picture")}
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}

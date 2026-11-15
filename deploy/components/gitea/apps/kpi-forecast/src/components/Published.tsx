import { useCallback, useEffect, useId, useRef, useState } from "react";
import { useAccess } from "../access";
import { moment, number, t, value } from "../i18n";
import type { Lang, Word } from "../i18n";
import type { KpiApi, PublishedForecast } from "../server";
import { problemText } from "./Earlier";

interface Props {
  lang: Lang;
  api: KpiApi;
  kpi: string;
  /** The period on screen, which a steward publishes from. */
  days: number;
}

/** A horizon the server takes: a whole number of steps, 1 to 48. */
export function horizonOf(text: string): number | null {
  const n = Number(text);
  return /^\d+$/.test(text.trim()) && n >= 1 && n <= 48 ? n : null;
}

/**
 * The indicator's published forecasts (T-3827): the newest in full, with who published it and the
 * parameters it was made with, and the earlier ones. A steward publishes one from the period on
 * screen with a horizon and a note, and withdraws one; to anyone else the form stays in sight,
 * closed, with the reason.
 */
export function Published({ lang, api, kpi, days }: Props) {
  const { steward } = useAccess();
  const [list, setShown] = useState<{ kpi: string; forecasts: PublishedForecast[] } | null>(null);
  const shown = useRef<PublishedForecast[]>([]);
  const show = useCallback(
    (forecasts: PublishedForecast[]) => {
      shown.current = forecasts;
      setShown({ kpi, forecasts });
    },
    [kpi],
  );
  const [horizon, setHorizon] = useState("12");
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState<number | null>(null);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);
  const id = useId();

  useEffect(() => {
    let current = true;
    setMessage(null);
    api.published(kpi).then(
      (found) => current && show(found),
      (error: unknown) => {
        if (!current) return;
        show([]);
        setMessage({ text: problemText(lang, error), error: true });
      },
    );
    return () => {
      current = false;
    };
  }, [api, kpi, show, lang]);

  const run = async (work: () => Promise<void>, done: Word) => {
    setBusy(true);
    setMessage(null);
    try {
      await work();
      setMessage({ text: t(lang, done), error: false });
    } catch (error) {
      setMessage({ text: problemText(lang, error), error: true });
    } finally {
      setBusy(false);
    }
  };
  const steps = horizonOf(horizon);
  const ready = steward && !busy && steps !== null && note.trim() !== "";
  const closed = steward ? undefined : t(lang, "publishOnly");
  const forecasts = list?.kpi === kpi ? list.forecasts : null;
  const [newest, ...earlier] = forecasts ?? [];
  const line = (f: PublishedForecast) =>
    t(lang, "publishedLine", { by: f.publisherName, when: moment(lang, Date.parse(f.publishedAt)), days: number(lang, f.days), horizon: number(lang, f.horizon) });
  const withdraw = (f: PublishedForecast) =>
    steward &&
    (confirming === f.id ? (
      <span className="app-actions">
        <button
          type="button"
          className="jc-button app-danger"
          disabled={busy}
          onClick={() => {
            setConfirming(null);
            void run(async () => {
              const before = shown.current;
              show(before.filter((one) => one.id !== f.id));
              try {
                await api.withdraw(f.id);
              } catch (error) {
                show(before);
                throw error;
              }
            }, "withdrawn");
          }}
        >
          {t(lang, "confirmWithdraw")}
        </button>
        <button type="button" className="jc-button" onClick={() => setConfirming(null)}>
          {t(lang, "cancel")}
        </button>
      </span>
    ) : (
      <button type="button" className="jc-button" aria-label={`${t(lang, "withdraw")}: ${f.note}`} disabled={busy} onClick={() => setConfirming(f.id)}>
        {t(lang, "withdraw")}
      </button>
    ));

  return (
    <section className="app-published" aria-labelledby={`${id}-title`}>
      <h3 className="app-subhead" id={`${id}-title`}>
        {t(lang, "published")}
      </h3>
      <p role="status" aria-live="polite" className={message?.error ? "app-error" : "app-note"}>
        {message?.text ?? ""}
      </p>
      {forecasts === null ? (
        <p className="app-note">{t(lang, "publishedLoading")}</p>
      ) : !newest ? (
        <p className="app-note">{t(lang, "publishedNone")}</p>
      ) : (
        <>
          <p>{line(newest)}</p>
          <blockquote className="app-quote">{newest.note}</blockquote>
          <table className="app-points">
            <caption className="app-sr">{t(lang, "published")}</caption>
            <thead>
              <tr>
                <th scope="col">{t(lang, "earlierFor")}</th>
                <th scope="col">{t(lang, "earlierExpected")}</th>
              </tr>
            </thead>
            <tbody>
              {newest.points.map((p) => (
                <tr key={p.t}>
                  <td>{moment(lang, p.t)}</td>
                  <td>{t(lang, "earlierRange", { v: value(lang, p.v), lo: value(lang, p.lo), hi: value(lang, p.hi) })}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {withdraw(newest)}
          {earlier.length > 0 && (
            <>
              <h4 className="app-subhead">{t(lang, "publishedEarlier")}</h4>
              <ul className="app-publications" aria-label={t(lang, "publishedEarlier")}>
                {earlier.map((f) => (
                  <li key={f.id}>
                    <span>
                      {line(f)} {f.note}
                    </span>
                    {withdraw(f)}
                  </li>
                ))}
              </ul>
            </>
          )}
        </>
      )}
      <form
        className="app-publish"
        aria-label={t(lang, "publish")}
        onSubmit={(event) => {
          event.preventDefault();
          if (!ready || steps === null) return;
          void run(async () => {
            show([await api.publish(kpi, days, steps, note.trim()), ...shown.current]);
            setNote("");
          }, "publishDone");
        }}
      >
        <p className="app-note">{steward ? t(lang, "publishLead") : t(lang, "publishOnly")}</p>
        <label>
          <span>{t(lang, "horizon")}</span>
          <input inputMode="numeric" value={horizon} disabled={!steward} title={closed} onChange={(event) => setHorizon(event.target.value)} />
        </label>
        <label>
          <span>{t(lang, "note")}</span>
          <input value={note} maxLength={200} disabled={!steward} title={closed} onChange={(event) => setNote(event.target.value)} />
        </label>
        <button type="submit" className="jc-button" disabled={!ready} title={closed}>
          {t(lang, "publish")}
        </button>
      </form>
    </section>
  );
}

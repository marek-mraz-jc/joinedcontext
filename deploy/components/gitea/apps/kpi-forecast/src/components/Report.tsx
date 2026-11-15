import { useId, useState } from "react";
import { go } from "../go";
import { t } from "../i18n";
import type { Lang } from "../i18n";
import type { KpiApi } from "../server";
import { problemText } from "./Earlier";

/** This month and the two before it, `YYYY-MM`, in UTC as the server reads them. */
export function recentMonths(now: number): string[] {
  const at = new Date(now);
  return [0, 1, 2].map((back) => {
    const d = new Date(Date.UTC(at.getUTCFullYear(), at.getUTCMonth() - back, 1));
    return `${d.getUTCFullYear()}-${String(d.getUTCMonth() + 1).padStart(2, "0")}`;
  });
}

/** A month's forecasts against what was measured, as CSV. */
export function Report({ lang, api, now }: { lang: Lang; api: KpiApi; now: number }) {
  const months = recentMonths(now);
  const [month, setMonth] = useState(months[0]);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);
  const id = useId();
  const download = async () => {
    setBusy(true);
    setMessage(null);
    try {
      go(await api.reportUrl(month));
      setMessage({ text: t(lang, "reportDownloading"), error: false });
    } catch (error) {
      setMessage({ text: problemText(lang, error), error: true });
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="app-report" aria-label={t(lang, "report")}>
      <h3 className="app-subhead">{t(lang, "report")}</h3>
      <div className="app-report-row">
        <label htmlFor={id}>{t(lang, "reportMonth")}</label>
        <select id={id} value={month} onChange={(event) => setMonth(event.target.value)}>
          {months.map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>
        <button type="button" className="jc-button" disabled={busy} onClick={() => void download()}>
          {busy ? t(lang, "reportBusy") : t(lang, "reportDownload")}
        </button>
      </div>
      <p role="status" aria-live="polite" className={message?.error ? "app-error" : "app-note"}>
        {message?.text ?? ""}
      </p>
    </section>
  );
}

import { useCallback, useEffect, useState } from "react";
import { Card, Empty, Loading } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import { localeOf, number, ZONE } from "../i18n";
import type { Lang } from "../i18n";
import type { KeptDay, ShareApi } from "../share";
import { t } from "../texts";
import { problemText } from "./ShareDay";

/** The days the reader shared while signed in: open one again, or delete it (gone at once, back on a refusal). */
export function MyDays({ lang, api }: { lang: Lang; api: ShareApi }) {
  const { me } = useAccess();
  const [days, setDays] = useState<KeptDay[] | null>(null);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);
  const [confirming, setConfirming] = useState<number | null>(null);

  const load = useCallback(async () => {
    try {
      setDays(await api.days());
    } catch (error) {
      setDays([]);
      setMessage({ text: problemText(lang, error, "daysFailed", "gone"), error: true });
    }
  }, [api, lang]);

  useEffect(() => {
    if (me) void load();
  }, [me, load]);

  const forget = async (id: number) => {
    const before = days;
    setDays((now) => now?.filter((day) => day.id !== id) ?? now);
    setConfirming(null);
    setMessage(null);
    try {
      await api.forgetDay(id);
      setMessage({ text: t(lang, "dayDeleted"), error: false });
    } catch (error) {
      setDays(before);
      setMessage({ text: problemText(lang, error, "deleteFailed", "gone"), error: true });
    }
  };

  const when = (day: string) => {
    const at = new Date(`${day}T12:00:00Z`);
    return Number.isNaN(at.getTime()) ? day : at.toLocaleDateString(localeOf(lang), { dateStyle: "medium", timeZone: ZONE });
  };

  if (me === null)
    return (
      <Card title={t(lang, "daysTitle")}>
        <Empty>{t(lang, "daysSignIn")}</Empty>
      </Card>
    );
  return (
    <Card title={t(lang, "daysTitle")}>
      <p role="status" aria-live="polite" className={message?.error ? "app-error" : "app-lead"}>
        {message?.text ?? ""}
      </p>
      {days === null ? (
        <Loading label={t(lang, "daysLoading")} />
      ) : days.length === 0 ? (
        <Empty>{t(lang, "daysEmpty")}</Empty>
      ) : (
        <ul className="app-days" aria-label={t(lang, "daysTitle")}>
          {days.map((day) => {
            const line = t(lang, "dayLine", { day: when(day.day), n: number(day.events) });
            return (
              <li key={day.id} className="app-event">
                <strong>{line}</strong>
                <div className="app-filters">
                  <a className="jc-button" href={`?share=${day.code}&lang=${lang}`} aria-label={`${t(lang, "openDay")}: ${line}`}>
                    {t(lang, "openDay")}
                  </a>
                  {confirming === day.id ? (
                    <>
                      <button type="button" className="jc-button app-danger" onClick={() => void forget(day.id)}>
                        {t(lang, "confirmDelete")}
                      </button>
                      <button type="button" className="jc-button" onClick={() => setConfirming(null)}>
                        {t(lang, "cancel")}
                      </button>
                    </>
                  ) : (
                    <button type="button" className="jc-button" onClick={() => setConfirming(day.id)} aria-label={`${t(lang, "deleteDay")}: ${line}`}>
                      {t(lang, "deleteDay")}
                    </button>
                  )}
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </Card>
  );
}

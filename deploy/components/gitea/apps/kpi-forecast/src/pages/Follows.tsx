import { useState } from "react";
import { Card, Empty, Loading, Page, displayName, useEntities } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import { problemText } from "../components/Earlier";
import { useFollows } from "../follows";
import { t } from "../i18n";
import type { Lang } from "../i18n";
import { KPI, shortId } from "../kpis";

const QUERY = { attrs: ["name"] };

/** Where an indicator opens on the indicators page: its id in the address, the language kept. */
export function openHref(kpi: string, lang: Lang): string {
  return `?kpi=${encodeURIComponent(kpi)}&lang=${lang}#kpis`;
}

/** The indicators the reader follows (T-3827), each opened on the indicators page or let go. */
export function Follows({ lang }: { lang: Lang }) {
  const { me } = useAccess();
  const follows = useFollows();
  const { rows, loading } = useEntities(KPI, QUERY);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);
  const nameOf = (kpi: string) => {
    const row = rows.find((r) => r.id === kpi);
    return (row && displayName(row)) || shortId(kpi);
  };

  return (
    <Page label={t(lang, "followsPage")}>
      <Card title={t(lang, "followsTitle")}>
        {me === null ? (
          <Empty>{t(lang, "followSignIn")}</Empty>
        ) : (
          <>
            <p role="status" aria-live="polite" className={message?.error ? "app-error" : "app-note"}>
              {message?.text ?? (follows.failed ? problemText(lang, follows.failed) : "")}
            </p>
            {follows.list === null || (loading && rows.length === 0) ? (
              <Loading label={t(lang, "followsLoading")} />
            ) : follows.list.length === 0 ? (
              follows.failed ? null : <Empty>{t(lang, "followsEmpty")}</Empty>
            ) : (
              <ul className="app-follows" aria-label={t(lang, "followsTitle")}>
                {follows.list.map((f) => {
                  const name = nameOf(f.kpi);
                  return (
                    <li key={f.id}>
                      <strong>{name}</strong>
                      <span className="app-actions">
                        <a className="jc-button" href={openHref(f.kpi, lang)} aria-label={`${t(lang, "open")}: ${name}`}>
                          {t(lang, "open")}
                        </a>
                        <button
                          type="button"
                          className="jc-button"
                          aria-label={`${t(lang, "unfollow")}: ${name}`}
                          onClick={() => {
                            setMessage(null);
                            follows.unfollow(f.id).then(
                              () => setMessage({ text: t(lang, "unfollowed", { name }), error: false }),
                              (error: unknown) => setMessage({ text: problemText(lang, error), error: true }),
                            );
                          }}
                        >
                          {t(lang, "unfollow")}
                        </button>
                      </span>
                    </li>
                  );
                })}
              </ul>
            )}
          </>
        )}
      </Card>
    </Page>
  );
}

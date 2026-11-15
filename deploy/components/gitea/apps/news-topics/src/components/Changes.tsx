import { useCallback, useEffect, useState } from "react";
import { Card, Empty, Loading } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import { formatDate, t } from "../i18n";
import type { Lang } from "../i18n";
import type { Change, Server } from "../server";

/** The App's history, from its `app_audit` table: a viewer's own changes, a steward's everyone's. */
export function Changes({ lang, api }: { lang: Lang; api: Server }) {
  const { me, steward } = useAccess();
  const [rows, setRows] = useState<Change[] | null>(null);
  const [failed, setFailed] = useState<string | null>(null);

  const load = useCallback(async () => {
    setFailed(null);
    try {
      setRows(await api.changes());
    } catch (error) {
      setRows([]);
      // The server and fetch fail with an Error each.
      setFailed((error as Error).message);
    }
  }, [api, lang]);

  useEffect(() => {
    if (me) void load();
  }, [me, load]);

  if (me === null)
    return (
      <Card title={t("changesTitle", lang)}>
        <Empty>{t("changesSignIn", lang)}</Empty>
      </Card>
    );
  return (
    <Card title={t("changesTitle", lang)}>
      <p className="app-note">{t(steward ? "changesAll" : "changesMine", lang)}</p>
      <button type="button" className="jc-button" disabled={rows === null} onClick={() => void load()}>
        {t("refresh", lang)}
      </button>
      {failed && <p className="jc-problem" role="alert">{t("changesFailed", lang, { reason: failed })}</p>}
      {rows === null ? (
        <Loading label={t("changesLoading", lang)} />
      ) : rows.length === 0 ? (
        failed ? null : <Empty>{t("changesEmpty", lang)}</Empty>
      ) : (
        <table className="app-history">
          <caption className="app-sr-only">{t("changesTitle", lang)}</caption>
          <thead>
            <tr>
              <th scope="col">{t("when", lang)}</th>
              <th scope="col">{t("who", lang)}</th>
              <th scope="col">{t("what", lang)}</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.id}>
                <td>{formatDate(row.at, lang)}</td>
                <td>{row.actorName}</td>
                <td>
                  {t(row.action, lang)}: {row.summary}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Card>
  );
}

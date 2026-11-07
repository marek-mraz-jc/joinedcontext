import { useCallback, useEffect, useState } from "react";
import { Card, Empty, Loading } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import { moment, t } from "../i18n";
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
      <Card title={t(lang, "changesTitle")}>
        <Empty>{t(lang, "changesSignIn")}</Empty>
      </Card>
    );
  return (
    <Card title={t(lang, "changesTitle")}>
      <p className="app-note">{t(lang, steward ? "changesAll" : "changesMine")}</p>
      <button type="button" className="jc-button" disabled={rows === null} onClick={() => void load()}>
        {t(lang, "refresh")}
      </button>
      {failed && <p className="jc-problem" role="alert">{t(lang, "changesFailed", { reason: failed })}</p>}
      {rows === null ? (
        <Loading label={t(lang, "changesLoading")} />
      ) : rows.length === 0 ? (
        failed ? null : <Empty>{t(lang, "changesEmpty")}</Empty>
      ) : (
        <table className="app-history">
          <caption className="app-visually-hidden">{t(lang, "changesTitle")}</caption>
          <thead>
            <tr>
              <th scope="col">{t(lang, "when")}</th>
              <th scope="col">{t(lang, "who")}</th>
              <th scope="col">{t(lang, "what")}</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.id}>
                <td>{moment(lang, Date.parse(row.at))}</td>
                <td>{row.actorName}</td>
                <td>
                  {t(lang, row.action)}: {row.summary}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Card>
  );
}

import { useCallback, useEffect, useState } from "react";
import { Card, Empty, Loading } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import { moment, t } from "../i18n";
import type { Lang } from "../i18n";
import type { HistoryRow, Server } from "../server";

/** The App's history, from its `app_audit` table: a viewer's own changes, a steward's everyone's. */
export function History({ lang, api }: { lang: Lang; api: Server }) {
  const { me, steward } = useAccess();
  const [rows, setRows] = useState<HistoryRow[] | null>(null);
  const [failed, setFailed] = useState<string | null>(null);

  const load = useCallback(async () => {
    setFailed(null);
    try {
      setRows(await api.history());
    } catch (error) {
      setRows([]);
      setFailed(error instanceof Error ? error.message : String(error));
    }
  }, [api, lang]);

  useEffect(() => {
    if (me) void load();
  }, [me, load]);

  const when = (iso: string) => {
    const at = Date.parse(iso);
    return Number.isNaN(at) ? iso : moment(lang, at);
  };

  if (me === null)
    return (
      <Card title={t(lang, "historyTitle")}>
        <Empty>{t(lang, "historySignIn")}</Empty>
      </Card>
    );
  return (
    <Card title={t(lang, "historyTitle")}>
      <p className="app-note">{t(lang, steward ? "historyAll" : "historyMine")}</p>
      <button type="button" className="jc-button" disabled={rows === null} onClick={() => void load()}>
        {t(lang, "refresh")}
      </button>
      {failed && <p className="app-error">{failed}</p>}
      {rows === null ? (
        <Loading label={t(lang, "historyLoading")} />
      ) : rows.length === 0 ? (
        failed ? null : <Empty>{t(lang, "historyEmpty")}</Empty>
      ) : (
        <table className="app-history">
          <caption className="app-sr">{t(lang, "historyTitle")}</caption>
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
                <td>{when(row.at)}</td>
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

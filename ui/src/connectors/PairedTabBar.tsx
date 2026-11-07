/**
 * The bar of a tab connected to a connector (T-5967): it says which connector and client may
 * steer the tab, lists what they did here, and disconnects. It listens to the tab's event stream
 * and runs each action; none of them saves anything.
 */
import { useEffect, useState, useSyncExternalStore } from "react";
import type { JSX } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { Button, InlineError } from "../components/ui";
import { attach, disconnectTab, dismissEnded, onPairingChange, pairing, pairingEnded, tabLog } from "./pairedTab";
import type { TabAction } from "./pairedTab";
import { clientLabel } from "../pages/mcp/aiConnectors";

function useStored<T>(read: () => T): T {
  return useSyncExternalStore(onPairingChange, read, read);
}

const pairedKey = () => {
  const value = pairing();
  return value ? `${value.connector}/${value.tab}/${value.name}` : "";
};

/** The client a person knows by name; an unknown id is shown as it is. */
function clientName(t: (key: string) => string, client: string | null): string {
  return client ? clientLabel(client, t) : t("pairedTab.anyClient");
}

function ActionLine({ entry }: { entry: TabAction }): JSX.Element {
  const { t, i18n } = useTranslation();
  return (
    <li className="flex flex-wrap gap-x-2 text-caption">
      <time dateTime={entry.at} className="text-fg-muted">
        {new Date(entry.at).toLocaleTimeString(i18n.language)}
      </time>
      <span>{t("pairedTab.sent", { client: clientName(t, entry.client), connector: entry.connector })}</span>
      <span>{t(`pairedTab.tool.${entry.tool}`, { defaultValue: entry.tool })}</span>
      <span className="break-all font-mono">{entry.what}</span>
      {entry.ran ? null : <span className="text-warning">{t("pairedTab.notFound")}</span>}
    </li>
  );
}

export function PairedTabBar(): JSX.Element | null {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const key = useStored(pairedKey);
  const log = useStored(tabLog);
  const ended = useStored(pairingEnded);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  const client = useQueryClient();
  useEffect(() => attach((href) => void navigate({ href }), () => void client.invalidateQueries()), [navigate, client]);

  if (!key && !ended) {
    return null;
  }
  const current = pairing();
  const disconnect = async () => {
    setBusy(true);
    setFailed(null);
    try {
      await disconnectTab();
      setOpen(false);
    } catch (error) {
      setFailed(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section
      aria-label={t("pairedTab.region")}
      className="fixed inset-x-2 bottom-2 z-40 max-h-96 overflow-y-auto rounded-md border border-info/40 bg-surface p-3 shadow-lg sm:left-4 sm:right-auto sm:max-w-md"
    >
      {current ? (
        <>
          <p className="text-body font-semibold">{t("pairedTab.connected", { name: current.name })}</p>
          <p className="text-caption text-fg-muted">{t("pairedTab.explain")}</p>
          <p role="status" aria-live="polite" className="mt-1 text-caption">
            {log[0] ? t("pairedTab.last", { tool: t(`pairedTab.tool.${log[0].tool}`, { defaultValue: log[0].tool }), what: log[0].what }) : t("pairedTab.waiting")}
          </p>
          {failed ? (
            <InlineError className="mt-1">{failed}</InlineError>
          ) : null}
          <div className="mt-2 flex flex-wrap gap-2">
            <Button variant="secondary" size="sm" aria-expanded={open} disabled={log.length === 0} disabledReason={t("pairedTab.noActions")} onClick={() => setOpen(!open)}>
              {t("pairedTab.showActions", { count: log.length })}
            </Button>
            <Button variant="danger" size="sm" loading={busy} onClick={() => void disconnect()}>
              {t("pairedTab.disconnect")}
            </Button>
          </div>
          {open ? (
            <ol aria-label={t("pairedTab.actions")} className="mt-2 flex flex-col gap-1">
              {log.map((entry) => (
                <ActionLine key={`${entry.at}/${entry.tool}/${entry.what}`} entry={entry} />
              ))}
            </ol>
          ) : null}
        </>
      ) : (
        <div className="flex flex-wrap items-center gap-2">
          <p role="status" className="text-caption">
            {t("pairedTab.ended")}
          </p>
          <Button variant="secondary" size="sm" onClick={dismissEnded}>
            {t("pairedTab.dismiss")}
          </Button>
        </div>
      )}
    </section>
  );
}

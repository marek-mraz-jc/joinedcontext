import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { useIdentity } from "../../auth/AuthProvider";
import { AdminGrid } from "../../components/grid/AdminGrid";
import { operatorAudit, operatorKeys, when } from "./operatorApi";
import type { OperatorAuditEntry } from "./operatorApi";

/** What one audit row is about: the host or the person its row names. */
function subjectOf(row: OperatorAuditEntry): string {
  for (const side of [row.after, row.before]) {
    if (side && typeof side === "object" && !Array.isArray(side)) {
      const fields = side as Record<string, unknown>;
      const named = fields.host ?? fields.subject ?? fields.payload;
      if (typeof named === "string") return named;
      if (named && typeof named === "object") {
        const inner = named as Record<string, unknown>;
        if (typeof inner.host === "string") return inner.host;
        if (typeof inner.subject === "string") return inner.subject;
      }
    }
  }
  return "";
}

/** An audit action in words: "proposed HostAdd" -> "Asked for: Add a host", "host.served" -> "Host now Served". */
function actionText(action: string, t: TFunction): string {
  const [verb, kind] = action.split(" ");
  if (kind && ["proposed", "applied", "rejected"].includes(verb)) {
    return t(`operator.action.${verb}`, { kind: t(`operator.kind.${kind}`, { defaultValue: kind }) });
  }
  if (action.startsWith("host.")) {
    const state = action.slice("host.".length);
    return t("operator.action.host", { state: t(`operator.state.${state}`, { defaultValue: state }) });
  }
  return t(`operator.action.${action}`, { defaultValue: action });
}

/**
 * The operators' append-only audit (T-7129 §4, T-7140): who did what and when, from which
 * address, for every operator Change and every step the reconciler took on a host's certificate
 * and Ingress. Nothing here can be changed.
 */
export function OperatorAudit(): JSX.Element {
  const { t, i18n } = useTranslation();
  const me = useIdentity()?.subject;
  const audit = useQuery({ queryKey: operatorKeys.audit, queryFn: operatorAudit });
  return (
    <section className="space-y-6" aria-labelledby="operator-audit-heading">
      <div>
        <h2 id="operator-audit-heading" className="text-title font-semibold text-fg">
          {t("operator.audit.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("operator.audit.lead")}</p>
      </div>
      <AdminGrid
        query={audit}
        caption={t("operator.audit.caption")}
        rows={audit.data ?? []}
        rowKey={(row) => String(row.id)}
        storageKey="operator.audit"
        empty={t("operator.audit.empty")}
        noSearch={(audit.data ?? []).length < 10}
        stack
        columns={[
          { key: "at", label: t("operator.audit.at"), text: (row) => when(row.at, i18n.language, true) },
          {
            key: "actor",
            label: t("operator.audit.actor"),
            text: (row) =>
              row.actor === "reconciler" ? t("operator.audit.reconciler") : row.actor === me ? t("operator.changes.you") : (row.actorName ?? row.actor),
          },
          { key: "action", label: t("operator.audit.action"), text: (row) => actionText(row.action, t) },
          { key: "subject", label: t("operator.audit.subject"), text: subjectOf, cell: (row) => <span className="font-mono [overflow-wrap:anywhere]">{subjectOf(row)}</span> },
          { key: "source", label: t("operator.audit.source"), text: (row) => row.sourceIp ?? "", hidden: true },
        ]}
      />
    </section>
  );
}

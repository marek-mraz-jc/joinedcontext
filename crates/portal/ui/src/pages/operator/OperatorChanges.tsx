import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { reasonOf } from "../../api/client";
import { useIdentity } from "../../auth/AuthProvider";
import { AdminGrid } from "../../components/grid/AdminGrid";
import { Alert, Badge, Button } from "../../components/ui";
import { decideChange, operatorChanges, operatorKeys, useOrganizationTitles, when } from "./operatorApi";
import type { OperatorChange } from "./operatorApi";

/**
 * The operator Changes (T-7129 §4, T-7140): what one operator asked for, waiting for a second
 * operator to approve it, and what was decided. The proposer reads that another operator decides
 * their own Change; the server refuses it anyway, and a refusal shows the server's sentence.
 */
export function OperatorChanges(): JSX.Element {
  const { t, i18n } = useTranslation();
  const client = useQueryClient();
  const me = useIdentity()?.subject;
  const changes = useQuery({ queryKey: operatorKeys.changes, queryFn: operatorChanges });
  const title = useOrganizationTitles();
  const decide = useMutation({
    mutationFn: ({ id, verb }: { id: string; verb: "approve" | "reject" }) => decideChange(id, verb),
    onSettled: () => client.invalidateQueries({ queryKey: ["operator"] }),
  });
  const target = (row: OperatorChange) => row.payload.host ?? row.payload.subject ?? row.payload.smtp?.host ?? row.payload.project ?? "";
  const asker = (row: OperatorChange) => (row.proposedBy === me ? t("operator.changes.you") : (row.proposedByName ?? row.proposedBy));
  return (
    <section className="space-y-6" aria-labelledby="operator-changes-heading">
      <div>
        <h2 id="operator-changes-heading" className="text-title font-semibold text-fg">
          {t("operator.changes.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("operator.changes.lead")}</p>
      </div>
      {decide.isError ? (
        <Alert tone="danger" title={t("operator.changes.refused")}>
          {reasonOf(decide.error, t("operator.tryAgain"))}
        </Alert>
      ) : null}
      {decide.isSuccess ? (
        <Alert tone="success">
          {t(decide.data.state === "applied" ? "operator.changes.applied" : "operator.changes.rejected", { target: target(decide.data) })}
        </Alert>
      ) : null}
      <AdminGrid
        query={changes}
        caption={t("operator.changes.caption")}
        rows={changes.data ?? []}
        rowKey={(row) => row.id}
        storageKey="operator.changes"
        empty={t("operator.changes.empty")}
        noSearch={(changes.data ?? []).length < 10}
        stack
        columns={[
          { key: "kind", label: t("operator.changes.kind"), text: (row) => t(`operator.kind.${row.kind}`) },
          // The Organization sits under the host and the time under the asker, so a phone card shows
          // all of them before its More details (three fields).
          {
            key: "target",
            label: t("operator.changes.target"),
            text: (row) => [target(row), title(row.payload.organization)].filter(Boolean).join(" "),
            cell: (row) => (
              <span className="block min-w-0">
                <span className="block font-mono [overflow-wrap:anywhere]">{target(row)}</span>
                {row.payload.organization ? (
                  <span className="block text-caption text-fg-muted">
                    {t("operator.changes.organization")}: {title(row.payload.organization)}
                  </span>
                ) : null}
              </span>
            ),
          },
          {
            key: "state",
            label: t("operator.changes.state"),
            text: (row) => t(`operator.changeState.${row.state}`, { defaultValue: row.state }),
            cell: (row) => (
              <Badge tone={row.state === "applied" ? "success" : row.state === "rejected" ? "neutral" : "info"}>
                {t(`operator.changeState.${row.state}`, { defaultValue: row.state })}
              </Badge>
            ),
          },
          {
            key: "proposedBy",
            label: t("operator.changes.proposedBy"),
            text: (row) => `${asker(row)} ${when(row.proposedAt, i18n.language)}`,
            cell: (row) => (
              <span className="block">
                <span className="block">{asker(row)}</span>
                <span className="block text-caption text-fg-muted">
                  <time dateTime={row.proposedAt}>{when(row.proposedAt, i18n.language)}</time>
                </span>
              </span>
            ),
          },
          {
            key: "actions",
            label: t("operator.changes.actions"),
            actions: true,
            text: () => "",
            cell: (row) =>
              row.state !== "proposed" ? null : (
                <span className="flex flex-wrap items-center gap-2">
                  {row.proposedBy === me ? (
                    <span className="text-caption text-fg-muted">{t("operator.changes.ownHint")}</span>
                  ) : (
                    <Button size="sm" loading={decide.isPending} onClick={() => decide.mutate({ id: row.id, verb: "approve" })}>
                      {t("operator.changes.approve")}
                    </Button>
                  )}
                  <Button size="sm" variant="secondary" loading={decide.isPending} onClick={() => decide.mutate({ id: row.id, verb: "reject" })}>
                    {t("operator.changes.reject")}
                  </Button>
                </span>
              ),
          },
        ]}
      />
    </section>
  );
}

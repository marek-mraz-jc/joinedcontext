import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { reasonOf } from "../../api/client";
import { useIdentity } from "../../auth/AuthProvider";
import { AdminGrid } from "../../components/grid/AdminGrid";
import { Alert, Badge, Button } from "../../components/ui";
import { decideChange, operatorChanges, operatorKeys, when } from "./operatorApi";
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
  const decide = useMutation({
    mutationFn: ({ id, verb }: { id: string; verb: "approve" | "reject" }) => decideChange(id, verb),
    onSettled: () => client.invalidateQueries({ queryKey: ["operator"] }),
  });
  const target = (row: OperatorChange) => row.payload.host ?? row.payload.subject ?? "";
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
        stack
        columns={[
          { key: "kind", label: t("operator.changes.kind"), text: (row) => t(`operator.kind.${row.kind}`) },
          { key: "target", label: t("operator.changes.target"), text: target, cell: (row) => <span className="font-mono [overflow-wrap:anywhere]">{target(row)}</span> },
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
          { key: "proposedBy", label: t("operator.changes.proposedBy"), text: (row) => (row.proposedBy === me ? t("operator.changes.you") : row.proposedBy) },
          { key: "proposedAt", label: t("operator.changes.proposedAt"), text: (row) => when(row.proposedAt, i18n.language) },
          {
            key: "organization",
            label: t("operator.changes.organization"),
            text: (row) => row.payload.organization ?? "",
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

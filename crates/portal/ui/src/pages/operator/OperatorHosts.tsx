import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { reasonOf } from "../../api/client";
import { AdminGrid } from "../../components/grid/AdminGrid";
import { Alert, Badge, Button, Card, CardHeader, ConfirmDialog } from "../../components/ui";
import { operatorHosts, operatorKeys, proposeChange, sentence, stateTone, useOrganizationTitles, when } from "./operatorApi";
import type { OperatorHost } from "./operatorApi";

/**
 * The host registry for the installation's operators (T-7129 §3, T-7140): each host's state on
 * its way to served (DNS, certificate, served), what it waits for in the wave's own words, the
 * day's new-certificate slots, and a removal asked for as an operator Change.
 */
export function OperatorHosts(): JSX.Element {
  const { t, i18n } = useTranslation();
  const client = useQueryClient();
  const hosts = useQuery({ queryKey: operatorKeys.hosts, queryFn: operatorHosts });
  const title = useOrganizationTitles();
  const [removing, setRemoving] = useState<OperatorHost | null>(null);
  const remove = useMutation({
    mutationFn: (host: string) => proposeChange({ kind: "HostRemove", host }),
    onSuccess: () => setRemoving(null),
    onSettled: () => client.invalidateQueries({ queryKey: ["operator"] }),
  });
  const slots = hosts.data?.certificates;
  return (
    <section className="space-y-6" aria-labelledby="operator-hosts-heading">
      <div>
        <h2 id="operator-hosts-heading" className="text-title font-semibold text-fg">
          {t("operator.hosts.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("operator.hosts.lead")}</p>
      </div>
      {slots ? (
        <Card>
          <CardHeader title={t("operator.hosts.slots")} description={t("operator.hosts.slotsLead", { week: slots.perDomainWeek })} />
          {slots.usedToday >= slots.perDay ? (
            <Alert tone="warning" title={t("operator.hosts.full")}>
              {t("operator.hosts.used", { used: slots.usedToday, limit: slots.perDay })}
            </Alert>
          ) : (
            <p className="text-body text-fg">
              {t("operator.hosts.used", { used: slots.usedToday, limit: slots.perDay })}
            </p>
          )}
          {slots.nextFreeAt ? (
            <p className="text-body text-fg-muted">
              {t("operator.hosts.nextFree", { at: when(slots.nextFreeAt, i18n.language) })}
            </p>
          ) : null}
        </Card>
      ) : null}
      {remove.isSuccess ? <Alert tone="success">{t("operator.hosts.removalProposed")}</Alert> : null}
      <AdminGrid
        query={hosts}
        caption={t("operator.hosts.caption")}
        rows={hosts.data?.items ?? []}
        rowKey={(row) => row.host}
        storageKey="operator.hosts"
        empty={t("operator.hosts.empty")}
        stack
        columns={[
          // What answers and for whom sit under the host, so a phone card shows them before its
          // More details (three fields).
          {
            key: "host",
            label: t("operator.hosts.host"),
            text: (row) => `${row.host} ${title(row.organization)}`,
            cell: (row) => (
              <span className="block min-w-0">
                <span className="block font-mono [overflow-wrap:anywhere]">{row.host}</span>
                <span className="block text-caption text-fg-muted">
                  {t(`operator.role.${row.role}`, { defaultValue: row.role })} · {title(row.organization)}
                </span>
              </span>
            ),
          },
          {
            key: "state",
            label: t("operator.hosts.state"),
            text: (row) => t(`operator.state.${row.state}`, { defaultValue: row.state }),
            cell: (row) => <Badge tone={stateTone(row.state)}>{t(`operator.state.${row.state}`, { defaultValue: row.state })}</Badge>,
          },
          {
            key: "detail",
            label: t("operator.hosts.detail"),
            text: (row) => row.detail ?? "",
            cell: (row) => <span className="text-fg-muted">{row.detail ? sentence(row.detail) : t(`operator.stateHint.${row.state}`, { defaultValue: "" })}</span>,
          },
          {
            key: "actions",
            label: t("operator.hosts.actions"),
            actions: true,
            text: () => "",
            cell: (row) =>
              row.state === "removed" ? null : (
                <Button size="sm" variant="secondary" onClick={() => setRemoving(row)}>
                  {t("operator.hosts.remove")}
                </Button>
              ),
          },
        ]}
      />
      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(open) => {
          if (!open) {
            setRemoving(null);
            remove.reset();
          }
        }}
        title={t("operator.hosts.removeTitle", { host: removing?.host ?? "" })}
        description={t("operator.hosts.removeLead")}
        confirmLabel={t("operator.hosts.removeConfirm")}
        tone="danger"
        pending={remove.isPending}
        onConfirm={() => removing && remove.mutate(removing.host)}
      >
        {remove.isError ? <Alert tone="danger">{reasonOf(remove.error, t("operator.tryAgain"))}</Alert> : null}
      </ConfirmDialog>
    </section>
  );
}

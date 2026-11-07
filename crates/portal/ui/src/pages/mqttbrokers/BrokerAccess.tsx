import { useId, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, forPeople, unwrap } from "../../api/client";
import { asManifests } from "../../api/manifest";
import { useReadableList } from "../../api/permissions";
import type { components } from "../../api/schema";
import {
  Alert,
  Badge,
  EmptyState,
  Field,
  Select,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";

type Access = components["schemas"]["Access"];

/**
 * Who can do what on a broker (T-5876, ADR-N-048 §2.13): pick one of the project's
 * ServiceAccounts the person reads, and the Portal answers which roles of the broker it holds now,
 * as the gateway decides at CONNECT, with each role's topics and when its grant ends.
 */
export function BrokerAccess({ project, broker }: { project: string; broker: string }): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const accounts = useReadableList(project, "serviceaccounts", "ServiceAccount");
  const names = asManifests(accounts.data?.items ?? []).map((account) => account.metadata.name);
  const [picked, setPicked] = useState("");
  const account = picked !== "" && names.includes(picked) ? picked : (names[0] ?? "");
  const heading = <h3 className="border-t border-border pt-3 text-body font-semibold text-fg">{t("mqttbrokers.access.title")}</h3>;
  if (accounts.hidden) return <></>;
  if (accounts.isPending) {
    return (
      <div aria-busy="true" className="flex flex-col gap-2" data-access="loading">
        {heading}
        <span className="sr-only">{t("app.loading")}</span>
        <Skeleton className="h-4 w-2/3" />
      </div>
    );
  }
  return (
    <section aria-label={t("mqttbrokers.access.title")} className="flex flex-col gap-2" data-access="panel">
      {heading}
      {names.length === 0 ? (
        <EmptyState bare title={t("mqttbrokers.access.noAccounts")} description={t("mqttbrokers.access.noAccountsHint")} />
      ) : (
        <>
          <Field id={`${id}-account`} label={t("mqttbrokers.access.account")} help={t("mqttbrokers.access.accountHelp")}>
            <Select id={`${id}-account`} value={account} onChange={(event) => setPicked(event.target.value)}>
              {names.map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </Select>
          </Field>
          <Held project={project} broker={broker} account={account} />
        </>
      )}
    </section>
  );
}

function Held({ project, broker, account }: { project: string; broker: string; account: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const access = useQuery({
    queryKey: ["mqttBrokerAccess", project, broker, account],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/mqttbrokers/{name}/access", {
          params: { path: { project, name: broker }, query: { account } },
        }),
      ),
  });
  const time = new Intl.DateTimeFormat(i18n.resolvedLanguage ?? i18n.language ?? "en", { dateStyle: "medium", timeStyle: "short" });
  if (access.isPending) {
    return (
      <div aria-busy="true" data-access="deciding">
        <span className="sr-only">{t("app.loading")}</span>
        <Skeleton className="h-4 w-1/2" />
      </div>
    );
  }
  if (access.isError) {
    const refused = access.error instanceof ApiError && access.error.status === 403;
    return (
      <Alert role="alert" tone={refused ? "warning" : "danger"} title={t(refused ? "mqttbrokers.access.refusedTitle" : "mqttbrokers.access.failedTitle")} data-access={refused ? "refused" : "failed"}>
        {access.error instanceof Error ? forPeople(access.error.message) : t("mqttbrokers.access.failedTitle")}
      </Alert>
    );
  }
  const answer: Access = access.data;
  const lapsed = answer.outOfWindow ?? [];
  return (
    <div className="flex flex-col gap-2" data-access={answer.roles.length > 0 ? "held" : lapsed.length > 0 ? "expired" : "none"}>
      {answer.roles.length === 0 ? (
        <Alert tone="info" title={t("mqttbrokers.access.noneTitle", { account })}>
          {t("mqttbrokers.access.noneHint", { broker })}
        </Alert>
      ) : (
        <Table stack caption={t("mqttbrokers.access.caption", { account, broker })}>
          <TableHead>
            <TableHeaderCell>{t("mqttbrokers.acl.role")}</TableHeaderCell>
            <TableHeaderCell>{t("mqttbrokers.acl.publish")}</TableHeaderCell>
            <TableHeaderCell>{t("mqttbrokers.acl.subscribe")}</TableHeaderCell>
            <TableHeaderCell>{t("mqttbrokers.access.through")}</TableHeaderCell>
            <TableHeaderCell>{t("mqttbrokers.access.until")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {answer.roles.map((role) => (
              <TableRow key={role.role}>
                <TableCell primary>{role.role}</TableCell>
                <TableCell data-label={t("mqttbrokers.acl.publish")}>
                  <span className="font-mono">{role.publish.join(", ") || "—"}</span>
                </TableCell>
                <TableCell data-label={t("mqttbrokers.acl.subscribe")}>
                  <span className="font-mono">{role.subscribe.join(", ") || "—"}</span>
                </TableCell>
                <TableCell data-label={t("mqttbrokers.access.through")}>
                  <span className="flex flex-wrap gap-1">
                    {role.bound ? <Badge tone="success">{t("mqttbrokers.access.bound")}</Badge> : null}
                    {role.legacy ? <Badge tone="warning">{t("mqttbrokers.access.legacy")}</Badge> : null}
                  </span>
                </TableCell>
                <TableCell data-label={t("mqttbrokers.access.until")}>{role.until ? time.format(new Date(role.until)) : t("mqttbrokers.access.noEnd")}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      {answer.roles.some((role) => role.legacy) ? (
        <p className="text-caption text-fg-muted">{t("mqttbrokers.access.legacyHint", { broker })}</p>
      ) : null}
      {lapsed.length > 0 ? (
        <Alert tone="warning" title={t("mqttbrokers.access.expiredTitle")} data-access-lapsed="">
          <ul className="list-disc pl-4">
            {lapsed.map((grant) => (
              <li key={grant.grant}>
                <code className="font-mono">{grant.grant}</code>{" "}
                {grant.notAfter
                  ? t("mqttbrokers.access.endedAt", { at: time.format(new Date(grant.notAfter)) })
                  : t("mqttbrokers.access.startsAt", { at: grant.notBefore ? time.format(new Date(grant.notBefore)) : "—" })}
              </li>
            ))}
          </ul>
        </Alert>
      ) : null}
    </div>
  );
}

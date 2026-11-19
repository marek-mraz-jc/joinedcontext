import { useState } from "react";
import type { FormEvent, JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { reasonOf } from "../../api/client";
import { AdminGrid } from "../../components/grid/AdminGrid";
import { CopyUrlButton } from "../../components/endpoints/links";
import { Alert, Badge, Button, Card, CardHeader, CodeBlock, Field, Input, Select } from "../../components/ui";
import {
  HOST_ROLES,
  mintId,
  operatorKeys,
  operatorOrganizations,
  proposeChange,
  requestChallenge,
  sentence,
} from "./operatorApi";
import type { Challenge, OperatorOrganization, OperatorOrganizationId } from "./operatorApi";

/**
 * The installation's Organizations for its operators (T-7129, T-7140): each with its domain and
 * its hosts per state, each Organization id with the TXT record that proves its domain and whether
 * it is minted, and the steps that add a domain: the record to publish, the check that mints the
 * id, then a host asked for as an operator Change a second operator approves.
 */
export function OperatorOrganizations(): JSX.Element {
  const { t } = useTranslation();
  const list = useQuery({ queryKey: operatorKeys.organizations, queryFn: operatorOrganizations });
  const items = list.data?.items ?? [];
  const ids = list.data?.ids ?? [];
  const drift = list.data?.drift;
  return (
    <section className="space-y-6" aria-labelledby="operator-organizations-heading">
      <div>
        <h2 id="operator-organizations-heading" className="text-title font-semibold text-fg">
          {t("operator.organizations.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("operator.organizations.lead")}</p>
      </div>
      {drift && (drift.missingFromForge.length > 0 || drift.missingFromRegistry.length > 0) ? (
        <Alert tone="warning" title={t("operator.organizations.driftTitle")}>
          <p>{t("operator.organizations.driftLead")}</p>
          <dl className="mt-2 space-y-2">
            {(
              [
                [t("operator.organizations.driftForge"), drift.missingFromForge],
                [t("operator.organizations.driftRegistry"), drift.missingFromRegistry],
              ] as const
            )
              .filter(([, names]) => names.length > 0)
              .map(([label, names]) => (
                <div key={label}>
                  <dt className="font-medium">{label}</dt>
                  <dd className="mt-1 flex flex-wrap gap-1">
                    {names.map((name) => (
                      <code key={name} className="rounded bg-surface px-1.5 py-0.5 font-mono text-caption">
                        {name}
                      </code>
                    ))}
                  </dd>
                </div>
              ))}
          </dl>
          <Link to="/organization/$tab" params={{ tab: "settings" }} className="mt-1 inline-block py-2 underline">
            {t("operator.organizations.driftFix")}
          </Link>
        </Alert>
      ) : null}
      <AdminGrid
        query={list}
        caption={t("operator.organizations.caption")}
        rows={items}
        rowKey={(row) => row.name}
        storageKey="operator.organizations"
        empty={t("operator.organizations.empty")}
        noSearch={items.length < 10}
        stack
        columns={[
          {
            key: "name",
            label: t("operator.organizations.name"),
            text: (row) => row.title ?? row.name,
            cell: (row) => (
              <span>
                <span className="block font-medium text-fg">{row.title ?? row.name}</span>
                <span className="block font-mono text-caption text-fg-muted">{row.name}</span>
              </span>
            ),
          },
          {
            key: "domain",
            label: t("operator.organizations.domain"),
            text: (row) => row.domain ?? "",
            cell: (row) =>
              row.domain ? (
                <span className="font-mono">{row.domain}</span>
              ) : (
                <span className="block">
                  <span className="block text-fg-muted">{t("operator.organizations.noDomain")}</span>
                  <Link to="/organization/$tab" params={{ tab: "settings" }} className="text-caption underline">
                    {t("operator.organizations.setDomain")}
                  </Link>
                </span>
              ),
          },
          { key: "hosts", label: t("operator.organizations.hosts"), text: (row) => hostSummary(row, t) },
          {
            key: "approvals",
            label: t("operator.organizations.approvals"),
            text: (row) => t(row.approvals.singlePerson ? "operator.organizations.approvalsOn" : "operator.organizations.approvalsOff"),
            cell: (row) => <SinglePersonCell row={row} />,
          },
        ]}
      />
      <p className="text-caption text-fg-muted">{t("operator.organizations.turnOnHint")}</p>
      <IdList ids={ids} organizations={items} />
      <AddDomain organizations={items} />
    </section>
  );
}

function hostSummary(row: OperatorOrganization, t: (key: string, options?: Record<string, unknown>) => string): string {
  const served = row.hosts.served ?? 0;
  const waiting = Object.entries(row.hosts)
    .filter(([state]) => state !== "served" && state !== "removed")
    .reduce((sum, [, n]) => sum + n, 0);
  return served + waiting === 0 ? t("operator.organizations.noHost") : t("operator.organizations.hostCount", { served, waiting });
}

/**
 * An Organization's single-person approvals (T-7388) and the Change that turns them on or off:
 * on only where one person holds a role (the server checks again), off always.
 */
function SinglePersonCell({ row }: { row: OperatorOrganization }): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const on = row.approvals.singlePerson;
  const people = row.approvals.peopleWithARole;
  const set = useMutation({
    mutationFn: () => proposeChange({ kind: "SinglePersonSet", organization: row.name, singlePerson: !on }),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: operatorKeys.organizations }),
  });
  const crowded = !on && people > 1;
  return (
    <span className="flex flex-col items-start gap-1">
      <span className="flex flex-wrap items-center gap-2">
        <Badge tone={on ? "info" : "neutral"}>
          {t(on ? "operator.organizations.approvalsOn" : "operator.organizations.approvalsOff")}
        </Badge>
        <span className="text-caption text-fg-muted">{t("operator.organizations.people", { count: people })}</span>
      </span>
      {row.approvals.automationLogins.length > 0 ? (
        <span className="text-caption text-fg break-words" data-testid="org-automation-logins">
          {t("operator.organizations.automationLogins", { logins: row.approvals.automationLogins.join(", ") })}
        </span>
      ) : null}
      <Button
        size="sm"
        variant="secondary"
        disabled={crowded}
        disabledReason={crowded ? t("operator.organizations.onePersonOnly", { count: people }) : undefined}
        loading={set.isPending}
        onClick={() => set.mutate()}
      >
        {t(on ? "operator.organizations.turnOff" : "operator.organizations.turnOn")}
      </Button>
      {crowded ? (
        <span className="text-caption text-fg-muted">{t("operator.organizations.onePersonOnly", { count: people })}</span>
      ) : null}
      {set.isSuccess ? (
        <span role="status" className="text-caption text-fg-muted">
          {t(set.data.state === "applied" ? "operator.organizations.applied" : "operator.organizations.asked")}
        </span>
      ) : null}
      {set.isError ? (
        <span role="alert" className="text-caption text-danger-fg">
          {reasonOf(set.error, t("app.error.generic"))}
        </span>
      ) : null}
    </span>
  );
}

/** Each Organization id and its TXT record; an id not minted yet is checked again here. */
function IdList({ ids, organizations }: { ids: OperatorOrganizationId[]; organizations: OperatorOrganization[] }): JSX.Element {
  const { t } = useTranslation();
  const client = useQueryClient();
  const mint = useMutation({
    mutationFn: mintId,
    onSettled: () => client.invalidateQueries({ queryKey: operatorKeys.organizations }),
  });
  const named = (row: OperatorOrganizationId) =>
    row.organizations.length > 0
      ? row.organizations.map((name) => organizations.find((org) => org.name === name)?.title ?? name).join(", ")
      : t("operator.ids.unclaimed");
  return (
    <Card>
      <CardHeader title={t("operator.ids.title")} description={t("operator.ids.lead")} />
      {mint.isError ? (
        <Alert tone="danger" title={t("operator.ids.notMinted")} className="mb-3">
          {reasonOf(mint.error, t("operator.tryAgain"))}
        </Alert>
      ) : null}
      <AdminGrid
        caption={t("operator.ids.caption")}
        rows={ids}
        rowKey={(row) => row.id}
        storageKey="operator.ids"
        empty={t("operator.ids.empty")}
        noSearch={ids.length < 10}
        stack
        columns={[
          { key: "domain", label: t("operator.ids.domain"), text: (row) => row.domain, cell: (row) => <span className="font-mono">{row.domain}</span> },
          {
            key: "id",
            label: t("operator.ids.organization"),
            text: (row) => `${named(row)} ${row.id}`,
            cell: (row) => (
              <span className="block">
                <span className={`block ${row.organizations.length > 0 ? "text-fg" : "text-fg-muted"}`}>{named(row)}</span>
                <span className="block font-mono text-caption text-fg-muted">{row.id}</span>
              </span>
            ),
          },
          {
            key: "record",
            label: t("operator.ids.record"),
            text: (row) => `${row.recordName} ${row.recordValue}`,
            cell: (row) => (
              <span className="block min-w-0 font-mono text-caption [overflow-wrap:anywhere]">
                {row.recordName} TXT &quot;{row.recordValue}&quot;
              </span>
            ),
          },
          {
            key: "state",
            label: t("operator.ids.state"),
            text: (row) => (row.mintedAt ? t("operator.ids.minted") : t("operator.ids.waiting")),
            cell: (row) =>
              row.mintedAt ? (
                <Badge tone="success">{t("operator.ids.minted")}</Badge>
              ) : (
                <Button
                  size="sm"
                  variant="secondary"
                  loading={mint.isPending}
                  onClick={() => mint.mutate(row.domain)}
                >
                  {t("operator.ids.check")}
                </Button>
              ),
          },
        ]}
      />
    </Card>
  );
}

/**
 * Adding a domain: its TXT record to publish, the check that mints its id, then a host of one of
 * the installation's Organizations asked for as a `HostAdd` Change.
 */
function AddDomain({ organizations }: { organizations: OperatorOrganization[] }): JSX.Element {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [domain, setDomain] = useState("");
  const [challenge, setChallenge] = useState<Challenge | null>(null);
  const [host, setHost] = useState("");
  const [organization, setOrganization] = useState("");
  const [role, setRole] = useState<string>("working");
  const refresh = () => client.invalidateQueries({ queryKey: ["operator"] });
  const ask = useMutation({ mutationFn: requestChallenge, onSuccess: setChallenge, onSettled: refresh });
  const mint = useMutation({ mutationFn: mintId, onSettled: refresh });
  const propose = useMutation({ mutationFn: proposeChange, onSettled: refresh });
  const orgId = challenge?.organizationId;
  // The id goes with the host only for the Organization whose domain it proves; the server
  // refuses it for any other (organizationId names another Organization).
  const chosen = organizations.find((org) => org.name === organization);
  const proved = mint.isSuccess && challenge !== null && chosen?.domain === challenge.domain;
  // The server names the field it refuses first ("host: …"): show that under the field itself.
  const said = (error: unknown) => (error ? reasonOf(error, t("operator.tryAgain")) : "");
  const fieldOf = (error: unknown, field: string): string[] | undefined => {
    const text = said(error);
    return text.startsWith(`${field}:`) ? [sentence(text.slice(field.length + 1).trim())] : undefined;
  };
  const proposeFields = ["host", "organization", "organizationId", "role"];
  const proposeElsewhere = propose.isError && !proposeFields.some((field) => fieldOf(propose.error, field));

  const onAsk = (event: FormEvent) => {
    event.preventDefault();
    ask.mutate(domain.trim().toLowerCase());
  };
  const onPropose = (event: FormEvent) => {
    event.preventDefault();
    propose.mutate({
      kind: "HostAdd",
      host: host.trim().toLowerCase(),
      organization,
      organizationId: proved ? orgId : undefined,
      role,
    });
  };

  return (
    <Card>
      <CardHeader title={t("operator.add.title")} description={t("operator.add.lead")} />
      <ol className="mt-4 divide-y divide-border">
        <li className="space-y-3 py-5 first:pt-0 last:pb-0">
          <h3 className="text-body font-semibold text-fg">{t("operator.add.step1")}</h3>
          <form className="flex flex-col items-start gap-3" onSubmit={onAsk}>
            <Field id="operator-domain" label={t("operator.add.domain")} help={t("operator.add.hint.domain")} errors={fieldOf(ask.error, "domain")} className="w-full max-w-md">
              <Input id="operator-domain" value={domain} onChange={(e) => setDomain(e.target.value)} autoComplete="off" spellCheck={false} required />
            </Field>
            <Button type="submit" loading={ask.isPending}>
              {t("operator.add.ask")}
            </Button>
          </form>
          {ask.isError && !fieldOf(ask.error, "domain") ? <Alert tone="danger">{said(ask.error)}</Alert> : null}
          {challenge ? (
            <div className="space-y-2">
              <p className="text-body text-fg">{t("operator.add.publish", { domain: challenge.domain })}</p>
              <CodeBlock label={t("operator.add.record")}>{challenge.record}</CodeBlock>
              <CopyUrlButton url={challenge.record} label={t("operator.add.copy")} />
            </div>
          ) : null}
        </li>
        <li className="space-y-3 py-5 first:pt-0 last:pb-0">
          <h3 className="text-body font-semibold text-fg">{t("operator.add.step2")}</h3>
          <p className="text-body text-fg-muted">{t("operator.add.step2Lead")}</p>
          <Button variant="secondary" disabled={!challenge} disabledReason={t("operator.add.mintFirst")} loading={mint.isPending} onClick={() => challenge && mint.mutate(challenge.domain)}>
            {t("operator.add.mint")}
          </Button>
          {mint.isError ? <Alert tone="danger">{reasonOf(mint.error, t("operator.tryAgain"))}</Alert> : null}
          {mint.isSuccess ? <Alert tone="success">{t("operator.add.minted", { id: orgId })}</Alert> : null}
        </li>
        <li className="space-y-3 py-5 first:pt-0 last:pb-0">
          <h3 className="text-body font-semibold text-fg">{t("operator.add.step3")}</h3>
          {organizations.length === 0 ? (
            <p className="text-body text-fg-muted">{t("operator.add.noOrganization")}</p>
          ) : (
            <form className="grid gap-3 sm:grid-cols-2" onSubmit={onPropose}>
              <Field id="operator-host" label={t("operator.add.host")} help={t("operator.add.hint.host")} errors={fieldOf(propose.error, "host") ?? fieldOf(propose.error, "organizationId")}>
                <Input id="operator-host" value={host} onChange={(e) => setHost(e.target.value)} autoComplete="off" spellCheck={false} required />
              </Field>
              <Field id="operator-organization" label={t("operator.add.organization")} help={t("operator.add.hint.organization")} errors={fieldOf(propose.error, "organization")}>
                <Select id="operator-organization" value={organization} onChange={(e) => setOrganization(e.target.value)} required>
                  <option value="">{t("operator.add.organizationPick")}</option>
                  {organizations.map((org) => (
                    <option key={org.name} value={org.name}>
                      {org.title ?? org.name}
                    </option>
                  ))}
                </Select>
              </Field>
              <Field id="operator-role" label={t("operator.add.role")} help={t("operator.add.hint.role")} errors={fieldOf(propose.error, "role")}>
                <Select id="operator-role" value={role} onChange={(e) => setRole(e.target.value)}>
                  {HOST_ROLES.map((r) => (
                    <option key={r} value={r}>
                      {t(`operator.role.${r}`)}
                    </option>
                  ))}
                </Select>
              </Field>
              {mint.isSuccess && challenge && chosen && !chosen.domain ? (
                <p className="text-body text-fg-muted sm:col-span-2">
                  {t("operator.add.orgNoDomain", { organization: chosen.title ?? chosen.name, domain: challenge.domain })}
                </p>
              ) : null}
              <div className="sm:col-span-2">
                <Button type="submit" loading={propose.isPending}>
                  {t("operator.add.propose")}
                </Button>
              </div>
            </form>
          )}
          {proposeElsewhere ? <Alert tone="danger">{said(propose.error)}</Alert> : null}
          {propose.isSuccess ? (
            <Alert tone="success">
              {t("operator.add.proposed")}{" "}
              <Link to="/organization/$tab" params={{ tab: "operator-changes" }} className="underline">
                {t("operator.add.toChanges")}
              </Link>
            </Alert>
          ) : null}
        </li>
      </ol>
    </Card>
  );
}

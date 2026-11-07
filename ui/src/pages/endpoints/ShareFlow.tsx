import { useId, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { asManifests, ORG_NAMESPACE } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { useReadableList } from "../../api/permissions";
import { useProposal } from "../../api/proposal";
import { useProjects } from "../../api/projects";
import { ChangeNotice } from "../../components/ChangeNotice";
import { endpointUrl } from "../../components/endpoints/links";
import { RecordView, Alert, Button, Checkbox, Field, Input, Select } from "../../components/ui";

/**
 * Sharing an Endpoint with another project, guided (T-3275, EP-15): the Endpoint lists the
 * project in `allowedProjects`, and one Policy grants that project's group the reads of the chosen
 * types, as the assistant's share renders it (EP-72). The Policy's `validity.to` is the share's
 * end, which the gateway enforces like every validity; revoking ends it now and takes the project
 * off the list, in one Change. Nothing is written until it is approved.
 */

type Spec = Record<string, unknown> & { allowedProjects?: string[]; audience?: string };

/** The Policy that carries one share, by the name the assistant's share gives it. */
export function sharePolicyName(endpoint: string, project: string): string {
  return `${endpoint}-${project}`;
}

/** A manifest as the API takes it back: without the status it computed. */
function writable(manifest: Manifest): Manifest {
  const copy = { ...manifest } as Manifest & { status?: unknown };
  delete copy.status;
  return copy;
}

/** The Endpoint and the Policy that share it with `target` until `until` (none: no end). */
export function shareBundle(
  endpoint: Manifest,
  space: string,
  target: string,
  types: string[],
  until: string | undefined,
): { endpoint: Manifest; policy: Manifest } {
  const spec = endpoint.spec as Spec;
  const allowed = [...new Set([...(spec.allowedProjects ?? []), target])];
  return {
    endpoint: { ...writable(endpoint), spec: { ...spec, audience: "project-list", allowedProjects: allowed } },
    policy: {
      apiVersion: endpoint.apiVersion,
      kind: "Policy",
      metadata: { name: sharePolicyName(endpoint.metadata.name, target), namespace: endpoint.metadata.namespace },
      spec: {
        contextSpaceRef: { kind: "ContextSpace", name: space },
        // Rendered by the loader, so a copied organization is its own assigner (CC-82).
        assigner: "did:web:{orgDomain}",
        assignee: { kind: "group", id: target },
        operations: ["retrieveOps"],
        information: [{ entities: types.map((type) => ({ type })) }],
        ...(until ? { validity: { to: until } } : {}),
      },
    } as Manifest,
  };
}

/**
 * The empty Group a share's Policy names when the repository has none of that name: a Policy
 * naming an undeclared group does not validate (T-1042, PF-62), as the assistant's share drafts it.
 */
export function shareGroup(endpoint: Manifest, target: string): Manifest {
  return {
    apiVersion: endpoint.apiVersion,
    kind: "Group",
    metadata: { name: target, namespace: endpoint.metadata.namespace },
    spec: { members: [] },
  } as Manifest;
}

/** The Endpoint without `target`, and its share's Policy ended `now`: the grant stops at once. */
export function revokeBundle(endpoint: Manifest, policy: Manifest | undefined, target: string, now: string): Manifest[] {
  const spec = endpoint.spec as Spec;
  const updated = { ...writable(endpoint), spec: { ...spec, allowedProjects: (spec.allowedProjects ?? []).filter((p) => p !== target) } };
  if (!policy) return [updated];
  const policySpec = policy.spec as Record<string, unknown> & { validity?: Record<string, unknown> };
  return [updated, { ...writable(policy), spec: { ...policySpec, validity: { ...policySpec.validity, to: now } } }];
}

/** Where one share stands: open with no end, open until a time, or ended. */
export function shareState(policy: Manifest | undefined, now: Date): { state: "open" | "until" | "ended" | "noGrant"; until?: Date } {
  if (!policy) return { state: "noGrant" };
  const to = (policy.spec as { validity?: { to?: string } }).validity?.to;
  if (!to) return { state: "open" };
  const until = new Date(to);
  return until.getTime() <= now.getTime() ? { state: "ended", until } : { state: "until", until };
}

/** An entity as the other project reads it through this Endpoint: its slots, never a hidden one. */
export function asSharedSees(entity: Record<string, unknown>, slots: string[], hidden: string[]): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(entity).filter(
      ([key]) => key === "id" || key === "type" || ((slots.length === 0 || slots.includes(key)) && !hidden.includes(key)),
    ),
  );
}

interface GroupList {
  hidden: boolean;
  data?: unknown;
}

/**
 * The Group names the organization and the project hold, or undefined while they are not known.
 * A list the caller does not read is not asked for (T-3852). The organization's then counts as
 * empty; the project's, where the Group would be proposed, is unknown: an empty Group proposed
 * over one the person cannot see would drop its members, so the share proposes none.
 */
export function knownGroupNames(organization: GroupList, project: GroupList): Set<string> | undefined {
  if (project.hidden || project.data === undefined) return undefined;
  if (!organization.hidden && organization.data === undefined) return undefined;
  return new Set(
    [organization, project].flatMap((list) =>
      asManifests((list.data as { items?: Manifest[] } | undefined)?.items ?? []).map((group) => group.metadata.name),
    ),
  );
}

export function ShareFlow({
  project,
  endpoint,
  space,
  types,
  slots,
  policies,
}: {
  project: string;
  endpoint: Manifest;
  space: string;
  /** The types the Endpoint publishes (its projection's classes). */
  types: string[];
  /** The attributes its projection keeps; none: every attribute. */
  slots: string[];
  /** The project's Policies, to find each share's own. */
  policies: Manifest[];
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const ids = useId();
  const spec = endpoint.spec as Spec & { slug?: string; projection?: { hiddenAttributes?: string[] } };
  const hidden = spec.projection?.hiddenAttributes ?? [];
  const shared = spec.allowedProjects ?? [];
  const projects = (useProjects().data ?? []).filter((name) => name !== project && !shared.includes(name));
  const groupNames = knownGroupNames(useReadableList(ORG_NAMESPACE, "groups", "Group"), useReadableList(project, "groups", "Group"));
  const [target, setTarget] = useState("");
  const [chosen, setChosen] = useState<string[]>(types);
  const [until, setUntil] = useState("");
  const [change, setChange] = useState<Change | null>(null);
  const needsGroup = target !== "" && groupNames !== undefined && !groupNames.has(target);
  const proposal = useProposal(project, "endpoints", setChange);
  const day = new Intl.DateTimeFormat(i18n.language, { dateStyle: "medium" });
  const policyOf = (other: string) => policies.find((policy) => policy.metadata.name === sharePolicyName(endpoint.metadata.name, other));

  const preview = useQuery({
    queryKey: ["share-preview", spec.slug, chosen],
    enabled: Boolean(spec.slug) && target !== "" && chosen.length > 0,
    retry: false,
    queryFn: async () => {
      const rows: Record<string, unknown>[] = [];
      for (const type of chosen) {
        const answer = await globalThis.fetch(
          new Request(endpointUrl(spec.slug ?? "", `/ngsi-ld/v1/entities?type=${encodeURIComponent(type)}&limit=2`), {
            credentials: "same-origin",
            headers: { Accept: "application/json" },
          }),
        );
        if (!answer.ok) continue;
        const body = (await answer.json()) as unknown;
        for (const entity of Array.isArray(body) ? body : []) rows.push(asSharedSees(entity as Record<string, unknown>, slots, hidden));
      }
      return rows;
    },
  });

  if (spec.audience === "organization" || spec.audience === "public") {
    return <p className="text-body text-fg-muted">{t(`endpoints.shareFlow.already.${spec.audience}`)}</p>;
  }

  const share = () => {
    if (!target || chosen.length === 0) return;
    const bundle = shareBundle(endpoint, space, target, chosen, until ? new Date(`${until}T23:59:59`).toISOString() : undefined);
    const extra = needsGroup ? [shareGroup(endpoint, target)] : [];
    proposal.mutation.mutate({ body: bundle.endpoint, create: false, bundle: [...extra, bundle.policy] });
  };
  const revoke = (other: string) => {
    const [updated, ...ended] = revokeBundle(endpoint, policyOf(other), other, new Date().toISOString());
    proposal.mutation.mutate({ body: updated, create: false, bundle: ended });
  };

  return (
    <div className="flex flex-col gap-4" data-testid="share-flow">
      <section aria-labelledby={`${ids}-now`} className="flex flex-col gap-2">
        <h3 id={`${ids}-now`} className="text-body font-semibold text-fg">
          {t("endpoints.shareFlow.sharedWith")}
        </h3>
        {shared.length === 0 ? (
          <p className="text-caption text-fg-muted">{t("endpoints.shareFlow.nobody")}</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {shared.map((other) => {
              const state = shareState(policyOf(other), new Date());
              return (
                <li key={other} className="flex flex-wrap items-center gap-2 rounded-md border border-border p-2">
                  <span className="font-mono text-body">{other}</span>
                  <span className="text-caption text-fg-muted">
                    {t(`endpoints.shareFlow.state.${state.state}`, { until: state.until ? day.format(state.until) : "" })}
                  </span>
                  {state.state !== "ended" ? (
                    <Button
                      size="sm"
                      variant="secondary"
                      className="ml-auto"
                      disabled={proposal.mutation.isPending}
                      disabledReason={t("app.busyReason")}
                      aria-label={t("endpoints.shareFlow.revokeOf", { project: other })}
                      onClick={() => revoke(other)}
                    >
                      {t("endpoints.shareFlow.revoke")}
                    </Button>
                  ) : null}
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section aria-labelledby={`${ids}-new`} className="flex flex-col gap-3">
        <h3 id={`${ids}-new`} className="text-body font-semibold text-fg">
          {t("endpoints.shareFlow.title")}
        </h3>
        <Field id={`${ids}-project`} label={t("endpoints.shareFlow.project")}>
          <Select id={`${ids}-project`} value={target} onChange={(event) => setTarget(event.target.value)}>
            <option value="">{t("form.choose")}</option>
            {projects.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </Select>
        </Field>
        <fieldset className="flex flex-col gap-1" aria-describedby={`${ids}-types-help`}>
          <legend className="text-body font-medium text-fg">{t("endpoints.shareFlow.types")}</legend>
          <p id={`${ids}-types-help`} className="text-caption text-fg-muted">{t("endpoints.shareFlow.typesHelp")}</p>
          {types.map((type) => (
            <Checkbox
              key={type}
              id={`${ids}-type-${type}`}
              label={type}
              checked={chosen.includes(type)}
              onChange={(event) => setChosen(event.target.checked ? [...chosen, type] : chosen.filter((t) => t !== type))}
            />
          ))}
        </fieldset>
        <Field id={`${ids}-until`} label={t("endpoints.shareFlow.until")} description={t("endpoints.shareFlow.untilHint")}>
          <Input id={`${ids}-until`} type="date" value={until} onChange={(event) => setUntil(event.target.value)} />
        </Field>
        {target ? (
          <section aria-labelledby={`${ids}-preview`} className="rounded-md border border-border bg-surface-subtle p-3">
            <h4 id={`${ids}-preview`} className="text-caption font-semibold text-fg">
              {t("endpoints.shareFlow.preview", { project: target })}
            </h4>
            <p className="text-caption text-fg-muted">
              {t("endpoints.shareFlow.previewLead", { types: chosen.join(", ") || "—", hidden: hidden.join(", ") || "—" })}
            </p>
            {preview.data && preview.data.length > 0 ? (
              <RecordView
                value={preview.data}
                label={t("endpoints.shareFlow.preview", { project: target })}
                className="mt-2 max-h-60"
                data-testid="share-preview"
              />
            ) : null}
          </section>
        ) : null}
        {needsGroup ? (
          <Alert role="status" tone="info">
            {t("endpoints.shareFlow.newGroup", { group: target })}
          </Alert>
        ) : null}
        {proposal.error ? (
          <Alert role="alert" tone="danger">
            {proposal.error}
          </Alert>
        ) : null}
        <Button
          variant="primary"
          className="w-fit"
          loading={proposal.mutation.isPending}
          disabled={!target || chosen.length === 0}
          disabledReason={!target ? t("endpoints.shareFlow.pickProject") : chosen.length === 0 ? t("endpoints.shareFlow.pickType") : undefined}
          onClick={share}
        >
          {t("endpoints.shareFlow.propose")}
        </Button>
        {change ? <ChangeNotice change={change} project={project} /> : null}
      </section>
    </div>
  );
}

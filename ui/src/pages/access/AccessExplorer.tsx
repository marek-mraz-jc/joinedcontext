import { useId, useState } from "react";
import type { FormEvent, JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import { asManifests, ORG_NAMESPACE } from "../../api/manifest";
import { administersOrganization, usePermissions } from "../../api/permissions";
import { useBranding } from "../../branding";
import { RecordLink } from "../../components/RecordLink";
import { ResourceNamePicker } from "../../components/pickers/ResourceNamePicker";
import { CopyUrlButton, hubUrl, namedServerUrl, representationUrl } from "../../components/endpoints/links";
import { Alert, Badge, Button, Card, CardHeader, ConfirmDialog, EmptyState, Field, Input, Select } from "../../components/ui";
import type { GeoQ, GrantEntry } from "../../api/access";

type Who = components["schemas"]["Who"];
type Kind = Who["kind"];
type Explored = components["schemas"]["Explored"];
type Reached = components["schemas"]["Reached"];
type Consent = components["schemas"]["Consent"];


const KINDS: Kind[] = ["person", "group", "role", "serviceAccount", "public"];

/** `?kind=serviceAccount&name=etl` (a person by `name=` their id): the link to one principal. */
function linked(): { kind: Kind; name: string } {
  const search = new URLSearchParams(window.location.search);
  const kind = search.get("kind") as Kind | null;
  return kind && KINDS.includes(kind) ? { kind, name: search.get("name") ?? "" } : { kind: "person", name: "" };
}

/**
 * Access explorer (T-3792, API/01 §41): an organization administrator picks a person, a group or
 * role member, a ServiceAccount or the public and sees every Endpoint of the project they reach,
 * what they may do there, the one MCP connector over all of it, and a person's connected clients.
 * The gateway decides every row; nothing is computed here. Anyone else sees nothing.
 */
export function AccessExplorer({ project }: { project: string }): JSX.Element | null {
  const permissions = usePermissions(ORG_NAMESPACE);
  if (!administersOrganization(permissions.data)) {
    return null;
  }
  return <Explorer project={project} />;
}

function Explorer({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const initial = linked();
  const [kind, setKind] = useState<Kind>(initial.kind);
  const [name, setName] = useState(initial.name);
  const [search, setSearch] = useState("");

  const accounts = useQuery({
    queryKey: queryKeys.list(project, "serviceaccounts"),
    enabled: kind === "serviceAccount",
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "serviceaccounts" } },
        }),
      ),
  });
  const people = useQuery({
    queryKey: queryKeys.peoplePage(search.trim(), 0),
    enabled: kind === "person",
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/organization/people", {
          params: { query: { ...(search.trim() ? { search: search.trim() } : {}), first: 0, max: 20 } },
        }),
      ),
  });
  const accountNames = asManifests(accounts.data?.items ?? []).map((a) => a.metadata.name);

  const who = (): Who => {
    switch (kind) {
      case "person":
        return { kind, id: name || people.data?.items[0]?.id || "" };
      case "serviceAccount":
        return { kind, name: name || accountNames[0] || "" };
      case "group":
      case "role":
        return { kind, name: name.trim() };
      case "public":
        return { kind };
    }
  };

  const explore = useMutation({
    mutationFn: async (subject: Who) =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/access/explore", {
          params: { path: { project } },
          body: { subject },
        }),
      ),
  });

  const subject = who();
  const missing = ("id" in subject && !subject.id) || ("name" in subject && !subject.name);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (missing) return;
    const link = new URLSearchParams({ kind, ...("id" in subject ? { name: subject.id } : {}), ...("name" in subject ? { name: subject.name } : {}) });
    window.history.replaceState(window.history.state, "", `${window.location.pathname}?${link.toString()}`);
    explore.mutate(subject);
  };

  return (
    <Card>
      <CardHeader titleId={`${id}-title`} title={t("access.explorer.title")} description={t("access.explorer.lead")} />
      <form className="mt-4 grid gap-4 sm:grid-cols-3" aria-labelledby={`${id}-title`} onSubmit={submit}>
        <Field id={`${id}-kind`} label={t("access.explorer.kind")} description={t("access.explorer.kindHelp")} required>
          <Select
            id={`${id}-kind`}
            value={kind}
            onChange={(e) => {
              setKind(e.target.value as Kind);
              setName("");
              explore.reset();
            }}
          >
            {KINDS.map((k) => (
              <option key={k} value={k}>
                {t(`policies.try.kinds.${k}`)}
              </option>
            ))}
          </Select>
        </Field>
        {kind === "person" ? (
          <>
            <Field id={`${id}-search`} label={t("access.explorer.search")}>
              <Input id={`${id}-search`} type="search" value={search} onChange={(e) => setSearch(e.target.value)} autoComplete="off" />
            </Field>
            <Field id={`${id}-person`} label={t("policies.try.kinds.person")} description={t("access.explorer.personHelp")} required>
              <Select id={`${id}-person`} value={subject.kind === "person" ? subject.id : ""} onChange={(e) => setName(e.target.value)}>
                {(people.data?.items ?? []).length === 0 ? <option value="">{t("policies.try.noPerson")}</option> : null}
                {(people.data?.items ?? []).map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.email}
                  </option>
                ))}
              </Select>
            </Field>
          </>
        ) : null}
        {kind === "serviceAccount" ? (
          <Field id={`${id}-account`} label={t("policies.try.kinds.serviceAccount")} description={t("access.explorer.accountHelp")} required>
            <Select id={`${id}-account`} value={subject.kind === "serviceAccount" ? subject.name : ""} onChange={(e) => setName(e.target.value)}>
              {accountNames.length === 0 ? <option value="">{t("policies.try.noAccount")}</option> : null}
              {accountNames.map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </Select>
          </Field>
        ) : null}
        {kind === "group" || kind === "role" ? (
          <Field id={`${id}-name`} label={t(`policies.try.kinds.${kind}`)} description={t("access.explorer.nameHelp")} required>
            <ResourceNamePicker
              id={`${id}-name`}
              label={t(`policies.try.kinds.${kind}`)}
              labelled
              // The groups of the organization, or the roles of this project and the organization.
              from={
                kind === "group"
                  ? { project: ORG_NAMESPACE, plural: "groups" }
                  : [
                      { project, plural: "roles" },
                      { project: ORG_NAMESPACE, plural: "roles" },
                    ]
              }
              value={name}
              onChange={setName}
              required
            />
          </Field>
        ) : null}
        <div className="sm:col-span-3">
          <Button
            type="submit"
            disabled={missing}
            disabledReason={t("access.explorer.pickFirst")}
            loading={explore.isPending}
          >
            {t("access.explorer.show")}
          </Button>
        </div>
      </form>
      <div aria-live="polite" className="mt-6">
        {explore.isError ? (
          <Alert tone="danger" title={t("access.explorer.failed")}>
            {explore.error instanceof ApiError ? (explore.error.problem?.detail ?? explore.error.message) : t("app.error.generic")}
          </Alert>
        ) : null}
        {explore.data ? (
          <Result
            project={project}
            personId={subject.kind === "person" ? subject.id : undefined}
            explored={explore.data}
            onRevoked={() => explore.mutate(subject)}
          />
        ) : null}
      </div>
    </Card>
  );
}

function Result({
  project,
  personId,
  explored,
  onRevoked,
}: {
  project: string;
  personId: string | undefined;
  explored: Explored;
  onRevoked: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const { domain } = useBranding();
  return (
    <div className="space-y-6">
      {explored.endpoints.length === 0 ? (
        <EmptyState bare title={t("access.explorer.nothing")} description={t("access.explorer.nothingLead")} />
      ) : (
        <section aria-labelledby="explorer-endpoints" className="space-y-2">
          <h3 id="explorer-endpoints" className="text-title font-semibold text-fg">
            {t("access.explorer.endpoints", { count: explored.endpoints.length })}
          </h3>
          <ul className="space-y-2">
            {explored.endpoints.map((row) => (
              <Row key={row.name} project={project} row={row} domain={domain} />
            ))}
          </ul>
        </section>
      )}
      {explored.endpoints.length > 0 ? (
        <section aria-labelledby="explorer-mcp" className="space-y-2">
          <h3 id="explorer-mcp" className="text-title font-semibold text-fg">
            {t("access.explorer.mcp")}
          </h3>
          <p className="text-body text-fg-muted">{t("access.explorer.mcpLead")}</p>
          <div className="flex flex-wrap items-center gap-2">
            <code className="break-all font-mono text-caption">{hubUrl(domain)}</code>
            <CopyUrlButton url={hubUrl(domain)} label={t("access.explorer.connectAll")} />
          </div>
          {explored.mcpServers.length > 0 ? (
            <ul className="space-y-1">
              {explored.mcpServers.map((server) => (
                <li key={server} className="flex flex-wrap items-center gap-2">
                  <RecordLink project={project} plural="mcpservers" name={server}>
                    {server}
                  </RecordLink>
                  <code className="break-all font-mono text-caption">{namedServerUrl(domain, project, server)}</code>
                </li>
              ))}
            </ul>
          ) : null}
        </section>
      ) : null}
      {personId ? <Consents personId={personId} consents={explored.consents} onRevoked={onRevoked} /> : null}
    </div>
  );
}

function Row({ project, row, domain }: { project: string; row: Reached; domain: string | undefined }): JSX.Element {
  const { t } = useTranslation();
  const entries = row.permissions as GrantEntry[];
  const prohibitions = row.prohibitions as GrantEntry[];
  return (
    <li className="rounded-lg border border-border">
      <details>
        <summary className="flex cursor-pointer flex-wrap items-center gap-2 p-3">
          <span className="font-semibold text-fg">{row.title ?? row.name}</span>
          <span className="text-caption text-fg-muted">{t("access.explorer.space", { space: row.space })}</span>
          {row.representations.map((rep) => (
            <Badge key={rep}>{rep}</Badge>
          ))}
        </summary>
        <div className="space-y-4 border-t border-border p-3">
          <p className="flex flex-wrap items-center gap-2 text-body">
            <span>{t("access.explorer.through")}</span>
            {row.policies.map((policy) => (
              <RecordLink key={policy} project={project} plural="policies" name={policy}>
                {policy}
              </RecordLink>
            ))}
            <RecordLink project={project} plural="endpoints" name={row.name}>
              {t("access.explorer.openEndpoint")}
            </RecordLink>
          </p>
          <Entries title={t("access.explorer.may")} entries={entries} />
          {prohibitions.length > 0 ? <Entries title={t("access.explorer.mayNot")} entries={prohibitions} /> : null}
          <div className="flex flex-wrap gap-2">
            {row.representations.map((rep) => (
              <CopyUrlButton key={rep} url={representationUrl(row.slug, rep, domain)} label={t("access.explorer.copy", { surface: rep })} />
            ))}
          </div>
        </div>
      </details>
    </li>
  );
}

function Entries({ title, entries }: { title: string; entries: GrantEntry[] }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div>
      <h4 className="text-body font-semibold text-fg">{title}</h4>
      <ul className="mt-1 space-y-2">
        {entries.map((entry, index) => (
          <li key={index} className="text-body">
            <span className="font-mono">{entry.resource?.type ?? entry.resource?.id ?? "*"}</span>
            {": "}
            {(entry.actions ?? []).join(", ")}
            <div className="text-caption text-fg-muted">
              {t("access.explorer.attributes")}{" "}
              {entry.attributes === "*" || !entry.attributes ? t("access.explorer.allAttributes") : entry.attributes.join(", ")}
            </div>
            {entry.constraints?.q ? <Constraint label="q" value={entry.constraints.q} /> : null}
            {entry.constraints?.scopeQ ? <Constraint label="scopeQ" value={entry.constraints.scopeQ} /> : null}
            {entry.constraints?.geoQ ? <GeoPreview geoQ={entry.constraints.geoQ} /> : null}
          </li>
        ))}
      </ul>
    </div>
  );
}

function Constraint({ label, value }: { label: string; value: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="text-caption">
      {t("access.explorer.only")} <code className="font-mono">{`${label}=${value}`}</code>
    </div>
  );
}

/** Every [lon, lat] pair in a GeoJSON `coordinates` value, however deep. */
function points(coordinates: unknown): Array<[number, number]> {
  if (Array.isArray(coordinates) && typeof coordinates[0] === "number" && typeof coordinates[1] === "number") {
    return [[coordinates[0], coordinates[1]]];
  }
  return Array.isArray(coordinates) ? coordinates.flatMap(points) : [];
}

/**
 * A geoQ drawn as its shape (T-3792): the geometry's points scaled into a small box, so "only
 * near the harbour" is seen, not read from coordinates. No basemap: the shape and its text.
 */
export function GeoPreview({ geoQ }: { geoQ: GeoQ }): JSX.Element {
  const { t } = useTranslation();
  const pts = points(geoQ.coordinates);
  const text = `${geoQ.georel ?? ""} ${geoQ.geometry ?? ""} ${JSON.stringify(geoQ.coordinates ?? [])}`.trim();
  if (pts.length === 0) {
    return <Constraint label="geoQ" value={text} />;
  }
  // A loop, not Math.min(...xs): spreading a large polygon's points overflows the call stack.
  let [minX, maxX, minY, maxY] = [Infinity, -Infinity, Infinity, -Infinity];
  for (const [x, y] of pts) {
    minX = Math.min(minX, x);
    maxX = Math.max(maxX, x);
    minY = Math.min(minY, y);
    maxY = Math.max(maxY, y);
  }
  const span = Math.max(maxX - minX, maxY - minY) || 1;
  const at = ([x, y]: [number, number]) => `${(((x - minX) / span) * 80 + 10).toFixed(1)},${(90 - ((y - minY) / span) * 80).toFixed(1)}`;
  return (
    <figure className="mt-1 flex items-center gap-3 text-caption">
      <svg viewBox="0 0 100 100" className="size-16 rounded border border-border" role="img" aria-label={t("access.explorer.geoQ", { shape: text })} data-testid="geoq-preview">
        {pts.length === 1 ? (
          <circle cx="50" cy="50" r="6" className="fill-primary-soft-fg" />
        ) : (
          <polygon points={pts.map(at).join(" ")} className="fill-primary-soft-fg/30 stroke-primary-soft-fg" strokeWidth="2" />
        )}
      </svg>
      <figcaption>
        {t("access.explorer.only")} <code className="font-mono">{`geoQ ${text}`}</code>
      </figcaption>
    </figure>
  );
}

function Consents({ personId, consents, onRevoked }: { personId: string; consents: Consent[]; onRevoked: () => void }): JSX.Element {
  const { t, i18n } = useTranslation();
  const [revoking, setRevoking] = useState<string | null>(null);
  const revoke = useMutation({
    mutationFn: async (client: string) =>
      unwrap(
        await api.DELETE("/api/v1/organization/people/{id}/consents/{client}", {
          params: { path: { id: personId, client } },
        }),
      ),
    onSuccess: () => {
      setRevoking(null);
      onRevoked();
    },
  });
  return (
    <section aria-labelledby="explorer-consents" className="space-y-2">
      <h3 id="explorer-consents" className="text-title font-semibold text-fg">
        {t("access.explorer.consents")}
      </h3>
      {consents.length === 0 ? (
        <p className="text-body text-fg-muted">{t("access.explorer.noConsents")}</p>
      ) : (
        <ul className="space-y-2">
          {consents.map((consent) => (
            <li key={consent.clientId} className="flex flex-wrap items-center gap-3">
              <span className="font-mono">{consent.clientId}</span>
              <span className="text-caption text-fg-muted">{(consent.grantedClientScopes ?? []).join(", ")}</span>
              {consent.lastUpdatedDate ? (
                <span className="text-caption text-fg-muted">
                  {t("access.explorer.lastUsed", { when: new Date(consent.lastUpdatedDate).toLocaleString(i18n.language) })}
                </span>
              ) : null}
              <Button size="sm" variant="danger" onClick={() => setRevoking(consent.clientId)}>
                {t("access.explorer.revoke")}
              </Button>
            </li>
          ))}
        </ul>
      )}
      {revoke.isError ? (
        <Alert tone="danger" title={t("access.explorer.revokeFailed")}>
          {revoke.error instanceof ApiError ? (revoke.error.problem?.detail ?? revoke.error.message) : t("app.error.generic")}
        </Alert>
      ) : null}
      <ConfirmDialog
        open={revoking !== null}
        onOpenChange={(open) => (open ? undefined : setRevoking(null))}
        title={t("access.explorer.revokeTitle", { client: revoking ?? "" })}
        description={t("access.explorer.revokeLead")}
        confirmLabel={t("access.explorer.revoke")}
        pending={revoke.isPending}
        onConfirm={() => revoking && revoke.mutate(revoking)}
      />
    </section>
  );
}

import { RowActions } from "../../components/ui/RowActions";
import { useEffect, useRef, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ManifestGrid } from "../../components/grid/ManifestGrid";
import { api, ApiError, unwrap } from "../../api/client";
import { proposeChecked } from "../../api/proposal";
import { isChange } from "../../api/manifest";
import type { Change, ResourceProposal } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import { EditResourceAction } from "../../components/EditResourceDialog";
import { RecordLink } from "../../components/RecordLink";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import { CkanAccessPanel } from "./CkanAccessPanel";
import { PublishDatasetDialog, useCatalogueFacets } from "../catalogue/PublishDataset";
import { Combobox } from "../../components/pickers/Combobox";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { usePermissions } from "../../api/permissions";
import type { components } from "../../api/schema";
import {
  Alert,
  Badge,
  Button,
  Card,
  EmptyState,
  ExternalLink,
  Field,
  Input,
  PageFailed,
  PageHeader,
} from "../../components/ui";

type CkanStatus = components["schemas"]["CkanStatus"];
type PublicationStatus = components["schemas"]["PublicationStatus"];
type ActivityEvent = components["schemas"]["ActivityEvent"];

export function ckanStatusKey(project: string) {
  return ["projects", project, "ckan", "status"] as const;
}

/**
 * The CKAN view of one project: which catalogues it can publish to, and what each endpoint
 * becomes in them (T-0319, EP-62…EP-67).
 *
 * The API token is never a field of this form. A `CkanInstance` names a `secretRef` and the
 * Portal refuses a manifest carrying a literal credential (MF-24, CC-06), so what a steward
 * enters here is the name of the secret the operator loaded into the secret store; the value
 * itself never passes through a browser and never reaches Git (EP-67).
 */
export function CkanPage({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [change, setChange] = useState<Change | null>(null);

  // What the last runs did to each dataset: the reconciler says it in the activity feed, one
  // event per change or failure, each naming its Endpoint (OPS-48, T-3091). Someone the feed is
  // closed to sees the publications without it.
  const lastRuns = useQuery({
    queryKey: ["projects", project, "activity", "catalogue.published"],
    retry: false,
    queryFn: async () => {
      const page = await unwrap(
        await api.GET("/api/v1/projects/{project}/activity", {
          params: { path: { project }, query: { kind: "catalogue.published", limit: 200 } },
        }),
      );
      return lastRunByEndpoint(page.items);
    },
  });

  const status = useQuery({
    queryKey: ckanStatusKey(project),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/ckan/status", {
          params: { path: { project } },
        }),
      ),
  });

  const create = useMutation({
    mutationFn: async (instance: InstanceDraft) => {
      const body = {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "CkanInstance",
        metadata: { name: instance.name, namespace: project },
        spec: {
          url: instance.url,
          ...(instance.organizationDefault
            ? { organizationDefault: instance.organizationDefault }
            : {}),
          apiTokenRef: { name: instance.secretName.trim(), key: instance.secretKey.trim() || "apiToken" },
        },
      };
      return proposeChecked(project, "ckaninstances", body as ResourceProposal, true);
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: ckanStatusKey(project) });
    },
  });

  return (
    <section aria-label={t("ckan.title")} className="space-y-8">
      <PageHeader title={t("ckan.title")} description={t("ckan.intro")} />

      {change ? <ChangeNotice change={change} project={project} /> : null}
      {create.error ? (
        <Alert role="alert" tone="danger">
          {create.error instanceof ApiError
            ? (create.error.problem?.detail ?? create.error.message)
            : t("app.error.generic")}
        </Alert>
      ) : null}
      {/* Without this the page answered a failed `/ckan/status` with its empty state: it told a
          steward as a fact that the project has no catalogue and publishes nothing, and the
          next move is to propose a catalogue that is already there (T-1765). */}
      {status.isError ? (
        <PageFailed
          error={status.error}
          onRetry={() => {
            void status.refetch();
          }}
        >
          {t("ckan.statusFailed", {
            reason:
              status.error instanceof ApiError
                ? (status.error.problem?.detail ?? status.error.message)
                : t("app.error.generic"),
          })}
        </PageFailed>
      ) : null}

      <Instances
        // A proposal that came back clears the form; a refusal keeps every word of it. The key
        // is the change's own name, so the reset happens on success and only on success.
        key={change?.metadata.name ?? "draft"}
        project={project}
        instances={status.data?.instances ?? []}
        loading={status.isLoading}
        failed={status.isError}
        onSubmit={(draft) => create.mutate(draft)}
        submitting={create.isPending}
      />
      {(status.data?.instances ?? []).length > 0 ? (
        <section aria-labelledby="ckan-access" className="space-y-3">
          <h2 id="ckan-access" className="text-lg font-semibold">
            {t("ckan.access.sectionTitle")}
          </h2>
          {(status.data?.instances ?? []).map((instance) => (
            <CkanAccessPanel
              key={instance.name}
              project={project}
              instance={instance.name}
              known={(status.data?.instances ?? []).map((known) => known.name)}
            />
          ))}
        </section>
      ) : null}
      <Publications
        project={project}
        hasCatalogue={(status.data?.instances ?? []).length > 0}
        publications={status.data?.publications ?? []}
        loading={status.isLoading}
        failed={status.isError}
        lastRuns={lastRuns.data}
      />
    </section>
  );
}

export interface InstanceDraft {
  name: string;
  url: string;
  organizationDefault: string;
  secretName: string;
  secretKey: string;
}

const EMPTY_DRAFT: InstanceDraft = {
  name: "",
  url: "https://",
  organizationDefault: "",
  secretName: "",
  secretKey: "apiToken",
};

/** A Kubernetes Secret's name: what the secret store accepts (a DNS subdomain). */
const SECRET_NAME = /^[a-z0-9]([-a-z0-9.]{0,251}[a-z0-9])?$/;
/**
 * A value that reads as a CKAN API token rather than a secret's name: a JWT (CKAN 2.9+) or the
 * UUID of a legacy API key. Refused with its own sentence, so a token pasted into the name field
 * never travels into a proposal and a Git commit (T-3215).
 */
const LOOKS_LIKE_TOKEN = /^eyJ|^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
type Fault = "name" | "url" | "secretName" | "secretKey";

/** What the form itself refuses, before anything is proposed (UI-04). */
export function faults(draft: InstanceDraft): Partial<Record<Fault, string>> {
  const found: Partial<Record<Fault, string>> = {};
  if (!/^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/.test(draft.name)) found.name = "ckan.instances.nameInvalid";
  // `type="url"` accepts `javascript:alert(1)` and `file:///etc/passwd`: the browser checks the
  // shape of a URL and not its scheme. A catalogue is fetched over HTTP by the reconciler, so
  // anything else is refused here rather than by a reconciler the steward cannot see (T-1765).
  if (!/^https?:\/\/[^\s/]+/i.test(draft.url.trim())) found.url = "ckan.instances.urlInvalid";
  const secret = draft.secretName.trim();
  if (secret === "") found.secretName = "ckan.instances.secretRequired";
  else if (LOOKS_LIKE_TOKEN.test(secret)) found.secretName = "ckan.instances.secretIsToken";
  else if (!SECRET_NAME.test(secret)) found.secretName = "ckan.instances.secretInvalid";
  if (!/^[-._a-zA-Z0-9]+$/.test(draft.secretKey.trim())) found.secretKey = "ckan.instances.secretKeyInvalid";
  return found;
}

function Instances({
  project,
  instances,
  loading,
  failed,
  onSubmit,
  submitting,
}: {
  project: string;
  instances: CkanStatus["instances"];
  loading: boolean;
  /** The catalogue list could not be read, so "none" is not a thing this section may say. */
  failed: boolean;
  onSubmit: (draft: InstanceDraft) => void;
  submitting: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const permissions = usePermissions(project);
  // A role that may not propose a catalogue reads why, not a form whose submit only refuses (T-4997).
  const mayPropose = permissions.data === undefined || permissions.can("CkanInstance", "propose");
  const [draft, setDraft] = useState<InstanceDraft>(EMPTY_DRAFT);
  const [shown, setShown] = useState<Partial<Record<Fault, string>>>({});
  // The CKAN organizations the Portal's catalogue lists; a new instance's own may be typed (T-4948).
  const facets = useCatalogueFacets();
  const fields = useRef<Record<string, HTMLInputElement | null>>({});
  // A second press while the first proposal is in flight. `submitting` arrives with the render
  // after the mutation starts, which is one render too late for a double click: measured, two
  // clicks sent two proposals and the project got two catalogues (T-1765, UI-48).
  const sending = useRef(false);
  useEffect(() => {
    if (!submitting) sending.current = false;
  }, [submitting]);

  return (
    <section aria-labelledby="ckan-instances" className="space-y-3">
      <h2 id="ckan-instances" className="text-lg font-semibold">
        {t("ckan.instances.title")}
      </h2>
      {loading ? <p role="status">{t("app.loading")}</p> : null}
      {!loading && !failed && instances.length === 0 ? (
        <EmptyState title={t("ckan.instances.empty")} description={t("ckan.instances.emptyHint")} icon="ckan" />
      ) : null}
      {instances.length > 0 ? (
        <ManifestGrid<(typeof instances)[number]>
          caption={t("ckan.instances.title")}
          storageKey="ckan-instances"
          searchLabel={t("grid.searchIn", { list: t("ckan.instances.title") })}
          rows={instances}
          rowKey={(instance) => instance.name}
          // The section draws its own loading line and failure; the grid gets the rows read.
          query={{ isPending: false, isError: false, error: null, refetch: () => undefined }}
          name={{
            id: "name",
            label: t("ckan.instances.name"),
            text: (instance) => instance.name,
            // The kind's own edit address: the same form as the row's Edit (T-2875).
            cell: (instance) => <RecordLink project={project} plural="ckaninstances" name={instance.name} />,
          }}
          columns={[
            {
              id: "url",
              label: t("ckan.instances.url"),
              text: (instance) => instance.url,
              cell: (instance) => (
                <ExternalLink href={instance.url} hideIcon className="hover:no-underline">
                  {instance.url}
                </ExternalLink>
              ),
            },
            {
              id: "organization",
              label: t("ckan.instances.organization"),
              filter: true,
              text: (instance) => instance.organizationDefault ?? "",
              cell: (instance) => instance.organizationDefault ?? "—",
            },
            { id: "tokenRef", label: t("ckan.instances.tokenRef"), text: (instance) => instance.apiTokenRef, cell: (instance) => <span className="font-mono">{instance.apiTokenRef}</span> },
          ]}
          actions={(instance) => (
            <RowActions label={instance.name} actions={[]} primary={<span className="inline-flex items-center gap-1.5">
              <EditResourceAction target={{ project, kind: "CkanInstance", plural: "ckaninstances", name: instance.name }} />
              <DeleteResourceAction target={{ project, kind: "CkanInstance", plural: "ckaninstances", name: instance.name }} />
            </span>} />
          )}
          empty={<EmptyState bare title={t("ckan.instances.empty")} description={t("ckan.instances.emptyHint")} icon="ckan" />}
        />
      ) : null}

      {!mayPropose ? (
        <Alert role="status" tone="info">
          {t("form.mayNotCreate")}
        </Alert>
      ) : (
        <form
          aria-labelledby="ckan-instance-new"
          className="max-w-4xl space-y-2 border-t border-border pt-4"
          noValidate
          onSubmit={(event) => {
            event.preventDefault();
            if (sending.current || submitting) return;
            const found = faults(draft);
            setShown(found);
            const first = (["name", "url", "secretName", "secretKey"] as const).find((field) => found[field]);
            if (first) {
              fields.current[first]?.focus();
              return;
            }
            // Nothing is cleared here: what the steward typed survives a refusal, and a proposal
            // that came back remounts this form from the page above (T-1765).
            sending.current = true;
            onSubmit(draft);
          }}
        >
          <h3 id="ckan-instance-new" className="text-body font-semibold text-fg">
            {t("ckan.instances.propose")}
          </h3>
          <Field
            id="ckan-instance-name"
            label={t("ckan.instances.name")}
            description={t("ckan.instances.nameHelp")}
            required
            errors={shown.name ? [t(shown.name)] : undefined}
          >
            <Input
              id="ckan-instance-name"
              ref={(node) => {
                fields.current.name = node;
              }}
              value={draft.name}
              onChange={(event) => setDraft({ ...draft, name: event.target.value })}
            />
          </Field>
          <Field
            id="ckan-instance-url"
            label={t("ckan.instances.url")}
            description={t("ckan.instances.urlHelp")}
            required
            errors={shown.url ? [t(shown.url)] : undefined}
          >
            <Input
              id="ckan-instance-url"
              ref={(node) => {
                fields.current.url = node;
              }}
              type="url"
              value={draft.url}
              placeholder="https://opendata.example.org"
              onChange={(event) => setDraft({ ...draft, url: event.target.value })}
            />
          </Field>
          <Field
            id="ckan-instance-org"
            label={t("ckan.instances.organization")}
            description={t("ckan.instances.organizationHelp")}
          >
            <Combobox
              id="ckan-instance-org"
              labelled
              label={t("ckan.instances.organization")}
              value={draft.organizationDefault ? [draft.organizationDefault] : []}
              onChange={(values) => setDraft({ ...draft, organizationDefault: values[0] ?? "" })}
              options={(facets.data?.publisher ?? []).map((org) => ({
                value: org.value,
                label: org.label,
                detail: org.value,
                group: t("ckan.instances.organizationsKnown"),
              }))}
              loading={facets.isPending}
              empty={t("ckan.instances.organizationNone")}
              create={{
                label: (name) => t("picker.createNamed", { name }),
                onCreate: (name) => setDraft({ ...draft, organizationDefault: name.trim().toLowerCase() }),
              }}
            />
          </Field>
          <Field
            id="ckan-instance-secret"
            label={t("ckan.instances.tokenRef")}
            description={t("ckan.instances.tokenHelp")}
            required
            errors={shown.secretName ? [t(shown.secretName)] : undefined}
          >
            <Input
              id="ckan-instance-secret"
              ref={(node) => {
                fields.current.secretName = node;
              }}
              value={draft.secretName}
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => setDraft({ ...draft, secretName: event.target.value })}
            />
          </Field>
          <Field
            id="ckan-instance-secret-key"
            label={t("ckan.instances.secretKey")}
            description={t("ckan.instances.secretKeyHelp")}
            required
            errors={shown.secretKey ? [t(shown.secretKey)] : undefined}
          >
            <Input
              id="ckan-instance-secret-key"
              ref={(node) => {
                fields.current.secretKey = node;
              }}
              value={draft.secretKey}
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => setDraft({ ...draft, secretKey: event.target.value })}
            />
          </Field>
          <Button type="submit" variant="primary" loading={submitting}>
            {t("ckan.instances.propose")}
          </Button>
        </form>
      )}
    </section>
  );
}

/** The newest catalogue event of each Endpoint, by name; the feed lists newest first. */
export function lastRunByEndpoint(events: ActivityEvent[]): Record<string, ActivityEvent> {
  const found: Record<string, ActivityEvent> = {};
  for (const event of events) {
    const object = (event.details as { object?: unknown } | null | undefined)?.object;
    if (typeof object !== "string" || !object.startsWith("endpoints/")) continue;
    const endpoint = object.slice("endpoints/".length);
    if (!found[endpoint] || found[endpoint].time < event.time) found[endpoint] = event;
  }
  return found;
}

function Publications({
  project,
  hasCatalogue,
  publications,
  loading,
  failed,
  lastRuns,
}: {
  project: string;
  /** Publishing needs a catalogue to land in; without one the button says so (T-3215). */
  hasCatalogue: boolean;
  publications: PublicationStatus[];
  loading: boolean;
  failed: boolean;
  lastRuns?: Record<string, ActivityEvent>;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const time = new Intl.DateTimeFormat(i18n.language, { dateStyle: "short", timeStyle: "short" });
  const [publishing, setPublishing] = useState(false);
  return (
    <section aria-labelledby="ckan-publications" className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 id="ckan-publications" className="text-lg font-semibold">
          {t("ckan.publications.title")}
        </h2>
        {/* The one-step flow of the endpoint page, started here with its endpoint picker (EP-83). */}
        <PermissionGuard project={project} kind="Endpoint" verb="propose">
          <Button
            onClick={() => setPublishing(true)}
            disabled={!hasCatalogue}
            disabledReason={t("ckan.publications.needsCatalogue")}
          >
            {t("catalogue.publish.open")}
          </Button>
        </PermissionGuard>
      </div>
      <PublishDatasetDialog project={project} open={publishing} onOpenChange={setPublishing} />
      {loading ? <p role="status">{t("app.loading")}</p> : null}
      {!loading && !failed && publications.length === 0 ? (
        <EmptyState
          title={t("ckan.publications.empty")}
          description={t("ckan.publications.emptyHint")}
          icon="share"
        />
      ) : null}
      <ul className="space-y-4">
        {publications.map((publication) => (
          <li key={publication.endpoint}>
            <Card padding="sm">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-mono font-semibold">{publication.endpoint}</span>
                <StatusChip publication={publication} />
                {publication.phase ? <LifecycleBadge kind="phase" value={publication.phase} /> : null}
                {publication.datasetUrl ? (
                  <ExternalLink
                    href={publication.datasetUrl}
                    hideIcon
                    className="hover:no-underline"
                  >
                    {publication.dataset}
                  </ExternalLink>
                ) : (
                  <span className="font-mono">{publication.dataset}</span>
                )}
              </div>
              {(() => {
                const last = lastRuns?.[publication.endpoint];
                if (!last) return null;
                const failedRun = last.severity === "error";
                return (
                  <p
                    className={`mt-1 text-sm [overflow-wrap:anywhere] ${failedRun ? "text-danger" : "text-fg-muted"}`}
                    data-testid="ckan-last-run"
                  >
                    {t(failedRun ? "ckan.publications.lastFailed" : "ckan.publications.lastChange", {
                      time: time.format(new Date(last.time)),
                      summary: last.summary,
                    })}
                  </p>
                );
              })()}
              <ul className="mt-2 flex flex-wrap gap-2">
                {publication.resources.map((resource) => (
                  <li key={resource.url}>
                    <ExternalLink
                      href={resource.url}
                      hideIcon
                      className="rounded border border-border px-2 py-1 text-sm hover:no-underline"
                    >
                      {resource.name && resource.name !== resource.format
                        ? `${resource.name} (${resource.format})`
                        : resource.format}
                    </ExternalLink>
                  </li>
                ))}
                {/* The theme draws the map and the explorer on the dataset page itself
                    (T-4639, T-5059), so that page is where both open. */}
                {publication.datasetUrl ? (
                  <li>
                    <ExternalLink
                      href={publication.datasetUrl}
                      hideIcon
                      className="rounded border border-border px-2 py-1 text-sm hover:no-underline"
                    >
                      {t("ckan.publications.mapAndTable")}
                    </ExternalLink>
                  </li>
                ) : null}
              </ul>
            </Card>
          </li>
        ))}
      </ul>
    </section>
  );
}

/** Published, or the reason it is not: a dangling instance is the usual one. */
function StatusChip({ publication }: { publication: PublicationStatus }): JSX.Element {
  const { t } = useTranslation();
  const missing = publication.instanceMissing;
  return (
    <Badge tone={missing ? "danger" : "neutral"}>
      {missing
        ? t("ckan.publications.missingInstance", { instance: publication.instance })
        : t("ckan.publications.publishedTo", { instance: publication.instance })}
    </Badge>
  );
}

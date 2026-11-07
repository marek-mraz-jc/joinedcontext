import { useDebounced } from "../../components/pickers/organizationModels";
import { useCallback, useState } from "react";
import type { JSX, ReactNode } from "react";
import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { EndpointAccessRequest } from "./AccessRequest";
import { ShareFlow } from "./ShareFlow";
import { AccessRequestInbox } from "../pipelines/AccessRequests";
import { ConnectAssistant } from "../../components/endpoints/ConnectAssistant";
import { QueryCard } from "../../components/endpoints/QueryCard";
import { ApiError, api, queryKeys, unwrap, whilePending } from "../../api/client";
import { asManifests, isChange, localized, refName } from "../../api/manifest";
import type { Change, ResourceProposal } from "../../api/manifest";
import { proposeBundle, proposeChecked } from "../../api/proposal";
import { ChangeNotice } from "../../components/ChangeNotice";
import type { Manifest } from "../../api/manifest";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import { SaveAsResourceAction } from "../../components/SaveAsDialog";
import { WorkOnCopyAction } from "../../components/WorkOnCopyDialog";
import { ExportButton } from "../../components/export/ExportButton";
import { EndpointQuality } from "./EndpointQuality";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { RowActions } from "../../components/ui/RowActions";
import type { RowAction } from "../../components/ui/RowActions";
import { usePermissions } from "../../api/permissions";
import {
  EndpointLink,
  REPRESENTATION_PATHS,
  endpointUrl,
  representationUrl,
  servedRepresentations,
  useCatalogueLinks,
} from "../../components/endpoints/links";
import { useBranding } from "../../branding";
import { spaceOf } from "../../components/endpoints/sharing";
import { bindingOf, spaceSegment } from "../../components/endpoints/policyBinding";
import { PolicyGrants } from "../../components/endpoints/PolicyGrants";
import { CatalogSection } from "./CatalogSection";
import { VisualizationsSection } from "./VisualizationsSection";
import { EndpointUsageSection } from "../../components/endpoints/UsageSection";
import { AccessMatrix } from "../../components/endpoints/AccessMatrix";
import { EndpointReaders } from "../organization/DataAccess";
import { FilterProof, classesWithSlots } from "./FilterProof";
import { filterSlotsOf, useModelSource } from "../../components/entities/filters";
import { parseModel } from "../models/linkml";
import type { CatalogManifest } from "./catalog";
import { EndpointDataView } from "../../components/entities/EndpointDataView";
import type { DataLink } from "../../components/entities/EndpointDataView";
import { PublishDatasetDialog } from "../catalogue/PublishDataset";
import {
  Alert,
  Badge,
  Button,
  Checkbox,
  Field,
  Icon,
  InlineError,
  Input,
  PageHeader,
  safeHref,
  Select,
  SourceLink,
} from "../../components/ui";
import { ResourcePageFailed } from "../../components/ui/PageState";
import { andQ, areaQuery, queryFromFilters, ringOfBounds } from "@joinedcontext/sdk";
import type { FilterOp } from "@joinedcontext/sdk";

/**
 * One endpoint, the whole window (T-2281, EP-51, UI-26, UI-61).
 *
 * The endpoints table had a cramped cell per aspect and every action beside it, so the settings that
 * decide what a city publishes were read three words at a time. This page is the endpoint's own: what
 * it is, what it answers, what it filters out before anybody sees a thing, who may call it, and what
 * it limits — each section labelled, in the order a person asks the questions.
 *
 * What is NOT here, and why: `EndpointSpec` is `deny_unknown_fields`, so the page shows the fields the
 * manifest actually has. **Filtering is not one of them** — a `q`, a `scopeQ` or an area lives on the
 * named `ModelProjection` the endpoint references (`spec.projectionRef`, MP-01), which is why this page
 * reads the filter from there and says so. There is likewise no `paused`/`retiring` field on an
 * endpoint: `status.phase` is the platform's own and never written from a form, so this page paints no
 * pause button that nothing behind it could honour. Deleting is how an endpoint stops (EP-89, T-2286),
 * and the last section says so and opens that deletion.
 */
export function EndpointPage({
  project,
  name,
}: {
  project: string;
  name: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const { domain } = useBranding();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const navigate = useNavigate();
  // The data view's type, query and order live in the address, so a link opens what was seen.
  const search = useSearch({ strict: false }) as { dataType?: string; dataQ?: string; dataSort?: string };
  const dataLink: DataLink = { type: search.dataType, q: search.dataQ, sort: search.dataSort };
  const writeDataLink = useCallback(
    (next: DataLink) => {
      if (next.type === search.dataType && next.q === search.dataQ && next.sort === search.dataSort) return;
      void navigate({
        to: ".",
        search: (previous: Record<string, unknown>) => ({ ...previous, dataType: next.type, dataQ: next.q, dataSort: next.sort }),
        replace: true,
      });
    },
    [navigate, search.dataType, search.dataQ, search.dataSort],
  );
  const permissions = usePermissions(project);
  // An action a role cannot take stays in the menu with its reason; UI-44 does not stop at a menu's
  // edge (T-2279).
  const denied = (verb: "propose" | "delete") =>
    permissions.can("Endpoint", verb) ? undefined : t("permissions.denied", { verb, kind: "Endpoint" });
  const [openAction, setOpenAction] = useState<"saveAs" | "workOnCopy" | "delete" | null>(null);
  const [publishing, setPublishing] = useState(false);

  const endpoint = useQuery({
    queryKey: queryKeys.resource(project, "endpoints", name),
    retry: false,
    // A change on its way polls until it lands (T-1392).
    refetchInterval: whilePending,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural: "endpoints", name } },
        }),
      ),
  });

  const policies = useQuery({
    queryKey: queryKeys.list(project, "policies"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "policies" } },
        }),
      ),
  });

  const spaces = useQuery({
    queryKey: queryKeys.list(project, "spaces"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "spaces" } },
        }),
      ),
  });

  const projections = useQuery({
    queryKey: queryKeys.list(project, "projections"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "projections" } },
        }),
      ),
  });

  // The space's model, for an endpoint that names no projection yet: the filter editor starts from
  // its classes and slots, and a proposed filter creates the projection (T-2776, MP-01).
  const models = useQuery({
    queryKey: queryKeys.list(project, "datamodels"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "datamodels" } },
        }),
      ),
  });
  const endpointSpace = endpoint.data ? spaceOf(endpoint.data as Manifest) : undefined;
  const spaceModel = asManifests(models.data?.items ?? []).find(
    (model) => (model.spec as { contextSpaceRef?: string }).contextSpaceRef === endpointSpace,
  );
  const modelSource = useModelSource(project, spaceModel);
  const catalogueLink = useCatalogueLinks([project]);

  if (endpoint.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }
  if (endpoint.isError) {
    return (
      <ResourcePageFailed
        title={name}
        description={t("endpoints.lead")}
        error={endpoint.error}
        onRetry={() => {
          void endpoint.refetch();
        }}
        back={
          <Link
            to="/projects/$project/$plural"
            params={{ project, plural: "endpoints" }}
            className="text-sm text-primary-soft-fg underline hover:no-underline focus:outline-none focus:ring-2 focus:ring-border-focus"
          >
            {t("endpoints.page.back")}
          </Link>
        }
      />
    );
  }

  const manifest = endpoint.data as Manifest;
  const spec = manifest.spec as EndpointSpec;
  const catalogue = catalogueLink(project, manifest);
  const slug = spec.slug ?? "";
  const space = spaceOf(manifest);
  const projectionName = refName(spec.projectionRef);
  const projection = projectionName
    ? asManifests(projections.data?.items ?? []).find((p) => p.metadata.name === projectionName)
    : undefined;
  const target = { project, kind: "Endpoint", plural: "endpoints", name } as const;
  // The projection the filter editor edits: the one the endpoint names, or one drawn from the
  // space's model that a proposed filter creates.
  const filtered =
    projection ??
    (space && spaceModel && modelSource
      ? projectionFromModel(
          project,
          name,
          space,
          spaceModel,
          modelSource,
          asManifests(projections.data?.items ?? []).map((one) => one.metadata.name),
        )
      : undefined);

  // The actions that open a dialog are rendered beside the menu, never inside it: the menu unmounts
  // its content when it closes and would take an open dialog with it (T-2279).
  const actions: RowAction[] = [
    ...(slug
      ? [
          {
            key: "copy",
            label: t("endpoints.copyUrl"),
            onSelect: () => {
              void navigator.clipboard?.writeText(endpointUrl(slug, "")).catch(() => undefined);
            },
          },
        ]
      : []),
    {
      key: "saveAs",
      label: t("saveAs.button"),
      onSelect: () => setOpenAction("saveAs"),
      disabledReason: denied("propose"),
    },
    {
      key: "workOnCopy",
      label: t("workspaces.open.action"),
      onSelect: () => setOpenAction("workOnCopy"),
    },
    {
      key: "delete",
      label: t("resourceDelete.button"),
      tone: "danger",
      onSelect: () => setOpenAction("delete"),
      disabledReason: denied("delete"),
    },
  ];

  return (
    <div className="space-y-6">
      {/* One header row: the title and id, then every action (owner 2026-10-30). */}
      <div className="flex flex-wrap items-center justify-between gap-4">
        <PageHeader
          title={localized(manifest.metadata.title, locale, name)}
          description={<span className="font-mono">{name}</span>}
        />
      <div className="flex flex-wrap items-center gap-2">
        <LifecycleBadge kind="phase" value={manifest.status?.phase} />
        {manifest.status?.sourceUrl ? (
          <SourceLink href={manifest.status.sourceUrl} label={t("spaces.field.source")} />
        ) : null}
        {/* What the person may do through this Endpoint, the Data access table opened on it (T-5593). */}
        {slug ? (
          <Link
            to="/projects/$project/settings/$tab"
            params={{ project, tab: "access" }}
            search={{ endpoint: name }}
            className="text-sm text-primary-soft-fg underline hover:no-underline focus:outline-none focus:ring-2 focus:ring-border-focus"
          >
            {t("projectSettings.access.title")}
          </Link>
        ) : null}
        <PermissionGuard project={project} kind="Endpoint" verb="propose">
          <Button
            onClick={() => {
              // The edit form is a page at its own address (T-2474).
              void navigate({
                to: "/projects/$project/$plural/$name/edit",
                params: { project, plural: "endpoints", name },
              });
            }}
          >
            {t("endpoints.page.change")}
          </Button>
        </PermissionGuard>
        {/* The one-step publish flow (EP-83): drafts the catalogue block from the model and the
            organization, and proposes it with the publication as one Change. */}
        <PermissionGuard project={project} kind="Endpoint" verb="propose">
          <Button icon={<Icon name="ckan" className="size-4" />} onClick={() => setPublishing(true)}>
            {t("catalogue.publish.open")}
          </Button>
        </PermissionGuard>
        <PublishDatasetDialog project={project} endpoint={name} open={publishing} onOpenChange={setPublishing} />
        <ExportButton
          project={project}
          target={{ plural: "endpoints", name }}
          label={t("export.action")}
          size="sm"
        />
        <RowActions label={localized(manifest.metadata.title, locale, name)} actions={actions} />
        <SaveAsResourceAction
          target={target}
          trigger={false}
          open={openAction === "saveAs"}
          onOpenChange={(next) => setOpenAction(next ? "saveAs" : null)}
        />
        <WorkOnCopyAction
          project={project}
          scope={{ kind: "resources", items: [{ kind: "Endpoint", name }] }}
          trigger={false}
          open={openAction === "workOnCopy"}
          onOpenChange={(next) => setOpenAction(next ? "workOnCopy" : null)}
        />
        <DeleteResourceAction
          target={target}
          trigger={false}
          open={openAction === "delete"}
          onOpenChange={(next) => setOpenAction(next ? "delete" : null)}
        />
      </div>
      </div>

      {/* What it answers, as one row of links (owner 2026-10-30). */}
      <ul className="flex flex-wrap gap-1" aria-label={t("endpoints.field.representations")}>
        {servedRepresentations(spec).map((rep) => (
          <li key={rep}>
            {slug && REPRESENTATION_PATHS[rep] ? (
              <EndpointLink href={representationUrl(slug, rep, domain)}>{rep}</EndpointLink>
            ) : (
              <Badge mono>{rep}</Badge>
            )}
          </li>
        ))}
      </ul>

      {/* The rows themselves, read with the person's own session through the Endpoint, so a steward
          sees what their grants show and nothing more (T-5082). Only a Live endpoint answers. */}
      {slug && (manifest.status?.phase ?? "").toLowerCase() === "live" ? (
        <Section title={t("endpoints.data.section")} lead={t("endpoints.data.sectionLead")}>
          <EndpointDataView
            project={project}
            slug={slug}
            hidden={spec.projection?.hiddenAttributes ?? []}
            space={space}
            link={dataLink}
            onLink={writeDataLink}
            csv={servedRepresentations(spec).includes("csv")}
          />
        </Section>
      ) : null}

      {/* This Endpoint's query card and MCP address, then the one connector for every Endpoint (EP-113). */}
      {slug && servedRepresentations(spec).includes("mcp") ? (
        <>
          <QueryCard slug={slug} name={name} domain={domain} />
          <ConnectAssistant domain={domain} />
        </>
      ) : null}

      <Section title={t("endpoints.page.filtering")} lead={t("endpoints.page.filteringLead")}>
        {filtered ? (
          <>
            <FilterForm
              project={project}
              projection={filtered}
              creating={projection === undefined}
              endpoint={manifest}
              segment={space ? spaceSegment(project, space, asManifests(spaces.data?.items ?? [])) : ""}
              enums={modelSource ? enumsOfClasses(modelSource, classesOf(filtered)) : {}}
              geoProperties={geoPropertiesOf(modelSource, classesOf(filtered))}
              live={(manifest.status?.phase ?? "").toLowerCase() === "live"}
            />
            {projection ? (
              <p className="text-caption text-fg-muted">
                {t("endpoints.page.filterLivesOn")}{" "}
                <Link
                  to="/projects/$project/$plural"
                  params={{ project, plural: "projections" }}
                  className="text-primary-soft-fg underline hover:no-underline"
                >
                  {projectionName}
                </Link>
              </p>
            ) : (
              <p className="text-caption text-fg-muted">
                {t("endpoints.filterEditor.createsProjection", { name: filtered.metadata.name })}
              </p>
            )}
          </>
        ) : (
          <p className="text-sm text-fg-muted">{t("endpoints.page.noProjection")}</p>
        )}
      </Section>

      <Section title={t("endpoints.page.whoMayCall")}>
        <PolicyGrants
          project={project}
          binding={bindingOf(
            manifest,
            project,
            space,
            asManifests(policies.data?.items ?? []),
            asManifests(spaces.data?.items ?? []),
          )}
          pending={policies.isPending || spaces.isPending}
          failure={
            policies.isError || spaces.isError
              ? {
                  error: policies.error ?? spaces.error,
                  retry: () => {
                    if (policies.isError) void policies.refetch();
                    if (spaces.isError) void spaces.refetch();
                  },
                }
              : undefined
          }
        />
        <Link
          to="/projects/$project/$plural"
          params={{ project, plural: "policies" }}
          className="text-caption text-primary-soft-fg underline hover:no-underline"
        >
          {t("endpoints.page.openPolicies")}
        </Link>
      </Section>

      {/* Who reads it in plain words, for the Organization's administrators (ADR-N-067 §2.5, T-6412). */}
      <Section title={t("dataAccess.endpointTitle")} lead={t("dataAccess.endpointLead")}>
        <EndpointReaders slug={slug} audience={spec.audience} />
      </Section>

      {/* A person's own ask for a read here, decided by the space's owners (EP-55, T-5615). */}
      <Section title={t("endpoints.request.section")} lead={t("endpoints.request.sectionLead")}>
        <EndpointAccessRequest project={project} name={name} />
      </Section>

      {/* Who may do what, for every principal at once, as the gateway answers it (T-5531). */}
      <Section title={t("endpoints.accessMatrix.title")} lead={t("endpoints.accessMatrix.lead")}>
        <AccessMatrix project={project} name={name} />
      </Section>

      {/* Sharing with another project, guided, with an end and a revoke (T-3275). */}
      <Section title={t("endpoints.shareFlow.section")} lead={t("endpoints.shareFlow.lead")}>
        <ShareFlow
          project={project}
          endpoint={manifest}
          space={space ?? ""}
          types={filtered ? classesOf(filtered) : []}
          slots={filtered ? slotsOf(filtered) : []}
          policies={asManifests(policies.data?.items ?? [])}
        />
      </Section>

      {/* Pipelines of other spaces asking for this one, for its owners (PL-91, T-4863). */}
      <PermissionGuard project={project} kind="ContextSpace" verb="approve">
        <Section title={t("pipelines.access.inbox")} lead={t("pipelines.access.inboxLead")}>
          <AccessRequestInbox project={project} space={space ?? ""} />
        </Section>
      </PermissionGuard>

      <Section title={t("endpoints.page.traffic")}>
        <Facts>
          <Fact label={t("endpoints.field.rateLimits")}>
            {typeof spec.rateLimits?.requestsPerMinute === "number" ? (
              <span>
                {t("endpoints.page.perMinute", { count: spec.rateLimits.requestsPerMinute })}
                {typeof spec.rateLimits.burst === "number"
                  ? ` · ${t("endpoints.page.burst", { count: spec.rateLimits.burst })}`
                  : ""}
              </span>
            ) : (
              <p className="text-caption text-fg-muted">{t("endpoints.page.noRateLimit")}</p>
            )}
          </Fact>
          <Fact label={t("endpoints.field.caching")}>
            {typeof spec.caching?.maxAgeSeconds === "number" ? (
              <span>{t("endpoints.page.maxAge", { count: spec.caching.maxAgeSeconds })}</span>
            ) : (
              <p className="text-caption text-fg-muted">{t("endpoints.page.noCaching")}</p>
            )}
          </Fact>
          <Fact label={t("endpoints.field.fileLimits")}>
            {spec.fileLimits?.maxFileRows || spec.fileLimits?.maxFileBytes ? (
              <ul className="space-y-0.5">
                {spec.fileLimits.maxFileRows ? (
                  <li>{t("endpoints.page.maxRows", { count: spec.fileLimits.maxFileRows })}</li>
                ) : null}
                {spec.fileLimits.maxFileBytes ? (
                  <li>{t("endpoints.page.maxBytes", { count: spec.fileLimits.maxFileBytes })}</li>
                ) : null}
              </ul>
            ) : (
              <p className="text-caption text-fg-muted">{t("endpoints.page.noFileLimits")}</p>
            )}
          </Fact>
        </Facts>
      </Section>

      <Section title={t("endpointUsage.title")} lead={t("endpointUsage.endpointLead")}>
        <EndpointUsageSection project={project} name={name} />
      </Section>

      {spec.publish?.ckan ? (
        <Section title={t("endpoints.page.publication")}>
          <Facts>
            <Fact label={t("endpoints.field.ckanInstance")}>
              <Badge mono>{refName(spec.publish.ckan.instanceRef)}</Badge>
            </Fact>
            {spec.publish.ckan.organization ? (
              <Fact label={t("endpoints.field.ckanOrganization")}>
                <Badge mono>{spec.publish.ckan.organization}</Badge>
              </Fact>
            ) : null}
            <Fact label={t("endpoints.field.catalogue")}>
              {catalogue ? (
                <a
                  href={safeHref(catalogue)}
                  target="_blank"
                  rel="noreferrer"
                  className="text-primary-soft-fg underline hover:no-underline"
                >
                  {spec.publish.ckan.name ?? name}
                </a>
              ) : (
                <Badge mono>{spec.publish.ckan.name ?? name}</Badge>
              )}
            </Fact>
          </Facts>
        </Section>
      ) : null}

      <EndpointQuality project={project} name={name} />

      <Section title={t("endpoints.page.catalog.title")} lead={t("endpoints.page.catalog.lead")}>
        <CatalogSection slug={slug} catalog={spec.catalog} audience={spec.audience ?? "project-list"} />
      </Section>

      <Section title={t("endpoints.page.visualizations.title")} lead={t("endpoints.page.visualizations.lead")}>
        <VisualizationsSection
          project={project}
          manifest={manifest}
          types={filtered ? classesOf(filtered) : []}
          columnsOf={(type) => filterSlotsOf(modelSource, type).map((slot) => slot.name)}
          served={servedRepresentations(spec)}
          canEdit={permissions.can("Endpoint", "propose")}
        />
      </Section>

      {/* An endpoint has no pause; deleting it through a change is how it stops (EP-89). */}
      <Section title={t("endpoints.page.stop.title")} lead={t("endpoints.page.stop.lead")}>
        <Button
          size="sm"
          variant="danger"
          disabled={denied("delete") !== undefined}
          disabledReason={denied("delete")}
          onClick={() => setOpenAction("delete")}
        >
          {t("endpoints.page.stop.delete")}
        </Button>
      </Section>
    </div>
  );
}

/**
 * The filter of the named projection, editable here (T-2281, the owner's own ask), proposed like any
 * other change.
 *
 * Four conditions is what the manifest holds — `q`, `scopeQ`, `geoQ`, `temporalQ` (MP-01) — and each
 * one only ever narrows what the endpoint publishes; there is no field here that could widen a grant.
 * An empty box removes the condition rather than storing an empty query, so "not narrowed" stays what
 * it says. T-2283 turns these four boxes into rows a person builds without knowing NGSI-LD, with the
 * count of what they match; the boxes are what the manifest can express today.
 */
/** A 409 on a filter proposal: another filter of the project already holds the name (F-UI2-013). */
const nameTaken = (error: unknown): boolean => error instanceof ApiError && error.status === 409;

function FilterForm({
  project,
  projection,
  creating,
  endpoint,
  segment,
  enums,
  geoProperties,
  live,
}: {
  project: string;
  projection: Manifest;
  /** The permissible values of each enum attribute of the model, by attribute (T-2706). */
  enums: Record<string, string[]>;
  /** The GeoProperties of the projected types, `location` first (T-4947). */
  geoProperties: string[];
  /** The projection does not exist yet: a proposal creates it with the endpoint that names it. */
  creating: boolean;
  /** The endpoint whose hidden attributes the editor holds; its slug is what the proof reads. */
  endpoint: Manifest;
  /** The `{space}` segment of the space's canonical surface (PF-84). */
  segment: string;
  live: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const spec = endpoint.spec as EndpointSpec;
  const slug = spec.slug ?? "";
  const stored = filterOf(projection);
  const storedClasses = classesWithSlots(projection);
  const storedHidden = spec.projection?.hiddenAttributes ?? [];
  const [draft, setDraft] = useState<Record<string, string>>(() =>
    Object.fromEntries(FILTER_KEYS.map((key) => [key, stored[key] ?? ""])),
  );
  const [served, setServed] = useState<string[]>(() => storedClasses.map((klass) => klass.name));
  const [hidden, setHidden] = useState<string[]>(storedHidden);
  const [changes, setChanges] = useState<Change[]>([]);

  const filter = Object.fromEntries(
    FILTER_KEYS.map((key) => [key, draft[key]?.trim() ?? ""]).filter(([, value]) => value !== ""),
  ) as Record<string, string>;
  const classes = storedClasses.filter((klass) => served.includes(klass.name));
  // A drawn projection holds every class and slot of the model and no condition, so it is touched
  // once it narrows something; a stored one, once it differs from what is stored.
  const projectionTouched =
    FILTER_KEYS.some((key) => (draft[key]?.trim() ?? "") !== (stored[key] ?? "")) ||
    classes.length !== storedClasses.length;
  const hiddenTouched = !sameSet(hidden, storedHidden);

  const propose = useMutation({
    mutationFn: async () => {
      if (creating && projectionTouched) {
        // The projection and the endpoint that names it are one proposal, checked together: the
        // endpoint alone would name a projection that does not exist yet (MF-23).
        const { projection: _stored, ...kept } = spec;
        void _stored;
        const rest = { ...endpoint };
        delete (rest as { status?: unknown }).status;
        const named = {
          ...rest,
          spec: {
            ...kept,
            projectionRef: { kind: "ModelProjection", name: projection.metadata.name },
            ...(hidden.length > 0 ? { projection: { hiddenAttributes: hidden } } : {}),
          },
        };
        const drawn = {
          ...projection,
          spec: {
            ...(projection.spec as Record<string, unknown>),
            classes,
            ...(Object.keys(filter).length > 0 ? { filter } : {}),
          },
        };
        return [await proposeBundle(project, [drawn, named])];
      }
      const proposed: unknown[] = [];
      if (projectionTouched) {
        // The status is the platform's to compute and never travels back (MF-04).
        const rest = { ...projection };
        delete (rest as { status?: unknown }).status;
        const body = {
          ...rest,
          spec: { ...(projection.spec as Record<string, unknown>), classes, filter },
        };
        if (Object.keys(filter).length === 0) {
          delete (body.spec as { filter?: unknown }).filter;
        }
        proposed.push(await proposeChecked(project, "projections", body as ResourceProposal, false));
      }
      if (hiddenTouched) {
        // Hidden attributes live on the endpoint itself (EP-61), so they are a change of their own.
        const rest = { ...endpoint };
        delete (rest as { status?: unknown }).status;
        const { projection: _stored, ...kept } = spec;
        void _stored;
        const body = {
          ...rest,
          spec: { ...kept, ...(hidden.length > 0 ? { projection: { hiddenAttributes: hidden } } : {}) },
        };
        proposed.push(await proposeChecked(project, "endpoints", body as ResourceProposal, false));
      }
      return proposed;
    },
    onSuccess: (results) => {
      setChanges(results.filter(isChange));
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "projections") });
      void queryClient.invalidateQueries({ queryKey: queryKeys.resource(project, "endpoints", endpoint.metadata.name) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project) });
    },
  });

  const faults = Object.fromEntries(
    FILTER_KEYS.map((key) => [key, filterFault(key, draft[key] ?? "")]),
  ) as Record<string, string | undefined>;
  const noType = classes.length === 0;
  const faulty = Object.values(faults).some(Boolean) || noType;
  const untouched = !projectionTouched && !hiddenTouched;
  const failure =
    propose.error instanceof ApiError
      ? (propose.error.problem?.detail ?? propose.error.message)
      : propose.error
        ? t("app.error.generic")
        : null;

  if (changes.length > 0) {
    return (
      <div className="space-y-2">
        {changes.map((change) => (
          <ChangeNotice key={change.metadata?.name ?? JSON.stringify(change)} change={change} project={project} />
        ))}
      </div>
    );
  }

  return (
    <div className="grid gap-6 2xl:grid-cols-[minmax(0,2fr)_minmax(0,3fr)]">
      <form
        className="space-y-3"
        aria-label={t("endpoints.filterEditor.label")}
        onSubmit={(event) => {
          event.preventDefault();
          propose.mutate();
        }}
      >
        <fieldset className="space-y-2" aria-describedby="filter-types-help">
          <legend className="text-caption font-medium">{t("endpoints.filterEditor.types")}</legend>
          <p id="filter-types-help" className="text-caption text-fg-muted">{t("endpoints.filterEditor.typesHelp")}</p>
          <div className="flex flex-wrap gap-x-4 gap-y-1">
            {storedClasses.map((klass) => (
              <Checkbox
                key={klass.name}
                id={`filter-type-${klass.name}`}
                label={klass.name}
                checked={served.includes(klass.name)}
                onChange={(event) =>
                  setServed(
                    event.target.checked
                      ? [...served, klass.name]
                      : served.filter((name) => name !== klass.name),
                  )
                }
              />
            ))}
          </div>
          {noType ? (
            <InlineError>
              {t("endpoints.filterEditor.noType")}
            </InlineError>
          ) : null}
        </fieldset>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          {FILTER_KEYS.map((key) => (
            <Field
              key={key}
              id={`filter-${key}`}
              label={t(`endpoints.filter.${key}`)}
              help={t(`endpoints.filter.${key}Help`)}
              errors={faults[key] ? [t(`endpoints.filter.fault.${faults[key] as string}`)] : undefined}
            >
              <Input
                id={`filter-${key}`}
                value={draft[key] ?? ""}
                placeholder={t("endpoints.page.filterEmpty")}
                aria-invalid={faults[key] ? true : undefined}
                onChange={(event) => setDraft({ ...draft, [key]: event.target.value })}
              />
            </Field>
          ))}
        </div>
        <MatchCount slug={slug} type={classesOf(projection)[0]} q={draft.q ?? ""} live={live} />
        <AreaFromBox geoProperties={geoProperties} onSet={(geoQ) => setDraft({ ...draft, geoQ })} />
        <ConditionBuilder
          attributes={slotsOf(projection)}
          enums={enums}
          onAdd={(term) =>
            setDraft({ ...draft, q: andQ(draft.q, term) ?? "" })
          }
        />
        <fieldset className="space-y-2" aria-describedby="filter-hidden-help">
          <legend className="text-caption font-medium">{t("endpoints.filterEditor.hidden")}</legend>
          <p id="filter-hidden-help" className="text-caption text-fg-muted">{t("endpoints.filterEditor.hiddenHelp")}</p>
          {classes.map((klass) => (
            <div key={klass.name} className="space-y-1">
              <p className="text-caption font-medium text-fg-muted">{klass.name}</p>
              <div className="flex flex-wrap gap-x-4 gap-y-1">
                {klass.slots.length === 0 ? (
                  <span className="text-caption text-fg-muted">{t("endpoints.filterEditor.identityOnly")}</span>
                ) : null}
                {klass.slots.map((slot) => (
                  <Checkbox
                    key={slot}
                    id={`filter-hide-${klass.name}-${slot}`}
                    label={slot}
                    checked={hidden.includes(slot)}
                    onChange={(event) =>
                      setHidden(
                        event.target.checked ? [...hidden, slot] : hidden.filter((name) => name !== slot),
                      )
                    }
                  />
                ))}
              </div>
            </div>
          ))}
        </fieldset>
        {/* Beside the button the person just pressed: the form is taller than a screen (F-UI2-013). */}
        {failure ? (
          <Alert tone="danger" role="alert">
            {failure}
            {nameTaken(propose.error) ? (
              <p className="mt-1">{t("endpoints.filterEditor.nameTaken")}</p>
            ) : null}
          </Alert>
        ) : null}
        <PermissionGuard project={project} kind="ModelProjection" verb="propose">
          <Button
            type="submit"
            disabled={untouched || faulty || propose.isPending}
            disabledReason={
              untouched
                ? t("endpoints.page.filterUntouched")
                : faulty
                  ? t("endpoints.page.filterFaulty")
                  : t("app.busyReason")
            }
          >
            {t("endpoints.page.filterPropose")}
          </Button>
        </PermissionGuard>
      </form>
      <FilterProof
        project={project}
        name={endpoint.metadata.name}
        slug={slug}
        segment={segment}
        live={live}
        draft={{ classes, filter, hiddenAttributes: hidden }}
      />
    </div>
  );
}

/**
 * A projection of every class of the space's model with every slot the class declares, as the
 * starting point of an endpoint's first filter (MP-01). It is named after the endpoint, or
 * `{endpoint}-filter` when a projection already has that name.
 *
 * A slot a class only inherits (`is_a` an imported `Entity`, whose `location` it carries) is not
 * listed: the write path checks a projection against the class's own slots (jc-core
 * `linkml_classes`) and would refuse it. The proof beside the editor strikes such an attribute
 * before anything is proposed, so the person sees what the first filter would stop serving.
 */
export function projectionFromModel(
  project: string,
  endpoint: string,
  space: string,
  model: Manifest,
  source: string,
  taken: string[],
): Manifest {
  const parsed = parseModel(source);
  const version = String((model.spec as { version?: string | number }).version ?? "1").split(".")[0] || "1";
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ModelProjection",
    metadata: {
      name: taken.includes(endpoint) ? `${endpoint}-filter` : endpoint,
      namespace: project,
      labels: { "joinedcontext.com/space": space },
    },
    spec: {
      contextSpaceRef: space,
      dataModelRef: { kind: "DataModel", name: model.metadata.name, version },
      classes: parsed.classes.map((klass) => ({
        name: klass.name,
        slots: filterSlotsOf(source, klass.name).map((slot) => slot.name),
      })),
    },
  } as Manifest;
}

/** The permissible values of every enum attribute of these classes, by attribute name. */
export function enumsOfClasses(source: string, classes: string[]): Record<string, string[]> {
  const found: Record<string, string[]> = {};
  for (const klass of classes) {
    for (const slot of filterSlotsOf(source, klass)) {
      if (slot.values && slot.values.length > 0) {
        found[slot.name] = slot.values;
      }
    }
  }
  return found;
}

/** Two lists holding the same names, in any order. */
function sameSet(left: string[], right: string[]): boolean {
  return left.length === right.length && left.every((name) => right.includes(name));
}


/** The results count header of a counted NGSI-LD read (CIM 009 6.3.13). */
const RESULTS_COUNT = "NGSILD-Results-Count";

async function countThrough(slug: string, params: URLSearchParams): Promise<number> {
  const response = await globalThis.fetch(
    new Request(endpointUrl(slug, `/ngsi-ld/v1/entities?${params.toString()}`), {
      headers: { Accept: "application/ld+json" },
    }),
  );
  if (!response.ok) {
    throw new ApiError(response.status, response.statusText || `HTTP ${response.status}`);
  }
  const raw = response.headers.get(RESULTS_COUNT);
  const count = raw === null ? Number.NaN : Number.parseInt(raw.trim(), 10);
  if (Number.isNaN(count) || count < 0) {
    throw new ApiError(502, "the endpoint answered no results count");
  }
  return count;
}

/**
 * How many entities the filter matches, of how many the endpoint answers (T-2283).
 *
 * A filter that matches nothing has to be visible before it is proposed — with a million entities in a
 * space, nobody can tell by reading a query string. Both counts are read **through this endpoint**, so
 * they are what the endpoint itself answers under the reader's own rights: no privileged path, and a
 * person who may not read the data sees the refusal, not a number they should not have.
 *
 * Only a Live endpoint answers at all, and NGSI-LD refuses a query with no selector, so this needs the
 * projection's own type; without either, it says so instead of showing a zero that would read as an
 * empty space.
 */
function MatchCount({
  slug,
  type,
  q,
  live,
}: {
  slug: string;
  type?: string;
  q: string;
  live: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  // Counted once the typing pauses, and the whole type once: a keystroke is no read (F-R2UI2-008).
  const asked = useDebounced(q.trim(), 400);
  const base = { type: type ?? "", limit: "1", count: "true" };
  const enabled = live && slug !== "" && type !== undefined;
  const total = useQuery({
    queryKey: ["endpoint", slug, "matches", type ?? "", ""],
    enabled,
    retry: false,
    queryFn: () => countThrough(slug, new URLSearchParams(base)),
  });
  const filtered = useQuery({
    queryKey: ["endpoint", slug, "matches", type ?? "", asked],
    enabled: enabled && asked !== "",
    retry: false,
    queryFn: () => countThrough(slug, new URLSearchParams({ ...base, q: asked })),
  });
  const counted = asked === "" ? total : filtered;

  if (!live || type === undefined) {
    return <p className="text-caption text-fg-muted">{t("endpoints.count.unavailable")}</p>;
  }
  if (total.isPending || counted.isPending) {
    return (
      <p role="status" className="text-caption text-fg-muted">
        {t("endpoints.count.counting")}
      </p>
    );
  }
  const failure = total.error ?? counted.error;
  if (failure) {
    const message =
      failure instanceof ApiError
        ? (failure.problem?.detail ?? failure.message)
        : t("app.error.generic");
    return (
      <p role="status" className="text-caption text-fg-muted">
        {t("endpoints.count.failed", { reason: message })}
      </p>
    );
  }
  const all = total.data;
  const matching = counted.data;
  return (
    <p role="status" className={matching === 0 ? "text-caption text-danger" : "text-caption text-fg-muted"}>
      {t("endpoints.count.matches", { matching, all })}
      {matching === 0 ? ` ${t("endpoints.count.none")}` : ""}
    </p>
  );
}

/**
 * What is wrong with one condition of the filter, in a sentence, or `undefined` (T-2283, UI-45).
 *
 * The server checks the manifest too, and it is the authority; this only spares a person a round trip on
 * the two mistakes the format makes easy. A scope is a path (`/Helsinki/Kamppi`, CIM 009 4.19), not free
 * text, and a `geoQ` carries `georel`, `geometry` and `coordinates` together or the gateway cannot read
 * any of it.
 */
export function filterFault(key: string, value: string): "scopePath" | "geoParts" | undefined {
  const written = value.trim();
  if (written === "") {
    return undefined;
  }
  if (key === "scopeQ") {
    // A scope path's segments may hold spaces ("/Banska Bystrica/Centrum"); what it may not do is start
    // without a slash or carry an empty segment, which is the mistake free text makes.
    const paths = written
      .split(/[;,|]/)
      .map((part) => part.trim())
      .filter((part) => part !== "");
    return paths.every((path) => /^\/[^/]+(\/[^/]+)*$/.test(path)) ? undefined : "scopePath";
  }
  if (key === "geoQ") {
    const parts = new Set(
      written
        .split(";")
        .map((part) => part.split("=")[0]?.trim())
        .filter((part): part is string => Boolean(part)),
    );
    return ["georel", "geometry", "coordinates"].every((part) => parts.has(part)) ? undefined : "geoParts";
  }
  return undefined;
}

/** `geoQ` as the manifest holds it: the parts of a geo query joined with `;` (MP-01, CIM 009 4.10). */
export function geoQueryString(query: {
  georel: string;
  geometry: string;
  coordinates: string;
  geoproperty: string;
}): string {
  return [
    `georel=${query.georel}`,
    `geometry=${query.geometry}`,
    `coordinates=${query.coordinates}`,
    `geoproperty=${query.geoproperty}`,
  ].join(";");
}

/** What is wrong with a box a person typed, or `undefined`. */
export function boxFault(box: Record<string, string>): "numbers" | "range" | "order" | undefined {
  const values = ["west", "south", "east", "north"].map((key) => Number(box[key]));
  if (values.some((value) => !Number.isFinite(value)) || ["west", "south", "east", "north"].some((k) => (box[k] ?? "").trim() === "")) {
    return "numbers";
  }
  const [west, south, east, north] = values;
  if (Math.abs(west) > 180 || Math.abs(east) > 180 || Math.abs(south) > 90 || Math.abs(north) > 90) {
    return "range";
  }
  return west < east && south < north ? undefined : "order";
}

/**
 * An area from a bounding box (T-2283, UI-72).
 *
 * The owner asked to share only what lies in one area. Typing a geo query by hand means writing
 * `georel`, `geometry`, a JSON ring and `geoproperty` without a mistake; four numbers cannot be written
 * wrong in a way that reaches the manifest, because the ring, the rounding and the length limit of
 * `coordinates` are the SDK's own (`ringOfBounds`, `areaQuery`) — the same code the grid's map uses, so a
 * shape too long for a URL is shortened in one place and said so once (UI-67).
 *
 * Drawing the shape on a map is the grid's job and stays there; this is the part a settings page can do
 * honestly without a map.
 */
/** The GeoProperties the model declares on `classes`, NGSI-LD's default `location` first. */
export function geoPropertiesOf(source: string | undefined, classes: string[]): string[] {
  const declared = classes.flatMap((cls) =>
    filterSlotsOf(source, cls)
      .filter((slot) => slot.kind === "GeoProperty")
      .map((slot) => slot.name),
  );
  return [...new Set(["location", ...declared])];
}

function AreaFromBox({ geoProperties, onSet }: { geoProperties: string[]; onSet: (geoQ: string) => void }): JSX.Element {
  const { t } = useTranslation();
  const [box, setBox] = useState<Record<string, string>>({ west: "", south: "", east: "", north: "" });
  const [property, setProperty] = useState("location");
  const [shown, setShown] = useState(false);
  const fault = boxFault(box);

  const apply = () => {
    setShown(true);
    if (fault || property.trim() === "") {
      return;
    }
    const query = areaQuery({
      geoproperty: property.trim(),
      geometry: {
        type: "Polygon",
        coordinates: [
          ringOfBounds(Number(box.west), Number(box.south), Number(box.east), Number(box.north)),
        ],
      },
    });
    if (!query) {
      return;
    }
    setShown(false);
    onSet(geoQueryString(query));
  };

  return (
    <div className="space-y-2 rounded-md border border-border p-3">
      <p className="text-caption font-medium text-fg-muted">{t("endpoints.area.title")}</p>
      <div className="flex flex-wrap items-end gap-2">
        {(["west", "south", "east", "north"] as const).map((corner) => (
          <Field key={corner} id={`area-${corner}`} label={t(`endpoints.area.${corner}`)} className="w-28">
            <Input
              id={`area-${corner}`}
              aria-describedby="area-hint"
              inputMode="decimal"
              value={box[corner] ?? ""}
              aria-invalid={shown && fault ? true : undefined}
              onChange={(event) => setBox({ ...box, [corner]: event.target.value })}
            />
          </Field>
        ))}
        <Field id="area-property" label={t("endpoints.area.property")} className="w-40">
          <Select
            id="area-property"
            aria-describedby="area-hint"
            value={property}
            onChange={(event) => setProperty(event.target.value)}
          >
            {geoProperties.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </Select>
        </Field>
        <Button type="button" variant="secondary" onClick={apply}>
          {t("endpoints.area.set")}
        </Button>
      </div>
      {shown && fault ? (
        <InlineError>
          {t(`endpoints.area.fault.${fault}`)}
        </InlineError>
      ) : null}
      <p id="area-hint" className="text-caption text-fg-muted">{t("endpoints.area.hint")}</p>
    </div>
  );
}

/** The operators a condition row offers, in the order a person reaches for them. */
const CONDITION_OPS: FilterOp[] = [
  "equals",
  "notEquals",
  "gt",
  "gte",
  "lt",
  "lte",
  "contains",
  "present",
  "empty",
];

/** The attributes a projection publishes, once, in order: what a condition can name. */
export function slotsOf(projection: Manifest): string[] {
  const classes = (projection.spec as { classes?: Array<{ slots?: string[] }> }).classes ?? [];
  const seen = new Set<string>();
  for (const klass of classes) {
    for (const slot of klass.slots ?? []) {
      if (slot) {
        seen.add(slot);
      }
    }
  }
  return [...seen];
}

/**
 * One condition, built instead of typed (T-2283, the owner's `age>30`).
 *
 * It **appends** to the query rather than replacing it: an existing `q` was written by somebody, may
 * use more of NGSI-LD §4.9 than these three boxes can say, and rewriting it from a parse that got it
 * wrong would silently change what a million entities' worth of endpoint publishes. Appending with
 * `;` is what the gateway does with two conditions, and it can only narrow.
 *
 * The term itself is compiled by the SDK's own compiler (`queryFromFilters`), which is what the entity
 * grid's filter row uses: one place quotes a value, escapes a pattern's metacharacters and decides
 * whether a literal is bare or quoted (UI-66, EP-07).
 */
function ConditionBuilder({
  attributes,
  enums = {},
  onAdd,
}: {
  attributes: string[];
  /** An attribute with permissible values is compared against one of them, picked (T-2706). */
  enums?: Record<string, string[]>;
  onAdd: (term: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [attr, setAttr] = useState(attributes[0] ?? "");
  const [op, setOp] = useState<FilterOp>("equals");
  const [value, setValue] = useState("");
  const [invalid, setInvalid] = useState<string | null>(null);
  const needsValue = op !== "present" && op !== "empty";

  const add = () => {
    if (!attr) {
      setInvalid(t("endpoints.condition.needsAttribute"));
      return;
    }
    if (needsValue && value.trim() === "") {
      setInvalid(t("endpoints.condition.needsValue"));
      return;
    }
    const { q } = queryFromFilters(
      // `untyped`, not `text`: this page has the projection's attribute names and no model to
      // read their types from, so the value is judged by its shape. Claiming `text` quotes it,
      // and `pm10>"50"` compares a number against a string and matches nothing (T-2448).
      [{ key: attr, attr, kind: "untyped" }],
      { [attr]: { op, value: value.trim() } },
    );
    if (!q) {
      setInvalid(t("endpoints.condition.needsValue"));
      return;
    }
    setInvalid(null);
    setValue("");
    onAdd(q);
  };

  return (
    <div className="space-y-2 rounded-md border border-border p-3">
      <p className="text-caption font-medium text-fg-muted">{t("endpoints.condition.title")}</p>
      <div className="flex flex-wrap items-end gap-2">
        <Field id="condition-attr" label={t("endpoints.condition.attribute")} className="min-w-40">
          <Select
            id="condition-attr"
            aria-describedby="condition-hint"
            value={attr}
            onChange={(event) => {
              setAttr(event.target.value);
              // A value typed for one attribute is rarely one of the next one's permitted values.
              setValue("");
            }}
          >
            {attributes.length === 0 ? <option value="">{t("endpoints.condition.noAttributes")}</option> : null}
            {attributes.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </Select>
        </Field>
        <Field id="condition-op" label={t("endpoints.condition.operator")} className="min-w-36">
          <Select
            id="condition-op"
            aria-describedby="condition-hint"
            value={op}
            onChange={(event) => setOp(event.target.value as FilterOp)}
          >
            {CONDITION_OPS.map((candidate) => (
              <option key={candidate} value={candidate}>
                {t(`entityGrid.ops.${candidate === "empty" ? "isEmpty" : candidate}`)}
              </option>
            ))}
          </Select>
        </Field>
        {needsValue ? (
          <Field id="condition-value" label={t("endpoints.condition.value")} className="min-w-40">
            {enums[attr] && (op === "equals" || op === "notEquals") ? (
              <Select id="condition-value" aria-describedby="condition-hint" value={value} onChange={(event) => setValue(event.target.value)}>
                <option value="">{t("form.choose")}</option>
                {enums[attr].map((permitted) => (
                  <option key={permitted} value={permitted}>
                    {permitted}
                  </option>
                ))}
              </Select>
            ) : (
              <Input
                id="condition-value"
                aria-describedby="condition-hint"
                value={value}
                onChange={(event) => setValue(event.target.value)}
              />
            )}
          </Field>
        ) : null}
        <Button type="button" variant="secondary" onClick={add}>
          {t("endpoints.condition.add")}
        </Button>
      </div>
      {invalid ? (
        <InlineError>
          {invalid}
        </InlineError>
      ) : null}
      <p id="condition-hint" className="text-caption text-fg-muted">{t("endpoints.condition.hint")}</p>
    </div>
  );
}

/** The endpoint spec as this page reads it; the manifest refuses anything not in `EndpointSpec`. */
interface EndpointSpec {
  contextSpaceRef?: unknown;
  slug?: string;
  audience?: string;
  allowedProjects?: string[];
  policyRef?: string;
  enabledRepresentations?: string[];
  disabledRepresentations?: string[];
  rateLimits?: { requestsPerMinute?: number; burst?: number };
  fileLimits?: { maxFileRows?: number; maxFileBytes?: number };
  caching?: { maxAgeSeconds?: number };
  projection?: { hiddenAttributes?: string[] };
  projectionRef?: unknown;
  viewMappingRef?: unknown;
  publish?: {
    ckan?: { instanceRef?: unknown; organization?: string; name?: string };
  };
  catalog?: CatalogManifest;
}

/** The four narrowings a `ModelProjection` can carry (MP-01); an endpoint has none of its own. */
export const FILTER_KEYS = ["q", "scopeQ", "geoQ", "temporalQ"] as const;

export function filterOf(projection: Manifest): Record<string, string | undefined> {
  const filter = (projection.spec as { filter?: Record<string, string> }).filter ?? {};
  return filter;
}

export function classesOf(projection: Manifest): string[] {
  const classes = (projection.spec as { classes?: Array<{ name?: string }> }).classes ?? [];
  return classes.map((klass) => klass.name ?? "").filter((klass) => klass !== "");
}

function Section({
  title,
  lead,
  children,
}: {
  title: string;
  lead?: string;
  children: ReactNode;
}): JSX.Element {
  return (
    <section className="space-y-2">
      <h2 className="text-base font-semibold">{title}</h2>
      {lead ? <p className="text-sm text-fg-muted">{lead}</p> : null}
      {children}
    </section>
  );
}

/** The facts of one section: two columns on a screen, stacked under 400 px. */
function Facts({ children }: { children: ReactNode }): JSX.Element {
  return <dl className="grid grid-cols-1 gap-3 sm:grid-cols-2">{children}</dl>;
}

function Fact({ label, children }: { label: ReactNode; children: ReactNode }): JSX.Element {
  return (
    // min-w-0 and anywhere: a code-list IRI in a fact made the page 409 px wide at 400 (T-5592).
    <div className="min-w-0 rounded-md border border-border p-3">
      <dt className="text-caption font-medium text-fg-muted">{label}</dt>
      <dd className="mt-1 text-sm text-fg [overflow-wrap:anywhere]">{children}</dd>
    </div>
  );
}


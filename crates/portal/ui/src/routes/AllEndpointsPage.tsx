import { useState } from "react";
import type { JSX } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { asManifests, localized } from "../api/manifest";
import type { Manifest } from "../api/manifest";
import { RepresentationSummary, servedRepresentations } from "../components/endpoints/links";
import { useProjects } from "../api/projects";
import { DeleteResourceAction } from "../components/DeleteResourceDialog";
import { RowActions } from "../components/ui/RowActions";
import type { RowAction } from "../components/ui/RowActions";
import { usePermissions } from "../api/permissions";
import { CkanAccessPanel } from "../pages/ckan/CkanAccessPanel";
import { SharedWithBadge, sharingOf, spaceOf } from "../components/endpoints/sharing";
import {
  Alert,
  Button,
  EmptyState,
  Icon,
} from "../components/ui";
import { ManifestGrid } from "../components/grid/ManifestGrid";

/**
 * Rows per page (T-4831): each row carries its representation links, about 130 px, so ten keep the
 * page under three screens; the whole list was 22,822 px with no way to find one.
 */
export const ENDPOINTS_PER_PAGE = 10;

type Row = { project: string; endpoint: Manifest };

/**
 * Every Endpoint of every project in one table (EP-08, EP-44, PF-61): the project and space it
 * publishes, its audience, and one direct link per representation. It is the Organization page's
 * Endpoints tab, for administrators of the organization only (T-2877); the organization-level
 * route answers it in one request and refuses everyone else with `404`.
 */
export function AllEndpointsPage(): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const endpoints = useQuery({
    queryKey: queryKeys.allEndpoints(),
    queryFn: async () => unwrap(await api.GET("/api/v1/endpoints")),
  });

  const header = (
    <div>
      <h2 id="all-endpoints-heading" className="text-title font-semibold text-fg">
        {t("allEndpoints.title")}
      </h2>
      <p className="text-body text-fg-muted">{t("allEndpoints.lead")}</p>
    </div>
  );

  const failed = endpoints.isError ? endpoints : null;
  if (failed) {
    const message =
      failed.error instanceof ApiError
        ? (failed.error.problem?.detail ?? failed.error.message)
        : t("app.error.generic");
    return (
      <div className="flex flex-col gap-section">
        {header}
        <Alert
          role="alert"
          tone="danger"
          actions={
            <Button
              size="sm"
              icon={<Icon name="refresh" className="size-4" />}
              onClick={() => {
                void failed.refetch();
              }}
            >
              {t("app.error.retry")}
            </Button>
          }
        >
          {message}
        </Alert>
      </div>
    );
  }

  const all: Row[] = asManifests(endpoints.data?.items ?? []).map((endpoint) => ({
    // The project is the manifest's own namespace, which is what the route answers with.
    project: endpoint.metadata.namespace ?? "",
    endpoint,
  }));
  const titleOf = (row: Row) => localized(row.endpoint.metadata.title, locale, row.endpoint.metadata.name);
  const specOf = (row: Row) => row.endpoint.spec as Parameters<typeof servedRepresentations>[0] & { audience?: string; slug?: string };

  return (
    <div className="flex flex-col gap-section">
      {header}
      <ManifestGrid<Row>
        caption={t("allEndpoints.title")}
        storageKey="all-endpoints"
        searchLabel={t("allEndpoints.find")}
        rows={all}
        rowKey={(row) => `${row.project}/${row.endpoint.metadata.name}`}
        query={endpoints}
        pageSize={ENDPOINTS_PER_PAGE}
        name={{
          id: "name",
          label: t("endpoints.field.name"),
          // The name is searched beside the title: a person often knows only one of the two.
          text: (row) => (row.endpoint.metadata.title ? `${titleOf(row)} (${row.endpoint.metadata.name})` : row.endpoint.metadata.name),
          cell: ({ project, endpoint }) => (
            // The whole row opens the endpoint's page, where its surface links are (T-4989).
            <>
              <Link
                data-row-link=""
                to="/projects/$project/$plural/$name"
                params={{ project, plural: "endpoints", name: endpoint.metadata.name }}
                className="focus-ring inline-block max-w-56 truncate rounded-sm align-bottom text-fg hover:text-primary-soft-fg"
              >
                {localized(endpoint.metadata.title, locale, endpoint.metadata.name)}
              </Link>
              {endpoint.metadata.title ? (
                <span className="ml-2 hidden font-mono text-caption font-normal text-fg-subtle 2xl:inline">
                  {endpoint.metadata.name}
                </span>
              ) : null}
            </>
          ),
        }}
        columns={[
          {
            id: "project",
            label: t("allEndpoints.field.project"),
            filter: true,
            text: (row) => row.project,
            cell: ({ project }) => (
              <Link
                to="/projects/$project/$plural"
                params={{ project, plural: "endpoints" }}
                className="focus-ring rounded-sm font-medium text-fg hover:text-primary-soft-fg"
              >
                {project}
              </Link>
            ),
          },
          {
            id: "space",
            label: t("allEndpoints.field.space"),
            filter: true,
            text: (row) => spaceOf(row.endpoint) ?? "",
            cell: ({ project, endpoint }) => {
              const space = spaceOf(endpoint);
              return space ? (
                <Link
                  to="/projects/$project/$plural/$name"
                  params={{ plural: "spaces", project, name: space }}
                  aria-label={`${t("spaces.inside.open")}: ${project}/${space}`}
                  title={space}
                  className="focus-ring truncate rounded-sm font-mono text-caption text-fg-muted hover:text-primary-soft-fg"
                >
                  {space}
                </Link>
              ) : null;
            },
          },
          { id: "slug", label: t("endpoints.field.slug"), hidden: true, text: (row) => specOf(row).slug ?? "" },
          {
            id: "audience",
            label: t("endpoints.field.audience"),
            filter: true,
            text: (row) => sharingOf(row.endpoint).audience,
            valueLabel: (value) => t(`endpoints.audience.${value}`),
            cell: ({ endpoint }) => <SharedWithBadge endpoint={endpoint} />,
          },
          {
            id: "representations",
            label: t("endpoints.field.representations"),
            text: (row) => servedRepresentations(specOf(row)).join(", "),
            cell: (row) => <RepresentationSummary served={servedRepresentations(specOf(row))} />,
          },
        ]}
        actions={(row) =>
          specOf(row).audience === "public" ? <PublicEndpointActions project={row.project} name={row.endpoint.metadata.name} /> : null
        }
        empty={<EmptyState bare icon="endpoints" title={t("allEndpoints.empty")} description={t("allEndpoints.emptyHint")} />}
      />
      <CkanInstancesAccess />
    </div>
  );
}

/**
 * What the project's own Endpoint page offers on a public Endpoint, here for every project
 * (PF-61): edit it (its audience and publication are fields of the form) and delete it. Each is
 * a Change in that project's repository through the project's own doors, in its normal lane.
 */
function PublicEndpointActions({ project, name }: { project: string; name: string }): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const permissions = usePermissions(project);
  const [deleting, setDeleting] = useState(false);
  const label = `${project}/${name}`;
  const target = { project, kind: "Endpoint", plural: "endpoints", name, label };
  // One menu, not two buttons, so the row keeps to the table's width (T-4989); an action the
  // role does not grant stays in it, disabled, with the reason (UI-44).
  const actions: RowAction[] = [
    {
      key: "edit",
      label: t("allEndpoints.editShort"),
      onSelect: () =>
        void navigate({ to: "/projects/$project/$plural/$name/edit", params: { project, plural: "endpoints", name } }),
      disabledReason: permissions.can("Endpoint", "propose")
        ? undefined
        : t("permissions.denied", { verb: "propose", kind: "Endpoint" }),
    },
    {
      key: "delete",
      label: t("resourceDelete.button"),
      tone: "danger",
      onSelect: () => setDeleting(true),
      disabledReason: permissions.can("Endpoint", "delete")
        ? undefined
        : t("permissions.denied", { verb: "delete", kind: "Endpoint" }),
    },
  ];
  return (
    <>
      <RowActions label={label} actions={actions} />
      <DeleteResourceAction target={target} trigger={false} open={deleting} onOpenChange={setDeleting} />
    </>
  );
}

/**
 * Every CKAN instance of every project, each with its Access panel (PF-107): who may manage it,
 * and the one step that hands it to a group or a person. Nothing is fetched until the
 * administrator asks for the list, and a panel loads when it is opened.
 */
function CkanInstancesAccess(): JSX.Element {
  const { t } = useTranslation();
  const [shown, setShown] = useState(false);
  return (
    <section aria-labelledby="all-ckan-heading" className="flex flex-col gap-3">
      <div>
        <h2 id="all-ckan-heading" className="text-title font-semibold text-fg">
          {t("allEndpoints.ckan.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("allEndpoints.ckan.lead")}</p>
      </div>
      <div>
        <Button
          size="sm"
          variant="secondary"
          aria-expanded={shown}
          aria-controls="all-ckan-list"
          onClick={() => setShown((current) => !current)}
        >
          {shown ? t("allEndpoints.ckan.hide") : t("allEndpoints.ckan.show")}
        </Button>
      </div>
      <div id="all-ckan-list">{shown ? <CkanInstancesList /> : null}</div>
    </section>
  );
}

function CkanInstancesList(): JSX.Element {
  const { t } = useTranslation();
  const projects = useProjects();
  const lists = useQueries({
    queries: (projects.data ?? []).map((project) => ({
      queryKey: queryKeys.list(project, "ckaninstances"),
      queryFn: async () =>
        unwrap(
          await api.GET("/api/v1/projects/{project}/{plural}", {
            params: { path: { project, plural: "ckaninstances" } },
          }),
        ),
    })),
  });
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const byProject = new Map(
    lists.map((query, index) => [
      (projects.data ?? [])[index] ?? "",
      asManifests(query.data?.items ?? []).map((instance) => instance.metadata.name),
    ]),
  );
  const instances = [...byProject].flatMap(([project, names]) => names.map((name) => ({ project, name })));
  const failed = projects.isError ? projects : lists.find((query) => query.isError);
  if (failed) {
    return (
      <Alert role="alert" tone="danger">
        {failed.error instanceof ApiError
          ? (failed.error.problem?.detail ?? failed.error.message)
          : t("app.error.generic")}
      </Alert>
    );
  }
  if (projects.isPending || lists.some((query) => query.isPending)) {
    return (
      <p role="status" className="text-body text-fg-muted">
        {t("app.loading")}
      </p>
    );
  }
  if (instances.length === 0) {
    return <p className="text-body text-fg-muted">{t("allEndpoints.ckan.empty")}</p>;
  }
  return (
    <ul className="flex flex-col gap-2" aria-label={t("allEndpoints.ckan.title")}>
      {instances.map(({ project, name }) => {
        const key = `${project}/${name}`;
        return (
          <li key={key}>
            <details
              onToggle={(event) => {
                const opened = event.currentTarget.open;
                setOpen((current) => {
                  const next = new Set(current);
                  if (opened) next.add(key);
                  else next.delete(key);
                  return next;
                });
              }}
            >
              <summary className="focus-ring cursor-pointer rounded-sm font-medium text-fg">
                {t("allEndpoints.ckan.instance", { project, name })}
              </summary>
              {open.has(key) ? (
                <div className="mt-2">
                  <CkanAccessPanel project={project} instance={name} known={byProject.get(project) ?? []} />
                </div>
              ) : null}
            </details>
          </li>
        );
      })}
    </ul>
  );
}

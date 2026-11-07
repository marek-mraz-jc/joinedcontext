/**
 * The Data models page: every DataModel of the project, one row each (DM-61, DM-62, T-2765), and
 * apart from them the organization's models, read-only, each with the name a model imports it
 * by (DM-75, DM-76, DM-79).
 *
 * It used to open on the Smart Data Models catalogue, and the project's own models were listed
 * nowhere. A row says which space the model belongs to, its version and lifecycle, its classes,
 * when it last changed and what uses it, and opens the model's own page. Creating one is an
 * action here, never the landing page: blank, from a file, or from Smart Data Models, each of
 * which opens the editor (`?new=…`), which asks for the space first.
 */
import { Fragment, useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import type { UseQueryResult } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap, whilePending } from "../../api/client";
import { asManifests, localized } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { ManifestGrid } from "../../components/grid/ManifestGrid";
import { usePermissions } from "../../api/permissions";
import {
  EmptyState,
  Button,
  PageHeader,
  buttonClass,
} from "../../components/ui";
import type { ButtonVariant } from "../../components/ui";
import { OrganizationModels } from "./OrganizationModels";
import { entityTypesOf } from "../spaces/spaceFacts";

import { spaceOfModel, usageCounts, usesOfModel } from "./modelUsage";
import type { ProjectManifests, UsingKind } from "./modelUsage";

/** Types a row names before "+N": a model of 27 classes was a four-line wall (T-6254). */
export const TYPES_SHOWN = 6;

/** A list's manifests; a module function, so `select` keeps its answer while the list stands still. */
function manifestsOf(list: { items?: Parameters<typeof asManifests>[0] }): Manifest[] {
  return asManifests(list.items ?? []);
}

/** One project list, cached under the key every page that lists the kind shares (T-0625). */
export function useProjectList(project: string, plural: string): UseQueryResult<Manifest[]> {
  return useQuery({
    queryKey: queryKeys.list(project, plural),
    // A change on its way polls until it lands (T-1392).
    refetchInterval: whilePending,
    retry: false,
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/{plural}", { params: { path: { project, plural } } })),
    select: manifestsOf,
  });
}

/** The lists `usesOfModel` reads, loaded side by side; a list that failed is left out. */
export function useProjectManifests(project: string): ProjectManifests {
  const spaces = useProjectList(project, "spaces").data;
  const endpoints = useProjectList(project, "endpoints").data;
  const pipelines = useProjectList(project, "pipelines").data;
  const subscriptions = useProjectList(project, "subscriptions").data;
  const apps = useProjectList(project, "apps").data;
  const dashboards = useProjectList(project, "dashboards").data;
  const layers = useProjectList(project, "layers").data;
  const mappings = useProjectList(project, "mappings").data;
  // One object while the lists stand still, so what is computed from it is computed once.
  return useMemo(
    () => ({ spaces, endpoints, pipelines, subscriptions, apps, dashboards, layers, mappings }),
    [spaces, endpoints, pipelines, subscriptions, apps, dashboards, layers, mappings],
  );
}

/** When the reconciler last saw the model change, from its newest condition. */
export function lastChangeOf(model: Manifest): string | undefined {
  const times = (model.status?.conditions ?? [])
    .map((condition) => condition.lastTransitionTime)
    .filter((time): time is string => typeof time === "string" && !Number.isNaN(Date.parse(time)));
  return times.sort().at(-1);
}

/** The "used by" column's kinds: the ones a person builds on a model. */
const COUNTED: readonly UsingKind[] = ["Endpoint", "Pipeline", "App", "Dashboard"];

/**
 * A link that proposes a change to a DataModel (create or edit): a link for a person whose role
 * may propose one, and a disabled button that says why for anyone else (UI-44), since a link
 * cannot be disabled and a person who follows it would only meet the refusal at Save.
 */
export function ProposeLink({
  project,
  search,
  variant = "secondary",
  children,
}: {
  project: string;
  search: { new?: "blank" | "file" | "sdm"; edit?: string; space?: string };
  variant?: ButtonVariant;
  children: string;
}): JSX.Element {
  const { t } = useTranslation();
  const { can } = usePermissions(project);
  if (!can("DataModel", "propose")) {
    return (
      <Button size="sm" variant={variant} disabled disabledReason={t("permissions.denied", { verb: "propose", kind: "DataModel" })}>
        {children}
      </Button>
    );
  }
  return (
    <Link to="/projects/$project/models" params={{ project }} search={search} className={buttonClass(variant, "sm")}>
      {children}
    </Link>
  );
}

/** One New button straight to the new model's window; the catalogue, a file and a blank model are its choices
    there (owner 2026-11-01: "just directly new model and from blank model", no menu first). */
function NewModelMenu({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <ProposeLink project={project} search={{ new: "file" }} variant="primary">
      {t("models.page.new.menu")}
    </ProposeLink>
  );
}

function text(value: unknown): string | undefined {
  return typeof value === "string" && value !== "" ? value : undefined;
}

export function ModelsList({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.language;
  const models = useProjectList(project, "datamodels");
  const all = useProjectManifests(project);
  const [change, setChange] = useState<Change | null>(null);

  const rows = useMemo(
    () =>
      (models.data ?? []).map((model) => {
        const title = localized(model.metadata.title, locale, model.metadata.name);
        return {
          model,
          title,
          space: spaceOfModel(model, all.spaces),
          classes: entityTypesOf(model),
          counts: usageCounts(usesOfModel(model, all)),
          changed: lastChangeOf(model),
        };
      }),
    [models.data, all, locale],
  );
  const date = new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" });

  const create = <NewModelMenu project={project} />;

  return (
    <div className="flex flex-col gap-6">
      <section aria-label={t("models.page.projectSection")} className="flex flex-col gap-4">
        <ManifestGrid
          caption={t("models.page.listCaption")}
          storageKey="datamodels"
          searchLabel={t("models.page.search")}
          // One row: the title, the list's search and tools, and one New button (owner 2026-10-31).
          header={(toolbar) => (
            <>
              <PageHeader compact title={t("nav.models")} description={t("models.page.listLead")} aside={toolbar} actions={create} />
              {change ? <ChangeNotice change={change} project={project} /> : null}
            </>
          )}
          rows={rows}
          rowKey={(row) => row.model.metadata.name}
          query={models}
          name={{
            id: "name",
            label: t("models.field.name"),
            text: (row) => row.title,
            cell: (row) => (
              <>
                {/* A floor under the name, so "Open data catalogue" keeps one line beside a long Types cell (T-6254). */}
                <Link
                  data-row-link=""
                  to="/projects/$project/models/$name"
                  params={{ project, name: row.model.metadata.name }}
                  className="focus-ring inline-block text-primary-soft-fg underline-offset-2 hover:underline sm:min-w-48"
                >
                  {row.title}
                </Link>
                {row.title !== row.model.metadata.name ? (
                  <span className="block font-mono text-caption text-fg-muted">{row.model.metadata.name}</span>
                ) : null}
              </>
            ),
          }}
          columns={[
            {
              id: "space",
              label: t("models.field.space"),
              field: "contextSpaceRef",
              filter: true,
              text: (row) => row.space ?? "",
              cell: (row) => (
                <>
                  {row.space ? (
                    <Link
                      to="/projects/$project/$plural/$name"
                      params={{ project, plural: "spaces", name: row.space }}
                      className="focus-ring text-primary-soft-fg underline-offset-2 hover:underline"
                    >
                      {row.space}
                    </Link>
                  ) : (
                    "—"
                  )}
                </>
              ),
            },
            { id: "version", label: t("models.field.version"), field: "version", text: (row) => text(row.model.spec.version) ?? "", cell: (row) => <span className="font-mono">{text(row.model.spec.version) ?? "—"}</span> },
            {
              id: "lifecycle",
              label: t("grid.field.lifecycle"),
              field: "lifecycle",
              filter: true,
              text: (row) => {
                const lifecycle = text(row.model.spec.lifecycle);
                return lifecycle ? t(`models.lifecycleOption.${lifecycle}`, { defaultValue: lifecycle }) : "";
              },
            },
            {
              id: "classes",
              label: t("models.classes"),
              text: (row) => row.classes.join(" "),
              cell: (row) => (
                <>
                  <span className="font-medium">{row.classes.length}</span>
                  {row.classes.length > 0 ? (
                    <span className="block text-caption text-fg-muted">
                      {row.classes.slice(0, TYPES_SHOWN).map((klass, index) => (
                        <Fragment key={klass}>
                          {index === 0 ? null : ", "}
                          <Link
                            to="/projects/$project/models/$name"
                            params={{ project, name: row.model.metadata.name }}
                            search={{ class: klass }}
                            className="focus-ring underline-offset-2 hover:underline"
                          >
                            {klass}
                          </Link>
                        </Fragment>
                      ))}
                      {row.classes.length > TYPES_SHOWN ? (
                        <>
                          {", "}
                          <Link
                            to="/projects/$project/models/$name"
                            params={{ project, name: row.model.metadata.name }}
                            aria-label={t("models.page.moreTypes", { count: row.classes.length - TYPES_SHOWN, model: row.title })}
                            className="focus-ring font-medium underline-offset-2 hover:underline"
                          >
                            +{row.classes.length - TYPES_SHOWN}
                          </Link>
                        </>
                      ) : null}
                    </span>
                  ) : null}
                </>
              ),
            },
            { id: "changed", label: t("models.page.lastChange"), text: (row) => row.changed ?? "", cell: (row) => (row.changed ? date.format(new Date(row.changed)) : "—") },
            {
              id: "usedBy",
              label: t("models.page.usedBy"),
              text: (row) =>
                COUNTED.every((kind) => (row.counts[kind] ?? 0) === 0)
                  ? t("models.page.unused")
                  : COUNTED.filter((kind) => (row.counts[kind] ?? 0) > 0)
                      .map((kind) => t(`models.page.uses.${kind}`, { count: row.counts[kind] }))
                      .join(", "),
            },
            { id: "openWorld", label: t("grid.field.openWorld"), field: "openWorld", hidden: true, text: (row) => (row.model.spec.openWorld === true ? t("grid.on") : t("grid.off")) },
          ]}
          edit={{ project, plural: "datamodels", kind: "DataModel", manifest: (row) => row.model, onChange: setChange }}
          empty={<EmptyState bare icon="models" title={t("models.page.emptyTitle")} description={t("models.page.emptyLead")} action={create} />}
        />
      </section>

      <OrganizationModels inProject />
    </div>
  );
}

import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { localized, ORG_NAMESPACE, plainTitle } from "../../api/manifest";
import { useProjectOrganization } from "../../api/projects";
import { administersOrganization, usePermissions } from "../../api/permissions";
import type { Rule } from "../../api/permissions";
import { RuleInWords, granteeName } from "../../components/AccessWords";
import { DeleteProjectAction } from "../../components/DeleteProjectDialog";
import { DuplicateProjectAction } from "../../components/DuplicateProjectDialog";
import { EditResourceAction } from "../../components/EditResourceDialog";
import type { EditableForm } from "../../components/EditResourceDialog";
import { ProjectQuota } from "../../components/ProjectQuota";
import { ProjectRelease } from "../../components/ProjectRelease";
import {
  Alert,
  Field,
  Input,
  PageFailed,
  PageHeader,
  Table,
  TableBody,
  TableCell,
  safeHref,
  TableEmpty,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableSkeleton,
  tabPanelProps,
  Tabs,
} from "../../components/ui";
import { APP_SERVICES_UI, projectSchema } from "../../schemas/kinds";
import { AccessExplorer } from "../access/AccessExplorer";
import { EffectivePermissions } from "../access/EffectivePermissions";
import { findSettings, settingsIndex } from "./settingsIndex";
import { RoleBindings } from "../access/RoleBindings";
import { Roles } from "../access/Roles";
import { ServiceAccounts } from "../access/ServiceAccounts";

// The tabs live apart so the router can know them without loading this page (bundle budget).
import { PROJECT_SETTINGS_TABS } from "./tabs";
import type { ProjectSettingsTab } from "./tabs";
export { isProjectSettingsTab, PROJECT_SETTINGS_TABS } from "./tabs";
export type { ProjectSettingsTab } from "./tabs";

interface ProjectManifest {
  metadata?: { name?: string; title?: unknown; description?: unknown };
  spec?: {
    quotas?: Record<string, number | undefined>;
    organizationRef?: unknown;
    /** What the project allows its Apps, within the organization's (AP-163, AP-165). */
    apps?: { services?: string[]; limits?: Record<string, number | undefined> };
    /** The body that publishes the project's catalogue (EP-127). */
    catalog?: ProjectCatalog;
  };
}

/** `spec.catalog` of `project.yaml`, as jc-core's `ProjectCatalog` reads it. */
interface ProjectCatalog {
  publisher: { name: Record<string, string>; uri?: string };
  contactPoint?: { name: string; email: string };
}

/** The catalogue group of the form: flat, so the schema can say what goes with what. */
interface CatalogForm {
  publisherName?: Record<string, string | undefined>;
  publisherUri?: string;
  contactName?: string;
  contactEmail?: string;
}

/** What General edits of `project.yaml`: its title, its description and its own quotas. */
export function fromProject(manifest: unknown): Record<string, unknown> {
  const project = (manifest ?? {}) as ProjectManifest;
  return {
    title: plainTitle(project.metadata?.title),
    description: plainTitle(project.metadata?.description),
    quotas: project.spec?.quotas ?? {},
    apps: { services: project.spec?.apps?.services ?? [], limits: project.spec?.apps?.limits ?? {} },
    catalog: catalogForm(project.spec?.catalog),
  };
}

function catalogForm(catalog: ProjectCatalog | undefined): CatalogForm {
  if (!catalog) {
    return {};
  }
  return {
    publisherName: { ...catalog.publisher.name },
    publisherUri: catalog.publisher.uri,
    contactName: catalog.contactPoint?.name,
    contactEmail: catalog.contactPoint?.email,
  };
}

/** A form text trimmed, or nothing when it is empty. */
function text(value: unknown): string | undefined {
  const trimmed = typeof value === "string" ? value.trim() : "";
  return trimmed === "" ? undefined : trimmed;
}

/**
 * The catalogue the form names, or nothing when every field is empty, so clearing the group
 * removes `spec.catalog`. The schema refuses a half-filled group before this runs, and jc-core
 * refuses it again on the server.
 */
export function toCatalog(form: CatalogForm | undefined): ProjectCatalog | undefined {
  const name = Object.fromEntries(
    Object.entries(form?.publisherName ?? {}).flatMap(([language, value]) => {
      const kept = text(value);
      return kept ? [[language, kept]] : [];
    }),
  );
  const uri = text(form?.publisherUri);
  const contact = { name: text(form?.contactName), email: text(form?.contactEmail) };
  if (Object.keys(name).length === 0 && !uri && !contact.name && !contact.email) {
    return undefined;
  }
  return {
    publisher: { name, ...(uri ? { uri } : {}) },
    ...(contact.name || contact.email
      ? { contactPoint: { name: contact.name ?? "", email: contact.email ?? "" } }
      : {}),
  };
}

/** The numeric members of a form object, or nothing when none is set. */
function numbers(value: unknown): Record<string, number> | undefined {
  const kept = Object.fromEntries(
    Object.entries((value ?? {}) as Record<string, unknown>).filter(([, member]) => typeof member === "number"),
  ) as Record<string, number>;
  return Object.keys(kept).length > 0 ? kept : undefined;
}

/**
 * The manifest to propose: the stored one with the title and description in its metadata and the
 * quotas in its spec, so `organizationRef` and every other field stay as they were (T-2470).
 */
export function toProject(form: Record<string, unknown>, stored: unknown): unknown {
  const manifest = { ...((stored ?? {}) as Record<string, unknown>) };
  delete manifest.status;
  const metadata = { ...((manifest.metadata ?? {}) as Record<string, unknown>) };
  const spec = { ...((manifest.spec ?? {}) as Record<string, unknown>) };
  for (const field of ["title", "description"] as const) {
    const value = typeof form[field] === "string" ? (form[field] as string).trim() : "";
    if (value === "") {
      delete metadata[field];
    } else {
      metadata[field] = value;
    }
  }
  const quotas = numbers(form.quotas);
  if (quotas) {
    spec.quotas = quotas;
  } else {
    delete spec.quotas;
  }
  // No box ticked leaves the organization's list in force; a project narrows, it never widens.
  const formApps = (form.apps ?? {}) as { services?: unknown; limits?: unknown };
  const services = Array.isArray(formApps.services) && formApps.services.length > 0 ? formApps.services : undefined;
  const limits = numbers(formApps.limits);
  if (services || limits) {
    spec.apps = { ...(services ? { services } : {}), ...(limits ? { limits } : {}) };
  } else {
    delete spec.apps;
  }
  const catalog = toCatalog(form.catalog as CatalogForm | undefined);
  if (catalog) {
    spec.catalog = catalog;
  } else {
    delete spec.catalog;
  }
  return { ...manifest, metadata, spec };
}

/**
 * General (PF-17, PF-61): the project's title and description, edited through the kind's form as a
 * proposed change, its quota in use, and — read only — the organization it belongs to and who
 * reads its projects there.
 */
function General({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const manifest = useQuery({
    queryKey: [...queryKeys.list(ORG_NAMESPACE, "projects"), project],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project: ORG_NAMESPACE, plural: "projects", name: project } },
        }),
      ),
  });
  // The project's own Organization: with several, the first of the list is another body's.
  const { organization } = useProjectOrganization(project);
  const orgSpec = (organization?.spec ?? {}) as { domain?: string; projects?: { visibility?: string } };
  const stored = (manifest.data ?? {}) as ProjectManifest;
  const title = localized(stored.metadata?.title as string | Record<string, string> | undefined, locale, project);
  const description = plainTitle(stored.metadata?.description);
  // The anonymous catalogue the gateway serves on this host once a publisher is saved (API/08 §6).
  const catalogUrl = stored.spec?.catalog
    ? `${window.location.origin}/catalog/${encodeURIComponent(project)}.jsonld`
    : undefined;
  const inOwnRepository = Boolean((stored.spec as { repository?: unknown } | undefined)?.repository);
  const form: EditableForm = {
    schema: projectSchema(t),
    uiSchema: { apps: { services: APP_SERVICES_UI } },
    fromManifest: fromProject,
    toManifest: toProject,
  };

  return (
    <div className="space-y-8">
      <section className="space-y-4" aria-labelledby="project-general-heading">
        <div className="flex flex-wrap items-end justify-between gap-3">
          <div>
            <h2 id="project-general-heading" className="text-title font-semibold text-fg">
              {t("projectSettings.general.title")}
            </h2>
            <p className="text-body text-fg-muted">{t("projectSettings.general.lead")}</p>
          </div>
          {manifest.data ? (
            <div className="flex flex-wrap gap-2">
              <DuplicateProjectAction project={project} inOwnRepository={inOwnRepository} />
              <EditResourceAction
              target={{
                project,
                home: ORG_NAMESPACE,
                kind: "Project",
                plural: "projects",
                name: project,
                label: title,
              }}
              form={form}
              />
            </div>
          ) : null}
        </div>
        {manifest.isError ? (
          <PageFailed
            error={manifest.error}
            onRetry={() => {
              void manifest.refetch();
            }}
          />
        ) : null}
        <dl className="grid gap-x-6 gap-y-3 text-body sm:grid-cols-[max-content_1fr]">
          <dt className="font-medium text-fg">{t("projectSettings.field.title")}</dt>
          <dd className="text-fg">{manifest.isPending ? t("app.loading") : title}</dd>
          <dt className="font-medium text-fg">{t("projectSettings.field.description")}</dt>
          <dd className="text-fg">
            {manifest.isPending ? t("app.loading") : (description ?? t("projectSettings.general.noDescription"))}
          </dd>
          <dt className="font-medium text-fg">{t("projectSettings.general.organization")}</dt>
          <dd className="text-fg">{orgSpec.domain ?? t("projectSettings.general.unknown")}</dd>
          <dt className="font-medium text-fg">{t("organization.field.visibility")}</dt>
          <dd className="text-fg">
            {t(`organization.visibility.${orgSpec.projects?.visibility === "members" ? "members" : "organization"}`)}
          </dd>
          <dt className="font-medium text-fg">{t("projectSettings.general.catalogUrl")}</dt>
          <dd className="break-all text-fg">
            {manifest.isPending ? (
              t("app.loading")
            ) : catalogUrl ? (
              <a className="focus-ring underline" href={safeHref(catalogUrl)}>
                {catalogUrl}
              </a>
            ) : (
              t("projectSettings.general.noCatalog")
            )}
          </dd>
        </dl>
      </section>
      {inOwnRepository ? <ProjectRelease project={project} /> : null}
      <ProjectQuota project={project} />
    </div>
  );
}

/**
 * Whether `rule` grants approve on a kind no rule of the person also grants delete on: such a
 * person approves the changes others propose, and their own wait for another approver (PF-58).
 */
export function approvesOthersOnly(rule: Rule, rules: readonly Rule[]): boolean {
  if (!(rule.verbs ?? []).includes("approve")) return false;
  const deletes = new Set(rules.filter((one) => (one.verbs ?? []).includes("delete")).flatMap((one) => one.kinds ?? []));
  return !deletes.has("*") && (rule.kinds ?? []).some((kind) => !deletes.has(kind));
}

/**
 * Your access (PF-50, PF-60): what the signed-in person's bindings let them do here, each grant
 * with the binding and the scope it came from, so an inherited grant does not look local. Then the
 * endpoint matrix of the old Access page, the gateway's own answer (EP-60).
 */
function YourAccess({
  project,
  endpoint,
  onEndpoint,
}: {
  project: string;
  endpoint?: string;
  onEndpoint: (name: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const permissions = usePermissions(project);
  const grants = permissions.data?.grants ?? [];
  const rules = grants.map((grant) => grant.rule as Rule);
  return (
    <div className="space-y-8">
      <section className="space-y-4" aria-labelledby="your-access-heading">
        <div>
          <h2 id="your-access-heading" className="text-title font-semibold text-fg">
            {t("projectSettings.access.title")}
          </h2>
          <p className="text-body text-fg-muted">{t("projectSettings.access.lead")}</p>
        </div>
        {permissions.data?.bootstrap ? <Alert tone="info">{t("projectSettings.access.bootstrap")}</Alert> : null}
        <Table
          caption={t("projectSettings.access.caption", { project })}
          status={permissions.isLoading ? t("app.loading") : undefined}
        >
          <TableHead>
            <TableHeaderCell>{t("projectSettings.access.role")}</TableHeaderCell>
            <TableHeaderCell>{t("projectSettings.access.allows")}</TableHeaderCell>
            <TableHeaderCell>{t("projectSettings.access.from")}</TableHeaderCell>
          </TableHead>
          {permissions.isLoading ? (
            <TableSkeleton columns={3} />
          ) : (
            <TableBody>
              {grants.length === 0 ? (
                <TableEmpty columns={3}>{t("projectSettings.access.none")}</TableEmpty>
              ) : (
                grants.map((grant, index) => (
                  <TableRow key={`${grant.binding}-${grant.role}-${index}`}>
                    <TableCell primary>
                      <span title={grant.role}>{granteeName("role", grant.role, t)}</span>
                    </TableCell>
                    <TableCell>
                      <RuleInWords rule={grant.rule as Rule} />
                      {/* PF-58: approve without delete approves other people's changes only. */}
                      {approvesOthersOnly(grant.rule as Rule, rules) ? (
                        <p className="mt-1 text-caption text-fg-muted">{t("projectSettings.access.approveOthers")}</p>
                      ) : null}
                    </TableCell>
                    <TableCell>
                      {t("projectSettings.access.binding", { binding: grant.binding, scope: grant.scope })}
                    </TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          )}
        </Table>
      </section>
      <EffectivePermissions project={project} endpoint={endpoint} onEndpoint={onEndpoint} />
    </div>
  );
}

/** Delete project (PF-77, PF-78): never one click, the cascade listed in the dialog it opens. */
function Danger({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="space-y-8">
      {/* A project has no owner field: who administers it is a binding, so handing it over is
          binding someone to the project's administrator role and taking one's own away (T-3277). */}
      <section className="space-y-3" aria-labelledby="project-handover-heading">
        <h2 id="project-handover-heading" className="text-title font-semibold text-fg">
          {t("projectSettings.danger.handoverTitle")}
        </h2>
        <p className="text-body text-fg-muted">{t("projectSettings.danger.handoverLead")}</p>
        <Link
          to="/projects/$project/settings/$tab"
          params={{ project, tab: "members" }}
          className="text-body text-primary-soft-fg underline-offset-2 hover:underline"
        >
          {t("projectSettings.danger.handoverLink")}
        </Link>
      </section>
      <section className="space-y-4 rounded-lg border border-danger p-4" aria-labelledby="project-danger-heading">
        <h2 id="project-danger-heading" className="text-title font-semibold text-fg">
          {t("projectSettings.danger.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("projectSettings.danger.lead")}</p>
        <DeleteProjectAction project={project} variant="danger" />
      </section>
    </div>
  );
}

/** Find a setting by what it is called or what it changes (T-3277): every result names its tab. */
function SettingsSearch({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const [query, setQuery] = useState("");
  const found = findSettings(settingsIndex(t, i18n.language), query);
  return (
    <div className="flex flex-col gap-2">
      <Field id="settings-search" label={t("projectSettings.search.label")}>
        <Input
          id="settings-search"
          type="search"
          value={query}
          placeholder={t("projectSettings.search.placeholder")}
          onChange={(event) => setQuery(event.target.value)}
        />
      </Field>
      {query.trim() === "" ? null : found.length === 0 ? (
        <p className="text-caption text-fg-muted" role="status">
          {t("projectSettings.search.nothing")}
        </p>
      ) : (
        <ul aria-label={t("projectSettings.search.results")} className="flex flex-col gap-2">
          {found.map((setting) => (
            <li key={setting.key} className="rounded-md border border-border p-2">
              <Link
                to="/projects/$project/settings/$tab"
                params={{ project, tab: setting.tab }}
                className="text-body font-semibold text-primary-soft-fg underline-offset-2 hover:underline"
              >
                {setting.label}
              </Link>{" "}
              <span className="text-caption text-fg-muted">· {t(`projectSettings.tab.${setting.tab}`)}</span>
              {setting.about ? <p className="text-caption text-fg-muted">{setting.about}</p> : null}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/**
 * Project settings (T-2606, UI-76, Architecture/09 §14.2): what belongs to one project — its title
 * and quotas, who is bound in it, its own roles, its service accounts, what the signed-in person
 * may do here, and its deletion. Project → Access folded into it; every write is a proposed change.
 */
export function ProjectSettingsPage({
  project,
  tab,
  endpoint,
}: {
  project: string;
  tab: ProjectSettingsTab;
  /** The Endpoint Your access opens on (`?endpoint=`, T-5593). */
  endpoint?: string;
}): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();
  // The explorer shows another principal's access: an organization administrator's tab (T-3792).
  const administrator = administersOrganization(usePermissions(ORG_NAMESPACE).data);
  return (
    <div className="space-y-6">
      <PageHeader title={t("projectSettings.title")} description={t("projectSettings.lead", { project })} />
      <SettingsSearch project={project} />
      <Tabs
        id="project-settings"
        label={t("projectSettings.tabsLabel")}
        value={tab}
        onChange={(next) =>
          void navigate({ to: "/projects/$project/settings/$tab", params: { project, tab: next } })
        }
        tabs={PROJECT_SETTINGS_TABS.filter((value) => value !== "explorer" || administrator).map((value) => ({
          value,
          label: t(`projectSettings.tab.${value}`),
        }))}
      />
      <div {...tabPanelProps("project-settings", tab)} className="space-y-8">
        {tab === "general" ? <General project={project} /> : null}
        {tab === "members" ? <RoleBindings project={project} scope="project" /> : null}
        {tab === "roles" ? <Roles project={project} scope="project" /> : null}
        {tab === "service-accounts" ? <ServiceAccounts project={project} /> : null}
        {tab === "access" ? (
          <YourAccess
            project={project}
            endpoint={endpoint}
            onEndpoint={(next) =>
              void navigate({
                to: "/projects/$project/settings/$tab",
                params: { project, tab: "access" },
                search: { endpoint: next },
                replace: true,
              })
            }
          />
        ) : null}
        {tab === "explorer" ? <AccessExplorer project={project} /> : null}
        {tab === "danger" ? <Danger project={project} /> : null}
      </div>
    </div>
  );
}

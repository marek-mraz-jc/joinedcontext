import { PageFailed } from "../../components/ui/PageState";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { asManifests, ORG_NAMESPACE } from "../../api/manifest";
import { useChosenOrganizationName } from "../../api/projects";
import { EditResourceAction } from "../../components/EditResourceDialog";
import type { EditableForm } from "../../components/EditResourceDialog";
import { EmptyState, Skeleton } from "../../components/ui";
import { APP_SERVICES_UI, organizationSchema } from "../../schemas/kinds";
import { OrganizationDomain } from "../access/OrganizationDomain";
import { OrganizationLimitsView } from "./OrganizationLimitsView";
import { ALLOWANCE_UI, withMqttDefault } from "../mqttbrokers/allowance";

interface Contact {
  role?: string;
  name?: string;
  email?: string;
  phone?: string;
}

interface ProjectsPolicy {
  creation?: string;
  visibility?: string;
  nameCooldownDays?: number;
  quota?: Record<string, number | undefined>;
}

interface OrganizationSpec {
  domain?: string;
  gitRepositoryUrl?: string;
  /** The installation Organization (PF-121); its administrators run every other one. */
  installation?: boolean;
  locales?: string[];
  defaultLocale?: string;
  contacts?: Contact[];
  projects?: ProjectsPolicy;
  policies?: { apps?: { public?: string; services?: string[] }; agents?: { models?: string[] } };
}

/** What the form edits: the spec without `gitRepositoryUrl`, which is the installation's. */
export function fromOrganization(manifest: unknown): Record<string, unknown> {
  const spec = { ...(((manifest as { spec?: OrganizationSpec } | null)?.spec ?? {}) as OrganizationSpec) };
  delete spec.gitRepositoryUrl;
  return spec as Record<string, unknown>;
}

/**
 * The manifest to propose: the stored one with the form's fields over its spec, so what the form
 * does not show (`gitRepositoryUrl`, the metadata) stays as it was (T-2470). `status` is the
 * Portal's own and never goes back (MF-04).
 */
export function toOrganization(form: Record<string, unknown>, stored: unknown): unknown {
  const manifest = { ...((stored ?? {}) as Record<string, unknown>) };
  delete manifest.status;
  const spec = (manifest.spec ?? {}) as OrganizationSpec;
  const next = { ...spec, ...form } as OrganizationSpec;
  // No box ticked is the default list, never "nothing": the form may hand back an empty list for
  // a field nobody touched, and that would switch files and jobs off for every App (AP-163).
  if (next.policies?.apps?.services?.length === 0) {
    const apps = { ...next.policies.apps };
    delete apps.services;
    next.policies = { ...next.policies, apps };
  }
  return { ...manifest, spec: next };
}

/** `projects.creation` in words (PF-65). */
function creationOf(t: (key: string, options?: Record<string, unknown>) => string, creation?: string): string {
  if (creation === "anyone") return t("organization.settings.creationAnyone");
  if (creation?.startsWith("group:")) {
    return t("organization.settings.creationGroup", { group: creation.slice("group:".length) });
  }
  return t("organization.settings.creationAdmin");
}

/**
 * Organization → Settings (T-2605, Architecture/09 §14.1): the `Organization` manifest read in
 * words, its form behind Edit (a red-lane change, never a direct write), and the domain's
 * verification below it (PF-41).
 */
export function OrganizationSettings(): JSX.Element {
  const { t } = useTranslation();
  const organizations = useQuery({
    queryKey: queryKeys.list(ORG_NAMESPACE, "organizations"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project: ORG_NAMESPACE, plural: "organizations" } },
        }),
      ),
  });
  // The catalog with the operator's bounds and the values in force (API/01 §28): the sections
  // below and the range every numeric field of the form is held to (PF-97, PF-102).
  const limits = useQuery({
    queryKey: ["organization-limits"],
    queryFn: async () => unwrap(await api.GET("/api/v1/organization/limits")),
  });
  // With several Organizations: the one `?org=` chooses, else the installation's (ADR-N-058 2.7).
  const chosen = useChosenOrganizationName();
  const all = asManifests(organizations.data?.items ?? []);
  const organization =
    all.find((item) => item.metadata.name === chosen) ??
    all.find((item) => (item.spec as OrganizationSpec | undefined)?.installation === true) ??
    all[0];
  const spec = (organization?.spec ?? {}) as OrganizationSpec;
  const policy = spec.projects ?? {};
  const form: EditableForm = {
    schema: withMqttDefault(organizationSchema(t, limits.data?.entries ?? []), t),
    uiSchema: { policies: { apps: { services: APP_SERVICES_UI } }, limits: { mqtt: ALLOWANCE_UI } },
    fromManifest: fromOrganization,
    toManifest: toOrganization,
  };

  return (
    <div className="space-y-8">
      <section className="space-y-4" aria-labelledby="organization-settings-heading">
        <div className="flex flex-wrap items-end justify-between gap-3">
          <div>
            <h2 id="organization-settings-heading" className="text-title font-semibold text-fg">
              {t("organization.settings.title")}
            </h2>
            <p className="text-body text-fg-muted">{t("organization.settings.lead")}</p>
          </div>
          {organization ? (
            <EditResourceAction
              target={{
                project: ORG_NAMESPACE,
                home: ORG_NAMESPACE,
                kind: "Organization",
                plural: "organizations",
                name: organization.metadata.name,
                label: spec.domain ?? organization.metadata.name,
              }}
              form={form}
            />
          ) : null}
        </div>
        {organizations.isPending ? (
          <div role="status" className="space-y-2">
            <span className="sr-only">{t("app.loading")}</span>
            <Skeleton className="h-4 w-2/3" />
            <Skeleton className="h-4 w-1/2" />
          </div>
        ) : organizations.isError ? (
          <PageFailed error={organizations.error} onRetry={() => void organizations.refetch()} />
        ) : !organization ? (
          <EmptyState
            title={t("organization.settings.empty")}
            description={t("organization.settings.emptyHint")}
          />
        ) : (
          <dl className="grid gap-x-6 gap-y-3 text-body sm:grid-cols-[max-content_1fr]">
            <dt className="font-medium text-fg">{t("organization.field.domain")}</dt>
            <dd className="text-fg">{spec.domain}</dd>
            <dt className="font-medium text-fg">{t("organization.field.locales")}</dt>
            <dd className="text-fg">
              {t("organization.settings.locales", {
                locales: (spec.locales ?? []).join(", "),
                fallback: spec.defaultLocale ?? "",
              })}
            </dd>
            <dt className="font-medium text-fg">{t("organization.field.contacts")}</dt>
            <dd className="text-fg">
              {(spec.contacts ?? []).length === 0 ? (
                t("organization.settings.noContacts")
              ) : (
                <ul>
                  {(spec.contacts ?? []).map((contact, index) => (
                    <li key={`${contact.role ?? ""}-${index}`}>
                      {t(`organization.contactRole.${contact.role ?? "administrative"}`)}: {contact.name}{" "}
                      &lt;{contact.email}&gt;
                    </li>
                  ))}
                </ul>
              )}
            </dd>
            <dt className="font-medium text-fg">{t("organization.field.creation")}</dt>
            <dd className="text-fg">{creationOf(t, policy.creation)}</dd>
            <dt className="font-medium text-fg">{t("organization.field.visibility")}</dt>
            <dd className="text-fg">
              {t(`organization.visibility.${policy.visibility === "members" ? "members" : "organization"}`)}
            </dd>
          </dl>
        )}
      </section>
      <section className="space-y-4" aria-labelledby="organization-limits-heading">
        <div>
          <h2 id="organization-limits-heading" className="text-title font-semibold text-fg">
            {t("organization.limits.title")}
          </h2>
          <p className="text-body text-fg-muted">{t("organization.limits.lead")}</p>
        </div>
        {limits.isPending ? (
          <div role="status" className="space-y-2">
            <span className="sr-only">{t("app.loading")}</span>
            <Skeleton className="h-4 w-2/3" />
          </div>
        ) : limits.isError ? (
          <PageFailed error={limits.error} onRetry={() => void limits.refetch()} />
        ) : (
          <OrganizationLimitsView
            limits={limits.data}
            publicApps={spec.policies?.apps?.public}
            services={spec.policies?.apps?.services}
            models={spec.policies?.agents?.models}
          />
        )}
      </section>
      <OrganizationDomain />
    </div>
  );
}

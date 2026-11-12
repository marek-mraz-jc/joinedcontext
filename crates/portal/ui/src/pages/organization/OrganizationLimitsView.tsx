import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { EmptyState } from "../../components/ui";
import { AdminGrid } from "../../components/grid/AdminGrid";
import type { DataColumn } from "../../components/grid/AdminGrid";
import { DEFAULT_ORGANIZATION_SERVICES } from "../../schemas/kinds";
import { boundText, entryKey, SECTIONS, valueText } from "./limits";
import type { OrganizationLimits } from "./limits";

/** One setting of a section: its name and hint, the value in force and the range allowed. */
interface SettingRow {
  key: string;
  label: string;
  hint: string;
  value: string;
  range: string;
}

type ProjectLimits = NonNullable<OrganizationLimits["projects"]>[number];

/** The limits are already read by the page around: the grids have no read of their own. */
const LOADED = { isPending: false, isError: false, error: null, refetch: () => undefined };

/** The quota dimensions a manifest count answers (PF-75), in the order a person reads them. */
const COUNTED = ["contextSpaces", "residentPipelines", "publicEndpoints", "apps"] as const;
/** The ones enforced at run time: only their limit is known here. */
const RUNTIME = ["ingestEventsPerSecond", "agentRunsPerDay", "entitiesPerSpace", "requestsPerMinute"] as const;
/** The MQTT allowance against what the project's brokers declare (T-4859, T-4860). */
const MQTT = ["mqttBrokers", "mqttConnections", "mqttMessagesPerSecond"] as const;

/**
 * Every policy and limit of the organization, one section per catalog section, each entry with
 * the value in force, the range allowed and a one-line hint; then each readable project's quota
 * with what it uses (PF-99, PF-101, PF-102).
 */
export function OrganizationLimitsView({
  limits,
  publicApps,
  services,
  models,
}: {
  limits: OrganizationLimits;
  /** `spec.policies.apps.public`; absent is `allowed`. */
  publicApps?: string;
  /** `spec.policies.apps.services`; absent is the default list (ADR-N-045). */
  services?: string[];
  /** `spec.policies.agents.models`; absent is every model of the installation. */
  models?: string[];
}): JSX.Element {
  const { t } = useTranslation();
  const quotaCell = (used: number | null | undefined, limit: number | null | undefined) => {
    const allowed = limit === null || limit === undefined ? t("organization.limits.noLimit") : String(limit);
    return used === null || used === undefined ? allowed : t("organization.limits.usedOf", { used, limit: allowed });
  };

  const settingColumns: DataColumn<SettingRow>[] = [
    {
      key: "setting",
      label: t("organization.limits.setting"),
      text: (row) => row.label,
      cell: (row) => (
        <>
          <span className="font-medium text-fg">{row.label}</span>
          <p className="text-caption text-fg-muted">{row.hint}</p>
        </>
      ),
    },
    { key: "inForce", label: t("organization.limits.inForce"), text: (row) => row.value },
    { key: "range", label: t("organization.limits.range"), text: (row) => row.range },
  ];
  const projectColumns: DataColumn<ProjectLimits>[] = [
    {
      key: "project",
      label: t("organization.limits.project"),
      text: (row) => row.project,
      cell: (row) => <span className="font-mono">{row.project}</span>,
    },
    {
      key: "whose",
      label: t("organization.limits.whose"),
      text: (row) => t(`organization.limits.origin.${row.origin}`),
    },
    ...[...COUNTED, ...RUNTIME, ...MQTT].map((dimension) => ({
      key: dimension,
      label: t(`organization.limit.projects.quota.${dimension}.label`),
      text: (row: ProjectLimits) => quotaCell(row.quota[dimension]?.used, row.quota[dimension]?.limit),
    })),
  ];

  return (
    <div className="space-y-8">
      {SECTIONS.map((section) => {
        const entries = (limits.entries ?? []).filter((entry) => entry.section === section);
        const headingId = `organization-limits-${section}`;
        const settings: SettingRow[] = [
          ...(section === "applications"
            ? [
                {
                  key: "publicApps",
                  label: t("organization.policy.publicApps"),
                  hint: t("organization.policy.publicAppsHint"),
                  value: t(`organization.policy.publicApps_${publicApps === "refused" ? "refused" : "allowed"}`),
                  range: "—",
                },
                {
                  key: "appServices",
                  label: t("appServices.field"),
                  hint: t("appServices.organizationHint"),
                  value:
                    (services ?? DEFAULT_ORGANIZATION_SERVICES)
                      .map((service) => t(`appServices.service.${service}.label`))
                      .join(", ") + (services ? "" : ` ${t("appServices.byDefault")}`),
                  range: "—",
                },
              ]
            : []),
          ...(section === "agents"
            ? [
                {
                  key: "models",
                  label: t("organization.policy.models"),
                  hint: t("organization.policy.modelsHint"),
                  value: models && models.length > 0 ? models.join(", ") : t("organization.policy.everyModel"),
                  range: "—",
                },
              ]
            : []),
          ...entries.map((entry) => ({
            key: entry.path,
            label: t(`organization.limit.${entryKey(entry.path)}.label`),
            hint: t(`organization.limit.${entryKey(entry.path)}.hint`),
            value: valueText(t, entry),
            range: boundText(t, entry),
          })),
        ];
        return (
          <section key={section} className="space-y-3" aria-labelledby={headingId}>
            <h3 id={headingId} className="text-body font-semibold text-fg">
              {t(`organization.limits.section.${section}`)}
            </h3>
            <AdminGrid<SettingRow>
              query={LOADED}
              caption={t(`organization.limits.section.${section}`)}
              storageKey={`organization:limits:${section}`}
              rows={settings}
              rowKey={(row) => row.key}
              columns={settingColumns}
              stack
              noSearch
              empty={
                <EmptyState
                  bare
                  title={t("organization.limits.noSettings")}
                  description={t("organization.limits.noSettingsHint")}
                />
              }
            />
          </section>
        );
      })}

      <section className="space-y-3" aria-labelledby="organization-limits-projects-table">
        <h3 id="organization-limits-projects-table" className="text-body font-semibold text-fg">
          {t("organization.limits.perProject")}
        </h3>
        <p className="text-body text-fg-muted">{t("organization.limits.perProjectLead")}</p>
        {/* A card per project below 640 px: a row of ten quota columns was cut off at 400 px. */}
        <AdminGrid<ProjectLimits>
          query={LOADED}
          caption={t("organization.limits.perProject")}
          storageKey="organization:limits:projects"
          rows={limits.projects ?? []}
          rowKey={(row) => row.project}
          columns={projectColumns}
          stack
          empty={
            <EmptyState
              bare
              title={t("organization.limits.noProjects")}
              description={t("organization.limits.noProjectsHint")}
            />
          }
        />
      </section>
    </div>
  );
}

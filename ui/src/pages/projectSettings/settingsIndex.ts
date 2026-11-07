/**
 * Every setting of a project, where it lives and what it changes (T-3277): the index the search of
 * Project settings reads. The words of a manifest field are its form's own help (the shipped
 * `project.uischema.yaml`), so the search and the form never describe one setting two ways.
 */
import { parse as parseYaml } from "yaml";
import type { UiSchemaManifest } from "../../components/forms/uischema";
import projectForm from "../../schemas/forms/project.uischema.yaml?raw";
import { QUOTA_DIMENSIONS } from "../../schemas/kinds";

export interface Setting {
  /** The tab of `/projects/{project}/settings/{tab}` that holds it. */
  tab: "general" | "members" | "roles" | "service-accounts" | "access" | "explorer" | "danger";
  key: string;
  label: string;
  /** What changing it changes, in the person's language. */
  about: string;
}

type T = (key: string, options?: Record<string, unknown>) => string;

// Only the Project form: the shipped forms together weigh more than every page's entry (T-3316).
let form: UiSchemaManifest | undefined;

/** The help text of one field of the shipped Project form, in `language` or English. */
function help(path: string, language: string): string {
  form ??= parseYaml(projectForm) as UiSchemaManifest;
  const text = (form?.spec?.fields as Record<string, { help?: Record<string, string> }> | undefined)?.[path]?.help;
  return text?.[language] ?? text?.[language.split("-")[0]] ?? text?.en ?? "";
}

export function settingsIndex(t: T, language: string): Setting[] {
  return [
    { tab: "general", key: "title", label: t("projectSettings.field.title"), about: help("title", language) },
    { tab: "general", key: "description", label: t("projectSettings.field.description"), about: help("description", language) },
    ...QUOTA_DIMENSIONS.map((dimension) => ({
      tab: "general" as const,
      key: `quotas.${dimension}`,
      label: t(`organization.field.quota.${dimension}`),
      about: help(`quotas.${dimension}`, language),
    })),
    { tab: "general", key: "organization", label: t("projectSettings.general.organization"), about: t("projectSettings.about.organization") },
    { tab: "general", key: "duplicate", label: t("projectSettings.about.duplicateLabel"), about: t("projectSettings.about.duplicate") },
    { tab: "members", key: "members", label: t("projectSettings.tab.members"), about: t("projectSettings.about.members") },
    { tab: "roles", key: "roles", label: t("projectSettings.tab.roles"), about: t("projectSettings.about.roles") },
    { tab: "service-accounts", key: "service-accounts", label: t("projectSettings.tab.service-accounts"), about: t("projectSettings.about.serviceAccounts") },
    { tab: "access", key: "access", label: t("projectSettings.access.title"), about: t("projectSettings.access.lead") },
    { tab: "explorer", key: "explorer", label: t("access.explorer.title"), about: t("access.explorer.lead") },
    { tab: "danger", key: "handover", label: t("projectSettings.danger.handoverTitle"), about: t("projectSettings.danger.handoverLead") },
    { tab: "danger", key: "delete", label: t("projectSettings.danger.title"), about: t("projectSettings.danger.lead") },
  ];
}

/** Lowercase without accents, so "kvota" finds "Kvóta". */
function fold(text: string): string {
  return text.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
}

/** The settings whose name or description holds every word of `query`. */
export function findSettings(settings: Setting[], query: string): Setting[] {
  const words = fold(query).split(/\s+/).filter(Boolean);
  if (words.length === 0) return [];
  return settings.filter((setting) => {
    const text = fold(`${setting.label} ${setting.about}`);
    return words.every((word) => text.includes(word));
  });
}

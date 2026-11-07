/** The tabs of `/projects/{project}/settings/{tab}`, in the order Architecture/09 §14.2 lists them. */
export const PROJECT_SETTINGS_TABS = [
  "general",
  "members",
  "roles",
  "service-accounts",
  "access",
  "explorer",
  "danger",
] as const;

export type ProjectSettingsTab = (typeof PROJECT_SETTINGS_TABS)[number];

export function isProjectSettingsTab(value: string): value is ProjectSettingsTab {
  return (PROJECT_SETTINGS_TABS as readonly string[]).includes(value);
}

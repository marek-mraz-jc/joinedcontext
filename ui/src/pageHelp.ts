/**
 * Help for the page a person is on (T-3269): which of the Portal's main pages it is, the words
 * that say what the page is for and its main steps (`pageHelp.{key}` in the locale files), and the
 * page of the User Guide that goes deeper, when the installation serves the guide (UI-02, DP-11).
 */
export interface PageHelp {
  key: string;
  /** The User Guide page, as `guideUrl` joins it to the installation's documentation address. */
  guide: string;
}

/** A project's sections by their address, and the guide page each one has. */
const SECTIONS: Record<string, PageHelp> = {
  spaces: { key: "spaces", guide: "User-Guide/02-organizations-projects-spaces" },
  models: { key: "models", guide: "User-Guide/03-data-models" },
  datasources: { key: "datasources", guide: "User-Guide/04-pipelines" },
  pipelines: { key: "pipelines", guide: "User-Guide/04-pipelines" },
  endpoints: { key: "endpoints", guide: "User-Guide/05-endpoints-and-sharing" },
  policies: { key: "policies", guide: "User-Guide/05-endpoints-and-sharing" },
  ckan: { key: "ckan", guide: "User-Guide/09-export-import" },
  dashboards: { key: "dashboards", guide: "User-Guide/06-dashboards" },
  explore: { key: "explore", guide: "User-Guide/06-dashboards" },
  approvals: { key: "approvals", guide: "User-Guide/07-users-roles-approvals" },
  activity: { key: "activity", guide: "User-Guide/07-users-roles-approvals" },
  assistant: { key: "assistant", guide: "User-Guide/08-working-with-ai-agents" },
  knowledge: { key: "knowledge", guide: "User-Guide/08-working-with-ai-agents" },
  workspaces: { key: "workspaces", guide: "User-Guide/02-organizations-projects-spaces" },
  apps: { key: "apps", guide: "User-Guide/11-apps" },
  settings: { key: "settings", guide: "User-Guide/12-managing-organization-and-projects" },
};

/** Every main page that has help: what the help tests hold to words in every locale. */
export const HELPED: string[] = ["home", "organization", ...Object.keys(SECTIONS)];

/** The help of the page at `pathname`, or nothing for a page without one. */
export function helpFor(pathname: string): PageHelp | undefined {
  const path = pathname.replace(/\/+$/, "") || "/";
  if (path === "/") return { key: "home", guide: "User-Guide/01-getting-started" };
  if (path === "/organization" || path.startsWith("/organization/")) {
    return { key: "organization", guide: "User-Guide/12-managing-organization-and-projects" };
  }
  const [, root, , section] = path.split("/");
  return root === "projects" && section ? SECTIONS[section] : undefined;
}

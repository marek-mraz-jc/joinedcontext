/**
 * The routes the live walkers open (T-2730, T-3986): every route of the router with one real item for
 * each name in it, and every create and edit form at its own address. `walker.spec.ts` judges them
 * as they open; `walk-dialogs.spec.ts` opens the dialogs on them.
 */
import type { Page } from "@playwright/test";

export const PROJECT = "helsinki";
export const P = `/projects/${PROJECT}`;

/** The sections that open a create and an edit form at their own address (`NEW_FORMS`). */
const FORMS = ["spaces", "endpoints", "pipelines", "policies", "subscriptions", "csrs", "datasources", "syncsources"];
const SECTIONS = ["spaces", "endpoints", "subscriptions", "datasources", "pipelines", "dashboards", "apps", "syncsources", "policies", "csrs"];
const ORGANIZATION_TABS = ["settings", "people", "members", "roles", "groups", "service-accounts", "projects"];
const SETTINGS_TABS = ["general", "members", "roles", "service-accounts", "access", "danger"];

/** One visit: the route it stands for (the key of the allow list) and the address opened. */
export interface Visit {
  route: string;
  address: string;
}

/** The first link on a list page that opens one of its items, as a person would find it. */
async function firstItem(page: Page, list: string, pattern: RegExp): Promise<string | undefined> {
  await page.goto(`${list}?lang=en`, { waitUntil: "load" });
  await page.locator("main h1:visible").first().waitFor({ timeout: 30_000 }).catch(() => undefined);
  // A list that loads after its heading (the catalogue's search) gets 15 s to draw its first item.
  for (let tries = 0; tries < 15; tries += 1) {
    const hrefs = await page.locator("main a[href]").evaluateAll((links) => links.map((link) => link.getAttribute("href") ?? ""));
    const found = hrefs.find((href) => pattern.test(href.split(/[?#]/)[0]));
    if (found) return found;
    await page.waitForTimeout(1_000);
  }
  return undefined;
}

/** The first stored item of a section, through the same API the list reads. */
async function firstName(page: Page, section: string): Promise<string | undefined> {
  const answer = await page.request.get(`/api/v1${P}/${section}`);
  if (!answer.ok()) return undefined;
  const items = ((await answer.json()) as { items?: { metadata?: { name?: string } }[] }).items ?? [];
  return items[0]?.metadata?.name;
}

/**
 * Every route of the router, with a real item for each name it takes. The Organization page's
 * Endpoints tab is an administration view (PF-61, T-2877): the steward, who administers the
 * organization on dev, visits it; the viewer is checked not to reach it (`endpointsRefused`).
 */
export async function visits(page: Page, who: string): Promise<Visit[]> {
  const found: Visit[] = [
    { route: "/", address: "/" },
    { route: "/endpoints", address: "/endpoints" },
    { route: "/organization", address: "/organization" },
    ...["/catalogue", "/glossary", "/connectors", "/ai-connectors", "/settings", "/playground"].map((route) => ({ route, address: route })),
    ...[...ORGANIZATION_TABS, ...(who === "steward" ? ["endpoints"] : [])].map((tab) => ({
      route: "/organization/$tab",
      address: `/organization/${tab}`,
    })),
    { route: "/organization/$tab/$", address: "/organization/people/new" },
    { route: "/organization/$tab/$", address: "/organization/groups/new" },
    ...[
      "home", "inbox", "workspaces", "activity", "grafana", "health", "approvals", "access", "settings", "models", "explore", "ckan", "mcp", "federation", "import", "assistant", "shared",
      // The knowledge pages, back since T-5328 linked every source row and gave the refused
      // conversations page its purpose line.
      "knowledge", "knowledge/conversations",
    ].map((page) => ({
      route: `/projects/$project/${page}`,
      address: `${P}/${page}`,
    })),
    { route: "/projects/$project/workspaces/new", address: `${P}/workspaces/new` },
    { route: "/projects/$project/secrets/new", address: `${P}/secrets/new` },
    { route: "/projects/$project/spaces/complete", address: `${P}/spaces/complete` },
    { route: "/projects/$project/spaces/upload", address: `${P}/spaces/upload` },
    ...SETTINGS_TABS.map((tab) => ({ route: "/projects/$project/settings/$tab", address: `${P}/settings/${tab}` })),
    ...["members", "roles", "service-accounts"].map((tab) => ({
      route: "/projects/$project/settings/$tab/$",
      address: `${P}/settings/${tab}/new`,
    })),
    ...SECTIONS.map((section) => ({ route: "/projects/$project/$plural", address: `${P}/${section}` })),
    ...FORMS.map((section) => ({ route: "/projects/$project/$plural/new", address: `${P}/${section}/new` })),
  ];
  const items: [string, string, RegExp][] = [
    ["/projects/$project/$plural/$name", `${P}/spaces`, new RegExp(`^${P}/spaces/(?!complete$)[^/]+$`)],
    ["/projects/$project/$plural/$name", `${P}/endpoints`, new RegExp(`^${P}/endpoints/[^/]+$`)],
    ["/projects/$project/$plural/$name", `${P}/apps`, new RegExp(`^${P}/apps/[^/]+$`)],
    ["/projects/$project/knowledge/$source", `${P}/knowledge`, new RegExp(`^${P}/knowledge/(?!conversations$)[^/]+$`)],
    ["/projects/$project/models/$name", `${P}/models`, new RegExp(`^${P}/models/[^/]+$`)],
    ["/projects/$project/approvals/$id", `${P}/approvals`, new RegExp(`^${P}/approvals/[^/]+$`)],
    ["/projects/$project/workspaces/$name/compare", `${P}/workspaces`, new RegExp(`^${P}/workspaces/[^/]+/compare$`)],
    ["/organization/$tab/$", "/organization/people", /^\/organization\/people\/(?!new$)[^/]+$/],
    ["/organization/$tab/$", "/organization/groups", /^\/organization\/groups\/(?!new$)[^/]+$/],
    // A dataset page, back since T-4949 put the publisher's commands and JSON in code blocks.
    ["/catalogue/$name", "/catalogue", /^\/catalogue\/[^/]+$/],
  ];
  for (const [route, list, pattern] of items) {
    const address = await firstItem(page, list, pattern);
    if (address) found.push({ route, address });
  }
  const copy = found.find((visit) => visit.route.endsWith("/compare"))?.address.replace(/\/compare$/, "");
  if (copy) {
    found.push({ route: "/projects/$project/workspaces/$name/try-it", address: `${copy}/try-it` });
    found.push({ route: "/projects/$project/workspaces/$name/bring-back", address: `${copy}/bring-back` });
  }
  // The public setup page of a named MCP server (T-4169), back since T-4949 let it reach its server.
  const server = await firstName(page, "mcpservers");
  if (server) found.push({ route: "/connect/$project/$name", address: `/connect/${PROJECT}/${encodeURIComponent(server)}` });
  for (const section of FORMS) {
    const name = await firstName(page, section);
    if (name) found.push({ route: "/projects/$project/$plural/$name/edit", address: `${P}/${section}/${encodeURIComponent(name)}/edit` });
  }
  return found;
}

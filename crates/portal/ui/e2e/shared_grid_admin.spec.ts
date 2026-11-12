import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-6749: admin lists, part 2, on the shared data grid: People (the server pages and searches the
// realm), Agents (an organization list) and Data sources (a project list), walked in the built
// bundle with the API answered from the browser. Each state is shot at 1440 and 400 px into
// test-results/T-6749 for the UI judge.

const IDENTITY = { subject: "b7c1e0f4", username: "jana", name: "Jana Kováčová", email: "jana@hel.fi", roles: ["portal-admin"] };
const PROJECTS = { apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] };
const GRANTS = {
  bootstrap: false,
  grants: [
    {
      role: "org-admin",
      binding: "admins",
      rule: {
        kinds: ["Person", "AgentProfile", "DataSource", "RoleBinding", "Role", "Group", "Organization", "Project"],
        verbs: ["read", "create", "update", "disable", "delete", "propose", "approve"],
      },
    },
  ],
};

const person = (id: string, first: string, last: string, extra: Record<string, unknown> = {}) => ({
  id,
  email: `${first.toLowerCase()}@hel.fi`,
  firstName: first,
  lastName: last,
  enabled: true,
  emailVerified: true,
  requiredActions: [],
  createdAt: "2026-09-01T08:00:00Z",
  lastSeen: "2026-11-10T09:30:00Z",
  locale: "fi",
  pendingDeletion: null,
  ...extra,
});
const PEOPLE = [
  person("jana", "Jana", "Kováčová"),
  person("aino", "Aino", "Virtanen", { emailVerified: false, lastSeen: null }),
  person("marek", "Marek", "Mráz", { enabled: false }),
];
const agent = (name: string, owner: string, state: string) => ({
  kind: "AgentProfile",
  project: "helsinki",
  name,
  title: null,
  owner: { type: "person", name: owner },
  actsAs: "self",
  lastActivity: null,
  spend: { used: null, cap: null },
  state,
  spaces: [],
  endpoints: [],
});
const AGENTS = [agent("air-quality-helper", "jana@hel.fi", "active"), agent("transport-digest", "aino@hel.fi", "active"), agent("events-bot", "marek@hel.fi", "paused")];
const source = (name: string, url: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataSource",
  metadata: { name, namespace: "helsinki" },
  spec: { type: "http", http: { url } },
});
const SOURCES = [source("espoo-air", "https://api.espoo.fi/air"), source("hsl-stops", "https://api.digitransit.fi/stops"), source("vantaa-parking", "https://api.vantaa.fi/parking")];

type Mode = "filled" | "empty" | "error";

async function stub(page: Page, mode: Mode): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const failed = () =>
      route.fulfill({ status: 503, contentType: "application/problem+json", body: JSON.stringify({ type: "about:blank", title: "Unavailable", status: 503, detail: "The configuration store did not answer. Try again in a minute." }) });
    const items = (rows: unknown[]) => (mode === "empty" ? [] : rows);
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname.endsWith("/permissions/me")) return json({ project: url.pathname.includes("/helsinki/") ? "helsinki" : "org", ...GRANTS });
    if (url.pathname === "/api/v1/projects") return json(PROJECTS);
    if (url.pathname === "/api/v1/organization/people") return mode === "error" ? failed() : json({ items: items(PEOPLE) });
    if (url.pathname === "/api/v1/organization/agents") return mode === "error" ? failed() : json({ items: items(AGENTS) });
    if (url.pathname.endsWith("/helsinki/datasources")) return mode === "error" ? failed() : json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: items(SOURCES) });
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

const PAGES = [
  { slug: "people", path: "/organization/people", first: /Jana Kováčová/ },
  { slug: "agents", path: "/organization/agents", first: /air-quality-helper/ },
  { slug: "datasources", path: "/projects/helsinki/datasources", first: /espoo-air/ },
];

for (const width of [1440, 400]) {
  for (const target of PAGES) {
    test(`${target.slug} on the shared grid at ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
      const shot = (state: string) => page.screenshot({ path: `test-results/T-6749/${target.slug}-${state}-${width}.png`, fullPage: true });

      await stub(page, "filled");
      await page.goto(`${target.path}?lang=en`);
      await expect(page.locator(".jc-data-grid").getByText(target.first).first()).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot("filled");

      if (width === 1440) {
        await page.locator(".jc-data-grid thead .jc-grid-meta-btn").nth(1).click();
        await expect(page.getByRole("menu")).toBeVisible();
        await shot("menu");
        await page.keyboard.press("Escape");
      }
      await page.getByRole("button", { name: "Arrange columns" }).click();
      await expect(page.getByRole("dialog", { name: "Arrange columns" })).toBeVisible();
      await shot("arrange");
      await page.getByRole("button", { name: "Done" }).click();

      await page.unrouteAll({ behavior: "ignoreErrors" });
      await stub(page, "empty");
      await page.goto(`${target.path}?lang=en`);
      await expect(page.locator(".jc-data-grid")).toBeVisible();
      await shot("empty");

      await page.unrouteAll({ behavior: "ignoreErrors" });
      await stub(page, "error");
      await page.goto(`${target.path}?lang=en`);
      await expect(page.getByText(/configuration store did not answer/)).toBeVisible();
      await shot("error");
    });
  }
}

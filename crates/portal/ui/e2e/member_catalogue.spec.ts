/**
 * The member shell and the signed-in catalogue on stubs (ADR-N-067 §2.4, T-6411).
 *
 * A member (signed in, no project) opens "/" and lands on /catalogue in the member shell; the
 * datasets they may read come from GET /api/v1/catalogue/readable: an Access filter of the one list,
 * each marked "Granted" and by what grants it, never a box above the list (T-7389).
 * A dataset by its Endpoint, /catalogue/endpoint/{slug}, shows the grid's refusals (T-6376): a
 * visitor of a non-public Endpoint "sign in", a member without a grant "no access". SHOTS_DIR
 * takes the pictures at 1440 and 400 px.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";
import { doorsAsRest } from "./manifestReads";

// The projects list is a Portal door (portal_project_list, T-7177): the stub answers its REST address.
test.beforeEach(({ page }) => doorsAsRest(page, ["portal_project_list"]));

const SLUG = "inc2dents4h7m2qz4tv6xh3n5jb";
const IDENTITY = { subject: "de300000-0000-4000-8000-000000000010", username: "demo.analyst", name: "Traffic Analyst", email: "demo.analyst@hel.fi", roles: [], front: "portal" };
const OPEN = (name: string, title: string, publisher: string, theme: string) => ({
  name,
  title,
  notes: `${title}, published as open data and updated every hour.`,
  publisher: { name: publisher, title: publisher === "bbsk" ? "Banskobystrický samosprávny kraj" : "Mesto Banská Bystrica" },
  licence: { id: "cc-by-4.0", title: "Creative Commons Attribution 4.0" },
  formats: ["CSV", "NGSI-LD", "OGCFEAT"],
  themes: [theme],
  modified: "2026-11-18T06:00:00Z",
});
const facet = (value: string, label: string, count: number) => ({ value, label, count });
const PAGE = {
  total: 3,
  page: 1,
  pageSize: 20,
  datasets: [
    OPEN("bb-pm10", "Air quality in Banská Bystrica — PM10 by station", "banskabystrica", "ENVI"),
    OPEN("bb-parking", "Parking occupancy in Banská Bystrica", "banskabystrica", "TRAN"),
    OPEN("bbsk-kpi", "Indicators of the region", "bbsk", "ECON"),
  ],
  facets: {
    publisher: [facet("banskabystrica", "Mesto Banská Bystrica", 2), facet("bbsk", "Banskobystrický samosprávny kraj", 1)],
    theme: [facet("ENVI", "Environment", 1), facet("TRAN", "Transport", 1), facet("ECON", "Economy and finance", 1)],
    format: [facet("CSV", "CSV", 3)],
    licence: [facet("cc-by-4.0", "Creative Commons Attribution 4.0", 3)],
    spatial: [],
    year: [],
  },
  unavailable: [],
};
const row = (name: string, title: string, access: string, group?: string) => ({
  project: "helsinki",
  name,
  slug: name === "incidents" ? SLUG : `${name.slice(0, 4)}2m7q4tv6xh3n5jb2ryd3wcf`,
  title,
  description: `${title} of the city, read live from its Endpoint.`,
  representations: ["ngsi-ld"],
  access,
  ...(group ? { group } : {}),
});
const MEMBER = [row("bikes", "Bike counters", "public"), row("counts", "Traffic counts", "organization")];
const ANALYST = [...MEMBER, row("incidents", "Traffic incidents", "group", "traffic-analysts")];

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  if (dir) await page.screenshot({ path: `${dir}/${name}.png`, fullPage: true });
}

async function stub(page: Page, who: "visitor" | typeof MEMBER | typeof ANALYST, endpoint: 200 | 401 | 403): Promise<void> {
  await page.route(`**/api/endpoint/${SLUG}/**`, (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
      route.fulfill({ status, headers, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    if (endpoint !== 200) {
      return json({ title: endpoint === 401 ? "Authentication Required" : "Forbidden", status: endpoint, detail: endpoint === 401 ? "sign in" : "no policy grants you this dataset" }, endpoint);
    }
    if (url.pathname.endsWith("/schema/index.json")) return json({ models: [{ name: "traffic", version: 1 }] });
    if (url.pathname.endsWith("/schema/v1/json-schema")) return json({ $defs: { TrafficIncident: { properties: { name: { type: "string" }, severity: { type: "string" } } } } });
    if (url.pathname.endsWith("/access/attributes")) {
      return json({ type: "TrafficIncident", attributes: ["name", "severity"].map((name) => ({ name, read: true, write: false, delete: false, reason: "not_in_grant" })) });
    }
    if (url.pathname.endsWith("/ngsi-ld/v1/entities")) {
      return json([{ id: "urn:ngsi-ld:TrafficIncident:1", type: "TrafficIncident", name: { type: "Property", value: "Mannerheimintie closed" }, severity: { type: "Property", value: "high" } }], 200, { "NGSILD-Results-Count": "1" });
    }
    return json({ title: "Not Found", status: 404 }, 404);
  });
  await page.route("**/api/v1/**", (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    const signedIn = who !== "visitor";
    if (url.pathname.endsWith("/auth/me")) return signedIn ? json(IDENTITY) : json({ title: "Unauthorized", status: 401 }, 401);
    if (url.pathname === "/api/v1/projects") return json({ apiVersion: "v1", kind: "List", items: [] });
    if (url.pathname === "/api/v1/catalogue") return json(PAGE);
    if (url.pathname === "/api/v1/catalogue/readable") {
      return json({ datasets: signedIn ? who : [row("bikes", "Bike counters", "public")], unanswered: 0 });
    }
    return json({ apiVersion: "v1", kind: "List", items: [] });
  });
}

/** The Access filter: the sidebar at 1440, the Filters sheet on a phone. */
async function accessFilter(page: Page, width: number) {
  if (width < 768) await page.getByRole("button", { name: /^Filters/ }).click();
  const scope = width < 768 ? page.getByRole("dialog") : page.getByRole("complementary", { name: "Filters" });
  return scope.getByRole("group", { name: "Access" });
}

/** The list starts right under the title and the search, with no box between (T-7389). */
async function listAtTop(page: Page): Promise<void> {
  const list = page.getByRole("region", { name: "Datasets" });
  await expect(list.getByRole("listitem").first()).toBeVisible();
  const box = await list.boundingBox();
  expect(box?.y ?? 9999).toBeLessThan(330);
  await expect(page.getByRole("region", { name: "Datasets you may read" })).toHaveCount(0);
}

test.describe("a member's catalogue", () => {
  for (const width of [1440, 400]) {
    test(`a member lands on the catalogue in the member shell, granted datasets first and marked (${width} px)`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stub(page, MEMBER, 200);
      await page.goto("/?lang=en");
      await expect(page).toHaveURL(/\/catalogue/);
      const nav = page.getByRole("navigation", { name: "Member menu" });
      await expect(nav.getByRole("link", { name: "Catalogue" })).toBeVisible();
      await expect(nav.getByRole("link", { name: "AI connectors" })).toBeVisible();
      await expect(nav.getByRole("link", { name: "Settings" })).toBeVisible();
      await expect(page.locator('a[href^="/projects/"]')).toHaveCount(0);
      await listAtTop(page);
      const first = page.getByRole("region", { name: "Datasets" }).locator("ul > li").first();
      await expect(first.getByRole("link", { name: "Traffic counts" })).toBeVisible();
      await expect(first.getByText("Granted", { exact: true })).toBeVisible();
      await expect(first.getByText("helsinki · Your organization")).toBeVisible();
      await expect(page.getByText("4 datasets")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `member-catalogue-en-${width}`);
    });

    test(`an analyst filters to what they are granted, and opens it (${width} px)`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stub(page, ANALYST, 200);
      await page.goto("/catalogue?lang=en");
      const access = await accessFilter(page, width);
      const mine = access.getByRole("checkbox", { name: /Granted to me/ });
      await expect(mine).toBeEnabled();
      await mine.check();
      if (width < 768) await page.getByRole("button", { name: "Show 2 datasets" }).click();
      await expect(page).toHaveURL(/access=granted/);
      await expect(page.getByText("2 datasets")).toBeVisible();
      const list = page.getByRole("region", { name: "Datasets" });
      await expect(list.locator("ul > li")).toHaveCount(2);
      await expect(list.getByText("helsinki · Group traffic-analysts")).toBeVisible();
      await expect(list.getByRole("group", { name: "Chosen filters" }).getByRole("button", { name: "Remove filter Access: Granted to me" })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `analyst-catalogue-granted-en-${width}`);
      await list.getByRole("link", { name: "Traffic incidents" }).click();
      await expect(page).toHaveURL(new RegExp(`/catalogue/endpoint/${SLUG}`));
      await expect(page.getByRole("heading", { level: 1, name: "Traffic incidents" })).toBeVisible();
      await expect(page.getByRole("grid").getByText("Mannerheimintie closed")).toBeVisible();
      await shot(page, `analyst-dataset-rows-en-${width}`);
    });

    test(`a member with nothing internal shared sees the list at once and Granted to me (0) saying why (${width} px)`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stub(page, [row("bikes", "Bike counters", "public")], 200);
      await page.goto("/catalogue?lang=en");
      await listAtTop(page);
      await expect(page.getByText("3 datasets, none granted to you yet")).toBeVisible();
      await shot(page, `member-catalogue-empty-en-${width}`);
      const access = await accessFilter(page, width);
      const mine = access.getByRole("checkbox", { name: /Granted to me/ });
      await expect(mine).toHaveAttribute("aria-disabled", "true");
      await expect(access.locator("p", { hasText: /A data steward of your organization can grant your group access/ })).toBeVisible();
      await mine.click({ force: true });
      await expect(mine).not.toBeChecked();
      expect(await axeViolations(page)).toEqual([]);
      if (width < 768) await shot(page, `member-catalogue-empty-filters-en-${width}`);
    });

    test(`a person in no project of this Organization's host is told who adds them, not shown an error (${width} px)`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stub(page, MEMBER, 200);
      // The Organization host's gate (organization_host.rs) answers 403 to everyone outside its projects (T-7317).
      await page.route("**/api/v1/catalogue/readable", (route) =>
        route.fulfill({
          status: 403,
          contentType: "application/problem+json",
          body: JSON.stringify({ type: "https://joinedcontext.com/errors/forbidden", title: "Forbidden", status: 403, detail: "this Portal is bbsk's; you belong to none of its projects" }),
        }),
      );
      await page.goto("/catalogue?lang=en");
      await listAtTop(page);
      await expect(page.getByText("3 datasets; you are not in a project of this organization yet")).toBeVisible();
      if (width < 768) await shot(page, `member-catalogue-no-project-page-en-${width}`);
      const access = await accessFilter(page, width);
      await expect(access.locator("p", { hasText: /A project owner or an administrator of this organization can add you/ })).toBeVisible();
      await expect(page.getByRole("alert")).toHaveCount(0);
      await expect(page.getByText(/belong to none of its projects/)).toHaveCount(0);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `member-catalogue-no-project-en-${width}`);
    });

    test(`an Organization with no dataset yet says so instead of listing another Organization's (${width} px)`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stub(page, [row("bikes", "Bike counters", "public")], 200);
      // The marek-mraz host's catalogue holds only its own projects' datasets: none yet (T-7389).
      await page.route("**/api/v1/catalogue?*", (route) =>
        route.fulfill({ contentType: "application/json", body: JSON.stringify({ ...PAGE, total: 0, datasets: [], facets: { publisher: [], theme: [], format: [], licence: [], spatial: [], year: [] } }) }),
      );
      await page.route("**/api/v1/catalogue", (route) =>
        route.fulfill({ contentType: "application/json", body: JSON.stringify({ ...PAGE, total: 0, datasets: [], facets: { publisher: [], theme: [], format: [], licence: [], spatial: [], year: [] } }) }),
      );
      await page.goto("/catalogue?lang=en");
      const list = page.getByRole("region", { name: "Datasets" });
      await expect(list.getByText("No public dataset yet")).toBeVisible();
      await expect(list.getByText(/once a project publishes an endpoint/)).toBeVisible();
      await expect(page.getByText("Air quality in Banská Bystrica — PM10 by station")).toHaveCount(0);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `organization-catalogue-empty-en-${width}`);
    });

    test(`a non-public dataset asks a visitor to sign in and tells a member without a grant no access (${width} px)`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stub(page, "visitor", 401);
      await page.goto(`/catalogue/endpoint/${SLUG}?lang=en`);
      await expect(page.getByText("Sign in to see this dataset")).toBeVisible();
      await expect(page.getByRole("grid")).toHaveCount(0);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `dataset-signed-out-en-${width}`);
      await page.unrouteAll();
      await stub(page, MEMBER, 403);
      await page.reload();
      await expect(page.getByText("You have no access to this dataset")).toBeVisible();
      await expect(page.getByRole("grid")).toHaveCount(0);
      await shot(page, `dataset-no-access-en-${width}`);
    });
  }
});

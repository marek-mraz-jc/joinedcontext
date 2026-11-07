/**
 * The member shell and the signed-in catalogue on stubs (ADR-N-067 §2.4, T-6411).
 *
 * A member (signed in, no project) opens "/" and lands on /catalogue in the member shell; the
 * datasets they may read come from GET /api/v1/catalogue/readable, each marked by what grants it.
 * A dataset by its Endpoint, /catalogue/endpoint/{slug}, shows the grid's refusals (T-6376): a
 * visitor of a non-public Endpoint "sign in", a member without a grant "no access". SHOTS_DIR
 * takes the pictures at 1440 and 400 px.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

const SLUG = "inc2dents4h7m2qz4tv6xh3n5jb";
const IDENTITY = { subject: "de300000-0000-4000-8000-000000000010", username: "demo.analyst", name: "Traffic Analyst", email: "demo.analyst@hel.fi", roles: [], front: "portal" };
const PAGE = { total: 0, page: 1, pageSize: 20, datasets: [], facets: { publisher: [], theme: [], format: [], licence: [], spatial: [], year: [] }, unavailable: [] };
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

test.describe("a member's catalogue", () => {
  for (const width of [1440, 400]) {
    test(`a member lands on the catalogue in the member shell, each dataset marked (${width} px)`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stub(page, MEMBER, 200);
      await page.goto("/?lang=en");
      await expect(page).toHaveURL(/\/catalogue/);
      const nav = page.getByRole("navigation", { name: "Member menu" });
      await expect(nav.getByRole("link", { name: "Catalogue" })).toBeVisible();
      await expect(nav.getByRole("link", { name: "AI connectors" })).toBeVisible();
      await expect(nav.getByRole("link", { name: "Settings" })).toBeVisible();
      await expect(page.locator('a[href^="/projects/"]')).toHaveCount(0);
      await expect(page.getByRole("link", { name: "Open dataset: Traffic counts" })).toBeVisible();
      await expect(page.getByText("Your organization", { exact: true })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `member-catalogue-en-${width}`);
    });

    test(`an analyst also reads what their group is granted, and opens it (${width} px)`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stub(page, ANALYST, 200);
      await page.goto("/catalogue?lang=en");
      await expect(page.getByText("Group traffic-analysts")).toBeVisible();
      await shot(page, `analyst-catalogue-en-${width}`);
      await page.getByRole("link", { name: "Open dataset: Traffic incidents" }).click();
      await expect(page).toHaveURL(new RegExp(`/catalogue/endpoint/${SLUG}`));
      await expect(page.getByRole("heading", { level: 1, name: "Traffic incidents" })).toBeVisible();
      await expect(page.getByRole("grid").getByText("Mannerheimintie closed")).toBeVisible();
      await shot(page, `analyst-dataset-rows-en-${width}`);
    });

    test(`a member with nothing internal shared is told what to do next (${width} px)`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stub(page, [row("bikes", "Bike counters", "public")], 200);
      await page.goto("/catalogue?lang=en");
      await expect(page.getByText("No internal dataset is shared with you yet")).toBeVisible();
      await shot(page, `member-catalogue-empty-en-${width}`);
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

import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-4989: the endpoint lists (a project's endpoints, what other projects share with it, and the
// Organization's Endpoints tab) read as one line a row at 1440 px: the surfaces in one summary,
// the name no underlined link, the row opening the endpoint. The API is answered in the browser.

const API = "joinedcontext.com/v1alpha1";
const ALL = ["ngsi-ld", "mcp", "csv", "ogc-features", "sta", "odata", "websocket", "ical"];
const SHOTS = process.env.JC_SHOTS_DIR ?? "test-results/endpoint-rows";

function endpoint(project: string, name: string, title: string, space: string, extra: Record<string, unknown> = {}) {
  return {
    apiVersion: API,
    kind: "Endpoint",
    metadata: { name, namespace: project, title: { en: title }, labels: { "joinedcontext.com/space": space } },
    spec: { contextSpaceRef: space, slug: `${name}-k7m2qz4tv6xh3n5jb2`, audience: "public", enabledRepresentations: ALL, ...extra },
    status: { phase: "Live" },
  };
}

const HELSINKI = [
  endpoint("helsinki", "air-quality", "Air quality and weather in Helsinki", "air-quality-and-weather-observations"),
  endpoint("helsinki", "bikes", "City bike stations", "liikenne", {
    audience: "project-list",
    allowedProjects: ["espoo", "vantaa", "tampere", "turku"],
  }),
  endpoint("helsinki", "events", "Helsinki events", "tapahtumat", { audience: "organization", enabledRepresentations: ["ngsi-ld"] }),
  {
    ...endpoint("helsinki", "app-air-quality", "Air quality app", "air-quality-and-weather-observations"),
    metadata: {
      name: "app-air-quality",
      namespace: "helsinki",
      title: { en: "Air quality app" },
      labels: { "joinedcontext.com/space": "air-quality-and-weather-observations" },
      annotations: { "joinedcontext.com/generated-by": "portal/app-reconciler" },
    },
  },
];
const ESPOO = [endpoint("espoo", "parking", "Parking places in Espoo", "pysakointi-ja-liikenne-espoo")];

const MAY = {
  project: "helsinki",
  bootstrap: false,
  grants: [
    {
      role: "org-admin",
      binding: "admins",
      rule: {
        kinds: ["Endpoint", "SharedSpaceReference", "Project", "Organization", "Role", "RoleBinding", "Group"],
        verbs: ["read", "propose", "approve", "delete"],
      },
    },
  ],
};

async function stubApi(page: Page): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    const list = (items: unknown[]) => json({ apiVersion: API, kind: "List", items });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "s", username: "jana", name: "Jana", email: "jana@hel.fi", roles: ["portal-admin"] });
    }
    if (path.endsWith("/permissions/me")) return json(MAY);
    if (path.endsWith("/events")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
    if (path === "/api/v1/projects") return list([{ name: "helsinki" }, { name: "espoo" }]);
    if (path === "/api/v1/projects/helsinki/endpoints") return list(HELSINKI);
    if (path === "/api/v1/projects/espoo/endpoints") return list(ESPOO);
    if (path === "/api/v1/endpoints") return list([...HELSINKI, ...ESPOO]);
    const one = /^\/api\/v1\/projects\/([^/]+)\/endpoints\/([^/]+)$/.exec(path);
    const found = one && [...HELSINKI, ...ESPOO].find((item) => item.metadata.namespace === one[1] && item.metadata.name === one[2]);
    if (found) return json(found);
    return list([]);
  });
}

/** Every body row of a table is one line: no taller than a row holding one badge or button. */
async function expectOneLineRows(table: Locator): Promise<void> {
  const rows = table.locator("tbody tr");
  await expect(rows.first()).toBeVisible();
  const heights = await rows.evaluateAll((all) => all.map((row) => Math.round(row.getBoundingClientRect().height)));
  expect(heights.every((height) => height <= 64), `row heights ${heights.join(", ")}`).toBe(true);
  const underlined = await table.locator("tbody a").evaluateAll((links) =>
    links.filter((link) => getComputedStyle(link).textDecorationLine.includes("underline")).map((link) => link.textContent),
  );
  expect(underlined, "underlined links in the rows").toEqual([]);
  // Every column, the actions included, fits its frame: nothing waits beyond a sideways scroll.
  const sideways = await table.evaluate((element) => {
    const frame = element.parentElement as HTMLElement;
    return frame.scrollWidth - frame.clientWidth;
  });
  expect(sideways, "the table scrolls sideways").toBeLessThanOrEqual(1);
}

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.use({ viewport: { width, height: 1000 } });

    test("a project's endpoints and the ones shared with it", async ({ page }) => {
      await stubApi(page);
      await page.goto("/projects/helsinki/endpoints?lang=en");
      const own = page.getByRole("table", { name: "Endpoints" });
      await expect(own.getByRole("link", { name: "City bike stations" })).toBeVisible();
      // Shared Endpoints are rows of the same grid, with a Source chip (T-6169).
      await expect(own.getByText("Parking places in Espoo")).toBeVisible();
      if (width === 1440) {
        await expectOneLineRows(own);
        // The rest of what an endpoint serves is one Tab away, read out and shown.
        const row = own.getByRole("row").filter({ hasText: "Air quality and weather in Helsinki" });
        await expect(row.getByText("NGSI-LD, MCP, OGC Features")).toBeVisible();
        const more = row.getByText("+5 more");
        await more.focus();
        await expect(row.getByRole("tooltip")).toBeVisible();
        await expect(row.getByRole("tooltip")).toHaveText("CSV, SensorThings, OData, WebSocket, iCal");
        await page.screenshot({ path: `${SHOTS}/projects_helsinki_endpoints-tip-${width}.png` });
        await page.keyboard.press("Escape");
      }
      expect(await axeViolations(page)).toEqual([]);
      await page.screenshot({ path: `${SHOTS}/projects_helsinki_endpoints-${width}.png`, fullPage: true });
      // A click anywhere on the row opens the endpoint's page, where each surface link is.
      await own.getByRole("row").filter({ hasText: "City bike stations" }).getByText("City bike stations").click();
      await expect(page).toHaveURL(/\/projects\/helsinki\/endpoints\/bikes/);
      await expect(page.getByRole("link", { name: "ngsi-ld" }).first()).toBeVisible();
    });

    test("the Organization's endpoints", async ({ page }) => {
      await stubApi(page);
      await page.goto("/organization/endpoints?lang=en");
      const table = page.getByRole("table", { name: "All endpoints" });
      await expect(table.getByRole("link", { name: "Parking places in Espoo" })).toBeVisible();
      await page.screenshot({ path: `${SHOTS}/organization_endpoints-${width}.png`, fullPage: true });
      if (width === 1440) {
        await expectOneLineRows(table);
      }
      expect(await axeViolations(page)).toEqual([]);
      // The keyboard reaches the row's link and opens it.
      await table.getByRole("link", { name: "Parking places in Espoo" }).focus();
      await page.keyboard.press("Enter");
      await expect(page).toHaveURL(/\/projects\/espoo\/endpoints\/parking/);
    });
  });
}

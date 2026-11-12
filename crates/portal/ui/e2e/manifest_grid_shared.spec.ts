import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-6750: the sidebar's manifest lists (ManifestGrid) draw their table through the SDK DataGrid.
// Endpoints, Context Spaces, Pipelines and Subscriptions walked in the built bundle with the API
// answered from the browser; each state is shot at 1440 and 400 px into test-results/T-6750.

const IDENTITY = { subject: "b7c1e0f4", username: "jana", name: "Jana Kováčová", email: "jana@hel.fi", roles: ["portal-admin"] };
const PROJECTS = { apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] };
const manifest = (kind: string, name: string, spec: Record<string, unknown>, phase = "Live") => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: "helsinki" },
  spec,
  status: { phase },
});
const ENDPOINTS = [
  manifest("Endpoint", "air-quality", { contextSpaceRef: "air", audience: "public", slug: "airqualityhelsinkiaaaaaaaaaa" }),
  manifest("Endpoint", "bus-stops", { contextSpaceRef: "transport", audience: "organization", slug: "busstopshelsinkiaaaaaaaaaaa" }),
  manifest("Endpoint", "cycling-counters", { contextSpaceRef: "transport", audience: "project-list", slug: "cyclinghelsinkiaaaaaaaaaaaa" }, "Pending"),
];
const SPACES = [
  manifest("ContextSpace", "air", { title: { en: "Air quality" } }),
  manifest("ContextSpace", "transport", { title: { en: "Transport" } }),
  manifest("ContextSpace", "events", { title: { en: "City events" } }, "Pending"),
];
const PIPELINES = [
  manifest("Pipeline", "air-sensors", { source: { kind: "http" }, target: { contextSpaceRef: "air" }, schedule: "PT5M" }),
  manifest("Pipeline", "gtfs-stops", { source: { kind: "http" }, target: { contextSpaceRef: "transport" }, schedule: "PT1H" }, "Failed"),
];
const SUBSCRIPTIONS = [
  manifest("Subscription", "pm10-alert", { contextSpaceRef: "air", entities: [{ type: "AirQualityObserved" }], notification: { endpoint: { uri: "https://alerts.hel.fi/hook" } } }),
  manifest("Subscription", "stop-changes", { contextSpaceRef: "transport", entities: [{ type: "BusStop" }], notification: { endpoint: { uri: "https://hsl.example.org/hook" } } }),
];

type Mode = "filled" | "empty" | "error";

async function stub(page: Page, mode: Mode): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const list = (items: unknown[]) =>
      mode === "error"
        ? route.fulfill({ status: 503, contentType: "application/problem+json", body: JSON.stringify({ type: "about:blank", title: "Unavailable", status: 503, detail: "The configuration store did not answer. Try again in a minute." }) })
        : json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: mode === "empty" ? [] : items });
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname === "/api/v1/projects") return json(PROJECTS);
    if (url.pathname.endsWith("/helsinki/endpoints")) return list(ENDPOINTS);
    if (url.pathname.endsWith("/helsinki/spaces")) return list(SPACES);
    if (url.pathname.endsWith("/helsinki/pipelines")) return list(PIPELINES);
    if (url.pathname.endsWith("/helsinki/subscriptions")) return list(SUBSCRIPTIONS);
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

const PAGES = [
  { slug: "endpoints", path: "/projects/helsinki/endpoints", first: /air-quality/ },
  { slug: "spaces", path: "/projects/helsinki/spaces", first: /Air quality|^air$/ },
  { slug: "pipelines", path: "/projects/helsinki/pipelines", first: /air-sensors/ },
  { slug: "subscriptions", path: "/projects/helsinki/subscriptions", first: /pm10-alert/ },
];

for (const width of [1440, 400]) {
  for (const target of PAGES) {
    test(`${target.slug} on the shared grid at ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
      const shot = (state: string) => page.screenshot({ path: `test-results/T-6750/${target.slug}-${state}-${width}.png`, fullPage: true });

      await stub(page, "filled");
      await page.goto(`${target.path}?lang=en`);
      await expect(page.getByRole("link", { name: target.first }).first()).toBeVisible();
      const grid = page.locator(".jc-data-grid");
      await expect(grid).toBeVisible();
      await expect(page.locator("[data-manifest-grid] table[data-records]")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot("filled");

      if (width === 1440) {
        // The header button sorts; the column menu sorts and filters one column.
        await grid.locator("thead .jc-grid-sort-btn").first().click();
        await expect(grid.locator("thead th[aria-sort]")).toHaveCount(1);
        await grid.locator("thead .jc-grid-meta-btn").nth(1).click();
        await expect(page.getByRole("menu")).toBeVisible();
        await shot("menu");
        await page.keyboard.press("Escape");
      }
      await page.getByRole("button", { name: "Arrange columns" }).click();
      await expect(page.getByRole("dialog", { name: "Arrange columns" })).toBeVisible();
      await shot("arrange");
      await page.getByRole("button", { name: "Done" }).click();

      // The row's menu opens Details, the cell panel the grid kept.
      await page.getByRole("row").nth(1).getByRole("button", { name: /More actions/ }).click();
      await page.getByRole("menuitem", { name: "Details" }).click();
      await expect(page.getByRole("dialog")).toBeVisible();
      await shot("panel");
      await page.keyboard.press("Escape");

      await page.getByRole("searchbox").first().fill("zzz-nothing");
      await shot("nomatch");

      await page.unrouteAll({ behavior: "ignoreErrors" });
      await stub(page, "empty");
      await page.goto(`${target.path}?lang=en`);
      await expect(page.locator("[data-manifest-grid]")).toBeVisible();
      await shot("empty");

      await page.unrouteAll({ behavior: "ignoreErrors" });
      await stub(page, "error");
      await page.goto(`${target.path}?lang=en`);
      await expect(page.getByText(/configuration store did not answer/)).toBeVisible();
      await shot("error");
    });
  }
}

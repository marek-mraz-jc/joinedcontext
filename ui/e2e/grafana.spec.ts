import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { dirname, join } from "node:path";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { pageFindings } from "../tests/pageChecks";
import { axeViolations } from "./axe";

const NAMESPACES = Object.keys(
  JSON.parse(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../src/locales/en.json"), "utf8")) as object,
);

// The whole journey on the built bundle with the API answered from the browser: a person picks an
// Endpoint, a type, an attribute and a kind, sees their rows, saves, and opens the result.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@hel.fi",
  roles: ["portal-approver"],
};
const OPTIONS = {
  endpoints: [
    { name: "helsinki-air-quality", title: "Helsinki air quality", slug: "jtjnhkaau64zmzz5csmcwem52icvufy2", public: true, usable: true },
    { name: "helsinki-bikes-ops", title: "City bike operations", slug: "ydp6nddchg4sewmylsiclg4377mlldrj", public: false, usable: false },
  ],
  types: ["AirQualityObserved"],
  attributes: [
    { name: "pm25", kinds: ["timeseries", "stat", "table"] },
    { name: "location", kinds: ["table", "map"] },
  ],
};
const SAVED = {
  id: "ab12",
  title: "PM2.5 by station",
  endpoint: "helsinki-air-quality",
  endpointTitle: "Helsinki air quality",
  type: "AirQualityObserved",
  attribute: "pm25",
  kind: "timeseries",
  createdAt: "2026-11-02T22:00:00Z",
  openUrl: "https://dev.joinedcontext.com/grafana/d/pv-ab12?orgId=2",
};
const STATIONS = ["Kallio 2", "Mäkelänkatu", "Vartiokylä Huivipolku"].map((name, index) => ({
  id: `urn:ngsi-ld:AirQualityObserved:station-${index}`,
  type: "AirQualityObserved",
  name,
  pm25: 4.2 + index,
}));

interface World {
  page?: object;
  save?: { status: number; body: unknown };
}

async function stubApi(page: Page, world: World): Promise<void> {
  const items: object[] = [];
  await page.route(/\/api\/endpoint\//, async (route) => {
    await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(STATIONS) });
  });
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (url.pathname.endsWith("/grafana/options")) return json(OPTIONS);
    if (url.pathname.endsWith("/grafana") && route.request().method() === "POST") {
      const save = world.save ?? { status: 201, body: SAVED };
      if (save.status === 201) items.push(SAVED);
      return json(save.body, save.status);
    }
    if (url.pathname.endsWith("/grafana")) {
      return json(world.page ?? { ready: true, mayCreate: true, items });
    }
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

const SHOTS = process.env.GRAFANA_SHOTS;
async function shot(page: Page, name: string, width: number): Promise<void> {
  if (!SHOTS) return;
  await page.waitForTimeout(300);
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  await page.setViewportSize({ width, height: Math.min(Math.max(height, 400), 2400) });
  await page.screenshot({ path: `${SHOTS}/${name}-${width}.png` });
}

async function fill(page: Page): Promise<void> {
  await page.getByLabel("Endpoint", { exact: true }).selectOption("helsinki-air-quality");
  await page.getByLabel("Type", { exact: true }).selectOption("AirQualityObserved");
  await page.getByLabel("Attribute", { exact: true }).selectOption("pm25");
  await page.getByLabel("Name", { exact: true }).fill("PM2.5 by station");
}

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("a person saves a visualization and gets its link", async ({ page }) => {
      await stubApi(page, {});
      await page.goto("/projects/helsinki/grafana");
      await expect(page.getByRole("heading", { level: 1, name: "Grafana" })).toBeVisible();
      await expect(page.getByText("Nothing saved yet")).toBeVisible();
      await expect(page.getByText(/Not in Grafana yet: City bike operations/)).toBeVisible();
      await shot(page, "empty-list", width);
      await fill(page);
      await expect(page.getByText("Vartiokylä Huivipolku")).toBeVisible();
      await shot(page, "filled", width);
      await page.getByRole("button", { name: "Save to Grafana" }).click();
      await expect(page.getByRole("link", { name: "Open it" })).toHaveAttribute("href", SAVED.openUrl);
      await expect(page.getByRole("link", { name: "Open PM2.5 by station in Grafana" })).toBeVisible();
      await expect(page.getByText("Helsinki air quality: AirQualityObserved, pm25")).toBeVisible();
      await shot(page, "saved", width);
      expect(await page.evaluate(pageFindings, { namespaces: NAMESPACES, layout: true })).toEqual([]);
    });

    test("a refusal names the field and keeps what was typed", async ({ page }) => {
      await stubApi(page, {
        save: { status: 422, body: { status: 422, title: "Unprocessable", detail: "Grafana cannot read the private Endpoint Helsinki air quality yet; ask your operator to add it to this project's Grafana, or choose another" } },
      });
      await page.goto("/projects/helsinki/grafana");
      await fill(page);
      await page.getByRole("button", { name: "Save to Grafana" }).click();
      await expect(page.getByRole("alert")).toContainText("operator");
      await expect(page.getByLabel("Name", { exact: true })).toHaveValue("PM2.5 by station");
      await shot(page, "error", width);
    });

    test("a reader sees what is saved and no form", async ({ page }) => {
      await stubApi(page, { page: { ready: true, mayCreate: false, items: [SAVED] } });
      await page.goto("/projects/helsinki/grafana");
      await expect(page.getByRole("link", { name: "Open PM2.5 by station in Grafana" })).toBeVisible();
      await expect(page.getByLabel("Endpoint", { exact: true })).toHaveCount(0);
      await shot(page, "refusal-reader", width);
    });

    test("a project without Grafana says who can add it", async ({ page }) => {
      await stubApi(page, {
        page: { ready: false, mayCreate: true, items: [], reason: "Grafana has no organization for helsinki yet; ask your operator to add this project to Grafana" },
      });
      await page.goto("/projects/helsinki/grafana");
      await expect(page.getByText(/ask your operator/)).toBeVisible();
      await shot(page, "not-ready", width);
    });
  });
}

test("the page has no axe violations", async ({ page }) => {
  await stubApi(page, {});
  await page.goto("/projects/helsinki/grafana");
  await expect(page.getByLabel("Endpoint", { exact: true })).toBeVisible();
  expect(await axeViolations(page)).toEqual([]);
});

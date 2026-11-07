import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// Phone tables and the assistant's button (T-4992): a table cut at the right edge shows a cue, a
// data source's address no longer breaks every nine characters, the closed assistant sits in the
// header on a phone instead of over the rows' "..." menus, a space's types are a picker above
// the grid on a phone, and the grid's Name column keeps its width at 1440. `vite preview` has no
// Portal API behind it, so the API and the space surface are answered in the browser. With
// SHOTS_DIR set, the 1440 px and 400 px screenshots are written there for the task's evidence.

const PROJECT = "helsinki";
const SPACE = "helsinki";
const TYPES = ["Event", "BikeHireDockingStation", "Vehicle", "Article", "WeatherObserved", "Alert", "AirQualityObserved", "PointOfInterest"];

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const manifest = (kind: string, name: string, spec: Record<string, unknown>, title?: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: PROJECT, ...(title ? { title } : {}) },
  spec,
  status: { phase: "Live" },
});

const LINKML = [
  "id: https://hel.fi/models/city",
  "name: helsinki-city",
  "classes:",
  ...TYPES.flatMap((type) => [`  ${type}:`, "    slots: [id, name, description]"]),
  "slots:",
  "  id: {}",
  "  name: { range: string, title: Name }",
  "  description: { range: string, title: Description }",
  "",
].join("\n");

const EVENTS = [
  ["Liiketoimintasuunnitelman laskelmat - video", "Hyvin tehdyt laskelmat ovat avain menestyvään liiketoimintaan. Tältä videolta opit, kuinka tehdä yrityksen rahoitus- ja kannattavuuslaskelmat."],
  ["Finnish Language Café at Iso Omena Library", "Welcome to learn Finnish at Iso Omena Library in a relaxed conversation club."],
  ["Toastmasters - become a better speaker", "A group of enthusiasts get together to practise their speaking and presentation skills in English."],
  ["Board game club", "A game club aimed at adults."],
].map(([name, description], i) => ({
  id: `urn:ngsi-ld:Event:hel.fi:${i + 1}`,
  type: "Event",
  name: { type: "Property", value: name },
  description: { type: "Property", value: description },
}));

const SOURCES = [
  manifest("DataSource", "digitraffic-traffic-messages", {
    type: "http",
    input: { url: "https://tie.digitraffic.fi/api/traffic-message/v1/messages?inactiveHours=0&includeAreaGeometry=false&situationType=TRAFFIC_ANNOUNCEMENT" },
  }, "Road works and traffic announcements (Fintraffic Digitraffic)"),
  manifest("DataSource", "digitraffic-road-weather", {
    type: "http",
    input: { url: "https://tie.digitraffic.fi/api/weather/v1/stations/data" },
  }, "Road weather stations (Fintraffic Digitraffic)"),
];

async function stubApi(page: Page): Promise<void> {
  await page.route("**/{api,cs}/**", async (route) => {
    const url = new URL(route.request().url());
    const path = decodeURIComponent(url.pathname);
    const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
      route.fulfill({ status, contentType: "application/json", headers, body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "demo.steward", name: "Demo Steward", roles: ["portal-editor"] });
    }
    if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
    if (path.endsWith("/datamodels/helsinki-city/source")) {
      return route.fulfill({ status: 200, contentType: "text/yaml", body: LINKML });
    }
    if (path.endsWith("/entities")) {
      const rows = url.searchParams.get("type") === "Event" ? EVENTS : [];
      return json(rows, 200, { "NGSILD-Results-Count": String(rows.length) });
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path.endsWith(`/spaces/${SPACE}/usage`)) return json({ entities: 4, broker: "main", observedAt: "2026-10-21T10:00:00Z" });
    if (path.endsWith(`/spaces/${SPACE}/views`)) return json({ items: [] });
    if (path.endsWith(`/spaces/${SPACE}`)) return json(manifest("ContextSpace", SPACE, { dataModelRef: "helsinki-city" }, "Helsinki city context"));
    if (path.endsWith("/datamodels")) {
      return json(list([manifest("DataModel", "helsinki-city", { contextSpaceRef: SPACE, version: "1.0.0", classes: TYPES, linkml: LINKML })]));
    }
    if (path.endsWith("/datasources")) return json(list(SOURCES));
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  if (dir) await page.screenshot({ path: `${dir}/${name}.png`, fullPage: true });
}

test("a space at 1440: the types beside the grid and a Name column at least 180 px wide", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await stubApi(page);
  await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en`);

  await expect(page.getByRole("button", { name: `Open: ${EVENTS[0].name.value}` })).toBeVisible();
  // The types are the Type filter of the grid's Filters row since the grid rework (no list of types beside it).
  await expect(page.getByRole("navigation", { name: "Entity types" })).toHaveCount(0);
  await expect(page.getByRole("combobox", { name: "Type", exact: true })).toBeVisible();
  const name = await page.getByRole("columnheader", { name: /Sort all rows by Name/ }).boundingBox();
  expect(name?.width ?? 0).toBeGreaterThanOrEqual(180);
  await expect(page.locator("[data-assistant-bubble]")).toHaveCSS("position", "fixed");
  expect(await axeViolations(page)).toEqual([]);
  await shot(page, "space-1440");
});

test("a space at 400: the types are a picker above the grid, and the assistant sits in the header", async ({ page }) => {
  await page.setViewportSize({ width: 400, height: 860 });
  await stubApi(page);
  await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?lang=en`);

  // The phone has no filter row (T-6463): the Type picker stands above the grid.
  const picker = page.getByRole("combobox", { name: "Type", exact: true });
  await expect(picker).toBeVisible();
  await expect(page.getByRole("navigation", { name: "Entity types" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: `Open: ${EVENTS[0].name.value}` })).toBeVisible();
  // The first row is on the first screen, not two screens down under the list of types.
  const first = await page.getByRole("button", { name: `Open: ${EVENTS[0].name.value}` }).boundingBox();
  expect(first?.y ?? Infinity).toBeLessThan(1300);

  const bubble = page.locator("[data-assistant-bubble]");
  await expect(bubble).toHaveCount(1);
  await expect(page.locator("header").locator("[data-assistant-bubble]")).toBeVisible();
  await expect(bubble).not.toHaveCSS("position", "fixed");
  expect(await page.evaluate(() => document.documentElement.scrollWidth), "nothing scrolls sideways").toBeLessThanOrEqual(400);
  expect(await axeViolations(page)).toEqual([]);
  await shot(page, "space-400");

  const read = page.waitForRequest((request) => new URL(request.url()).searchParams.get("type") === "Vehicle");
  await picker.selectOption("Vehicle");
  await read;
  await expect(page.getByRole("button", { name: `Open: ${EVENTS[0].name.value}` })).toHaveCount(0);
  await expect(picker).toHaveValue("Vehicle");
  await shot(page, "space-400-picked");

  await bubble.click();
  await expect(page.getByLabel("Ask the assistant")).toBeVisible();
  await expect(page.locator("header").locator("[data-assistant-bubble]")).toHaveCount(0);
});

test("data sources at 400: the address wraps in a readable column and the frame shows a cue until scrolled", async ({ page }) => {
  await page.setViewportSize({ width: 400, height: 860 });
  await stubApi(page);
  await page.goto(`/projects/${PROJECT}/datasources?lang=en`);

  const cell = page.getByRole("cell", { name: /tie\.digitraffic\.fi\/api\/traffic-message/ });
  await expect(cell).toBeVisible();
  const box = await cell.boundingBox();
  // min-w-48 is 48 spacing units: 9.6rem since the denser scale of T-6093, about 20 characters.
  expect(box?.width ?? 0, "the address column keeps min-w-48").toBeGreaterThanOrEqual(150);
  expect(box?.height ?? Infinity, "the address takes a few lines, not 25").toBeLessThan(200);

  const cue = page.getByTestId("table-more-columns");
  await expect(cue).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth), "the page itself never scrolls sideways").toBeLessThanOrEqual(400);
  await shot(page, "datasources-400");

  const frame = page.getByRole("group", { name: "Data sources" });
  await frame.evaluate((element) => element.scrollTo({ left: element.scrollWidth }));
  await expect(cue).toHaveCount(0);
  await shot(page, "datasources-400-scrolled");
  expect(await axeViolations(page)).toEqual([]);
});

test("data sources at 1440: everything fits, so no cue", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await stubApi(page);
  await page.goto(`/projects/${PROJECT}/datasources?lang=en`);
  await expect(page.getByRole("cell", { name: /tie\.digitraffic\.fi\/api\/weather/ })).toBeVisible();
  await expect(page.getByTestId("table-more-columns")).toHaveCount(0);
  await shot(page, "datasources-1440");
});

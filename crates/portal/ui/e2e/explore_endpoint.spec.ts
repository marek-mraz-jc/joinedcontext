import { mkdirSync } from "node:fs";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-5604: Explore reads a type through the space's own Endpoint before an App's, writes the
// Endpoint it reads into the address and names it under the filters; a link naming an Endpoint
// the person is not shown says so and reads through no other. `vite preview` has no API, so the
// browser answers it.

const TYPE = "AirQualityObserved";
const OWN = { name: "helsinki-air-quality", slug: "jtjnhkaau64zmzz5csmcwem52icvufy2" };
const APP = { name: "app-air-quality", slug: "3jlwl5dg2oidblwqhc6qdi3zxl" };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const LINKML = [
  "id: https://hel.fi/models/air",
  "name: helsinki-air",
  "classes:",
  `  ${TYPE}:`,
  "    slots: [id, name, pm10]",
  "slots:",
  "  id: {}",
  "  name: { range: string }",
  "  pm10: { range: float }",
  "",
].join("\n");

const SPARSE = ["description", "dataProvider", "source", "address", "areaServed", "stationCode", "stationName", "reliability"];

const endpoint = (e: { name: string; slug: string }, spec: Record<string, unknown>, annotations?: Record<string, string>) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: e.name, namespace: "helsinki", title: { en: e === OWN ? "Helsinki air quality" : "Air quality" }, ...(annotations ? { annotations } : {}) },
  spec: { contextSpaceRef: "helsinki", slug: e.slug, enabledRepresentations: ["ngsi-ld"], ...spec },
  status: { phase: "Live" },
});

interface Options {
  /** The space's own Endpoint serves CSV files (T-6107). */
  csv?: boolean;
  /** Its Policy refuses this person every read. */
  refuse?: boolean;
  /** The project holds no space at all. */
  noSpaces?: boolean;
  /** Only the first row carries the SPARSE attributes: the others show empty cells, as Helsinki's Events do. */
  sparse?: boolean;
}

async function stub(page: Page, withApp: boolean, options: Options = {}): Promise<string[]> {
  const reads: string[] = [];
  await page.route("**/{api,cs}/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, headers: Record<string, string> = {}) =>
      route.fulfill({ status: 200, contentType: "application/json", headers, body: JSON.stringify(body) });
    const path = url.pathname;
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "demo.viewer", name: "Demo Viewer", roles: [], groups: ["helsinki"] });
    // A viewer: reads, never writes.
    if (path.endsWith("/access/check")) return json({ decision: /"(query|retrieve)/.test(route.request().postData() ?? "") });
    if (path.endsWith("/access/v1/evaluations")) return json({ evaluations: (JSON.parse(route.request().postData() ?? "{}").evaluations as { action: { name: string } }[]).map((item) => ({ decision: /^(query|retrieve)/.test(item.action.name) })) });
    if (path.endsWith("/access")) return json({ permissions: [{ resource: { type: TYPE }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" }] });
    if (path.endsWith("/entities")) {
      reads.push(path);
      if (options.refuse) {
        return route.fulfill({ status: 403, contentType: "application/problem+json", body: JSON.stringify({ type: "https://uri.etsi.org/ngsi-ld/errors/Forbidden", title: "Forbidden", status: 403, detail: "the policy of this endpoint does not let you read AirQualityObserved" }) });
      }
      const property = (value: unknown) => ({ type: "Property", value });
      const rows = options.sparse
        ? [1, 2, 3].map((n) => ({ id: `urn:ngsi-ld:${TYPE}:hel.fi:helsinki:fmi-${n}`, type: TYPE, name: property(`Station ${n}`), ...(n === 1 ? Object.fromEntries(SPARSE.map((slot) => [slot, property(`${slot} of station 1`)])) : {}) }))
        : [1, 2, 3].map((n) => ({ id: `urn:ngsi-ld:${TYPE}:hel.fi:helsinki:fmi-${n}`, type: TYPE, name: `Station ${n}`, pm10: 10 + n }));
      return json(rows, { "NGSILD-Results-Count": "3" });
    }
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/spaces")) {
      if (options.noSpaces) return json(list([]));
      const second = { apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "helsinki-parks", namespace: "helsinki", title: { en: "Helsinki parks" } }, spec: { dataModelRef: "helsinki-air" }, status: { phase: "Live" } };
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "helsinki", namespace: "helsinki", title: { en: "Helsinki city context" } }, spec: { dataModelRef: "helsinki-air" }, status: { phase: "Live" } }, second]));
    }
    if (path.endsWith("/endpoints")) {
      const own = endpoint(OWN, { audience: "public", policyRef: "urn:ngsi-ld:Policy:hel.fi:helsinki:public-air-quality", ...(options.csv ? { enabledRepresentations: ["ngsi-ld", "csv"] } : {}) });
      const app = endpoint(APP, { audience: "project-list", callerRole: true }, { "joinedcontext.com/generated-by": "portal/app-reconciler" });
      return json(list(withApp ? [app, own] : [own]));
    }
    if (path.endsWith("/datamodels")) {
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "DataModel", metadata: { name: "helsinki-air", namespace: "helsinki" }, spec: { version: "1.0.0", classes: [TYPE], linkml: LINKML } }]));
    }
    return json(list([]));
  });
  return reads;
}

const SHOTS = process.env.T5604_SHOTS ?? "test-results/explore-endpoint";
mkdirSync(SHOTS, { recursive: true });

for (const width of [1440, 400]) {
  test(`reads through the space's own Endpoint and writes it into the address at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const reads = await stub(page, true);
    await page.goto(`/projects/helsinki/explore?space=helsinki&type=${TYPE}&lang=en`);
    await expect(page.locator("tbody tr").first()).toBeVisible();
    await expect(page).toHaveURL(new RegExp(`endpoint=${OWN.name}`));
    await expect(page.locator("#explore-endpoint")).toHaveValue(OWN.name);
    await expect(page.locator("main").getByText("Read through Helsinki air quality")).toBeAttached();
    expect(reads.every((path) => path.includes(OWN.slug))).toBe(true);
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: `${SHOTS}/read-through-${width}.png`, fullPage: true });
  });

  test(`a link naming an Endpoint the person is not shown says so at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const reads = await stub(page, false);
    await page.goto(`/projects/helsinki/explore?space=helsinki&endpoint=${APP.name}&type=${TYPE}&lang=en`);
    await expect(page.getByText(`The link names the Endpoint ${APP.name}, which this space does not show you`)).toBeVisible();
    await expect(page.locator("#explore-endpoint")).toHaveValue("");
    await expect(page.locator("tbody tr")).toHaveCount(0);
    await expect(page.getByText("Choose an entity type to load entities.")).toHaveCount(0);
    expect(reads).toEqual([]);
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: `${SHOTS}/unshown-${width}.png`, fullPage: true });

    // Picking one the person can read clears the refusal and reads through it.
    await page.locator("#explore-endpoint").selectOption(OWN.name);
    await expect(page.locator("tbody tr").first()).toBeVisible();
    await expect(page).toHaveURL(new RegExp(`endpoint=${OWN.name}`));
    await expect(page.getByText(`The link names the Endpoint ${APP.name}`)).toHaveCount(0);
  });
}

// T-6107: the page opens on data with nothing in the address, the pickers on the title line, and
// the space page's Export menu in the grid's toolbar.
const AFTER = process.env.T6107_SHOTS ?? "test-results/explore-t6107";
mkdirSync(AFTER, { recursive: true });

for (const width of [1440, 400]) {
  test(`opens on data and exports through the Endpoint's formats at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const reads = await stub(page, false, { csv: true });
    await page.goto("/projects/helsinki/explore?lang=en");
    await expect(page.locator("tbody tr").first()).toBeVisible();
    await expect(page.locator("#explore-space")).toHaveValue("helsinki");
    await expect(page.locator("#explore-endpoint")).toHaveValue(OWN.name);
    expect(reads.every((path) => path.includes(OWN.slug))).toBe(true);
    // The pickers sit in the header, beside the description at 1440, above the filters, with no
    // hint sentence.
    const description = await page.getByText("Rows of a context space, read through one of its Endpoints.").boundingBox();
    const picker = await page.locator("#explore-space").boundingBox();
    if (width === 1440) expect(Math.abs(picker!.y + picker!.height - (description!.y + description!.height))).toBeLessThan(24);
    // The filters fold under one line: rows in the first screen at 1440, the grid's header at 400.
    await expect(page.getByLabel("Entity type")).toBeHidden();
    const first = await page.locator(width === 1440 ? "tbody tr" : "thead").first().boundingBox();
    expect(first!.y + first!.height).toBeLessThan(900);
    await expect(page.getByText("whose Policy decides what you see")).toHaveCount(0);
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: `${AFTER}/loaded-${width}.png`, fullPage: true });

    const exportButton = page.locator("summary", { hasText: "Export" });
    await exportButton.click();
    const csv = page.getByRole("link", { name: "CSV", exact: true });
    await expect(csv).toBeVisible();
    // The menu opens under its button, at 400 too (no bottom sheet over the toolbar).
    const button = await exportButton.boundingBox();
    const menu = await csv.boundingBox();
    expect(menu!.y).toBeGreaterThan(button!.y + button!.height);
    expect(menu!.y).toBeLessThan(button!.y + button!.height + 120);
    const href = new URL((await csv.getAttribute("href"))!, "http://x");
    expect(href.pathname).toBe(`/api/endpoint/${OWN.slug}/file.csv`);
    expect(href.searchParams.get("type")).toBe(TYPE);
    await page.screenshot({ path: `${AFTER}/export-open-${width}.png`, fullPage: true });

    // A filter typed in the grid travels as the export's q.
    await page.goto(`/projects/helsinki/explore?space=helsinki&type=${TYPE}&q=${encodeURIComponent("pm10>11")}&lang=en`);
    await expect(page.locator("tbody tr").first()).toBeVisible();
    await page.locator("summary", { hasText: "Export" }).click();
    const narrowed = new URL((await page.getByRole("link", { name: "CSV", exact: true }).getAttribute("href"))!, "http://x");
    expect(narrowed.searchParams.get("q")).toBe("pm10>11");

    // Another space without an Endpoint says so instead of reading through another one.
    await page.locator("#explore-space").selectOption("helsinki-parks");
    await expect(page.getByText("This space has no endpoint to read through.")).toBeVisible();
    await expect(page.locator("tbody tr")).toHaveCount(0);
  });

  test(`shows an Endpoint's refusal in the grid at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await stub(page, false, { refuse: true });
    await page.goto("/projects/helsinki/explore?lang=en");
    await expect(page.getByText("does not let you read", { exact: false }).first()).toBeVisible();
    await expect(page.getByText("You have no access to this dataset")).toBeVisible();
    await expect(page.getByText("try again", { exact: false })).toHaveCount(0);
    await expect(page.locator("summary", { hasText: "Your access" })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Use this data" })).toHaveCount(0);
    await page.screenshot({ path: `${AFTER}/refused-${width}.png`, fullPage: true });
  });

  test(`an Endpoint that serves no file offers no download at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await stub(page, false);
    await page.goto("/projects/helsinki/explore?lang=en");
    await expect(page.locator("tbody tr").first()).toBeVisible();
    const exportButton = page.getByRole("button", { name: /^Export/ });
    await expect(exportButton).toHaveAttribute("aria-disabled", "true");
    await exportButton.focus();
    await expect(page.getByRole("link", { name: "CSV", exact: true })).toHaveCount(0);
    await page.screenshot({ path: `${AFTER}/export-none-${width}.png`, fullPage: true });
  });

  test(`a project without a space says where to make one at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await stub(page, false, { noSpaces: true });
    await page.goto("/projects/helsinki/explore?lang=en");
    await expect(page.getByText("This project has no context space yet")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: `${AFTER}/empty-project-${width}.png`, fullPage: true });
  });

  // T-6325: every empty cell holds a screen-reader "not set" placed absolutely; outside the grid's
  // scroll box they widened the whole page (1603 px sideways on Helsinki's Events at 400 px).
  test(`rows with empty cells keep the page within the screen at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await stub(page, false, { sparse: true });
    await page.goto(`/projects/helsinki/explore?space=helsinki&type=${TYPE}&lang=en`);
    await expect(page.locator("tbody tr").first()).toBeVisible();
    await expect(page.locator("thead th", { hasText: "reliability" })).toBeAttached();
    const sideways = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(sideways).toBeLessThanOrEqual(1);
    await page.screenshot({ path: `${AFTER}/sparse-${width}.png`, fullPage: true });
  });
}

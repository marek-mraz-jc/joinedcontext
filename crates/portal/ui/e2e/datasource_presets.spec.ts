import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { axeViolations } from "./axe";
import { manifestReadsAsRest } from "./manifestReads";

// The DataSource doors run through the OData service (T-7117); the stubs answer their REST doors.
test.beforeEach(({ page }) => manifestReadsAsRest(page));

// T-7076: an http source with an open data API preset. The steward picks the API, Find in the
// service lists the feature types the WFS offers (read by the Portal on the project's runner),
// Use writes the pick into the preset, and the connection test of a saved paged source says how
// many pages it read. `vite preview` has no Portal API behind it, so the browser answers it.

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const SHOTS = process.env.SHOTS_DIR;
const WFS = "https://kartta.hel.fi/ws/geoserver/avoindata/wfs";

type Discovery = "listed" | "skipped" | "refused";

async function stubApi(page: Page, discovery: Discovery = "listed"): Promise<unknown[]> {
  const asked: unknown[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "aino.virtanen", name: "Aino Virtanen", roles: ["portal-approver"] });
    }
    if (url.pathname === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: PROJECT }] });
    if (request.method() === "POST" && url.pathname.endsWith(`/projects/${PROJECT}/datasource-discovery`)) {
      asked.push(request.postDataJSON());
      if (discovery === "refused") {
        return json({ type: "about:blank", title: "Forbidden", status: 403, detail: "No grant proposes a DataSource in project helsinki." }, 403);
      }
      return json({
        field: "typeNames",
        url: `${WFS}?service=WFS&version=2.0.0&request=GetCapabilities`,
        choices:
          discovery === "listed"
            ? [
                { value: "avoindata:Helsinki_osoiteluettelo", label: "Helsinki addresses" },
                { value: "avoindata:FMI_Paikkakohtainen_vyorymakerroin_piste", label: "FMI landslide factor points" },
                { value: "avoindata:Puut", label: "Street trees" },
              ]
            : [],
        truncated: discovery === "listed",
        ...(discovery === "skipped" ? { skipped: "the feed answered 503 Service Unavailable" } : {}),
      });
    }
    const lakes = {
        apiVersion: API,
        kind: "DataSource",
        metadata: { name: "lakes", namespace: PROJECT },
        spec: { type: "http", http: { url: "https://demo.pygeoapi.io/master/collections/lakes/items", preset: { kind: "ogcFeatures", pageSize: 10 } } },
        status: { phase: "Live" },
    };
    if (request.method() === "GET" && url.pathname.endsWith(`/projects/${PROJECT}/datasources`)) {
      return json({ apiVersion: API, kind: "List", items: [lakes] });
    }
    if (request.method() === "GET" && url.pathname.endsWith(`/projects/${PROJECT}/datasources/lakes`)) return json(lakes);
    if (request.method() === "POST" && url.pathname.endsWith(`/projects/${PROJECT}/datasources/lakes/test`)) {
      return json({ records: 25, bytes: 9120, pages: 3, sample: { id: 1, name: "Lake Baikal" } });
    }
    return json({ apiVersion: API, kind: "List", items: [] });
  });
  return asked;
}

async function shot(page: Page, state: string): Promise<void> {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: `${SHOTS}/${state}-${width}.png`, fullPage: true });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

async function openWfsForm(page: Page): Promise<void> {
  await page.goto(`/projects/${PROJECT}/datasources/new?lang=en`);
  const form = page.getByTestId("form-page");
  await expect(form.getByText("Choose an API under Open data API preset")).toBeVisible();
  await shot(page, "presets-empty");
  await form.getByRole("textbox", { name: "Name", exact: true }).fill("street-trees");
  await form.getByRole("textbox", { name: /^URL/ }).fill(WFS);
  await form.getByRole("combobox", { name: "API", exact: true }).selectOption({ label: "WFS 2.0" });
}

test.describe("an http source with an open data API preset", () => {
  test("a steward finds the feature types of a WFS and writes one into the preset", async ({ page }) => {
    const asked = await stubApi(page);
    await openWfsForm(page);
    const form = page.getByTestId("form-page");
    await expect(form.getByText(/WFS only: the feature type read, for example avoindata:Helsinki_osoiteluettelo/)).toBeVisible();
    // Only what a WFS preset reads is shown: no CKAN field, and no Paging, which the preset sets.
    await expect(form.getByRole("textbox", { name: /^Organization/ })).toBeHidden();
    await expect(form.getByRole("combobox", { name: /^Next page by/ })).toBeHidden();
    await page.getByRole("button", { name: "Find", exact: true }).click();
    const choice = page.getByRole("combobox", { name: /Feature type · Found 3 entries/ });
    await expect(choice).toBeVisible();
    expect(asked).toEqual([{ kind: "wfs", url: WFS }]);
    await expect(page.getByText(/lists more than the Portal reads/)).toBeVisible();
    await choice.selectOption("avoindata:Puut");
    await shot(page, "presets-listed");
    await page.getByRole("button", { name: "Use", exact: true }).click();
    await expect(form.getByRole("textbox", { name: /^Feature type/ })).toHaveValue("avoindata:Puut");
    await expect(page.getByText("Written into the form: avoindata:Puut.")).toBeVisible();
    await shot(page, "presets-used");
    expect(await axeViolations(page)).toEqual([]);
  });

  test("a service that did not answer says so and what to check", async ({ page }) => {
    await stubApi(page, "skipped");
    await openWfsForm(page);
    await page.getByRole("button", { name: "Find", exact: true }).click();
    await expect(page.getByText(/Nothing was listed: the feed answered 503 Service Unavailable\. Check the URL and the API kind/)).toBeVisible();
    await shot(page, "presets-error");
  });

  test("a person who may not propose a source reads the refusal", async ({ page }) => {
    await stubApi(page, "refused");
    await openWfsForm(page);
    await page.getByRole("button", { name: "Find", exact: true }).click();
    await expect(page.getByText(/No grant proposes a DataSource in project helsinki\. Ask a project administrator for a role/)).toBeVisible();
    await shot(page, "presets-refused");
  });

  test("the connection test of a paged source says how many pages it read", async ({ page }) => {
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/datasources/lakes?lang=en`);
    await page.getByRole("button", { name: "Test connection" }).click();
    await expect(page.getByTestId("datasource-test-pages")).toHaveText("Read 3 pages.");
    await shot(page, "presets-test-pages");
  });
});

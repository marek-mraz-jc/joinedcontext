import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { axeViolations } from "./axe";

// T-7094 (G-22): the new data source wizard. A steward picks what the source is, gives its
// address, lets an open data API list what it offers, reads how the source pages in words and
// reaches the first rows of the check, with no YAML typed. The answers are the recorded G-08
// fixtures' numbers (ekiben-io-http tests/fixtures/presets); `vite preview` has no Portal API, so
// the browser answers it.

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const SHOTS = process.env.SHOTS_DIR;
const WFS = "https://kartta.hel.fi/ws/geoserver/avoindata/wfs";
const LAYER = "avoindata:FMI_Paikkakohtainen_vyorymakerroin_piste";

type Paging = "wfs" | "odata" | "skipped" | "refused";

async function stubApi(page: Page, paging: Paging = "wfs"): Promise<{ checked: unknown[]; guessed: unknown[] }> {
  const checked: unknown[] = [];
  const guessed: unknown[] = [];
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
      return json({
        field: "typeNames",
        url: `${WFS}?service=WFS&version=2.0.0&request=GetCapabilities`,
        choices: [
          { value: "avoindata:Helsinki_osoiteluettelo", label: "Helsinki addresses" },
          { value: LAYER, label: "FMI landslide factor points" },
        ],
        truncated: false,
      });
    }
    if (request.method() === "POST" && url.pathname.endsWith(`/projects/${PROJECT}/datasource-paging`)) {
      guessed.push(request.postDataJSON());
      if (paging === "refused") {
        return json({ type: "about:blank", title: "Forbidden", status: 403, detail: "No grant proposes a DataSource in project helsinki." }, 403);
      }
      if (paging === "skipped") return json({ more: false, skipped: "the feed did not answer within the test's three seconds" });
      if (paging === "odata") {
        return json({ paging: { mode: "nextUrl", next: "@odata.nextLink" }, records: "value", onPage: 20, more: true });
      }
      return json({ paging: { mode: "offset", total: "numberMatched" }, records: "features", onPage: 200, total: 453, pages: 3, more: false });
    }
    if (request.method() === "POST" && url.pathname.endsWith(`/projects/${PROJECT}/datasources`) && url.searchParams.get("dryRun") === "All") {
      checked.push(request.postDataJSON());
      return json({
        valid: true,
        lane: "green",
        plan: { fields: [{ path: "spec.http.url", op: "add" }] },
        probe: { records: 453, bytes: 32488, pages: 3, sample: { type: "Feature", properties: { id: 1, c: 0.42 } } },
      });
    }
    return json({ apiVersion: API, kind: "List", items: [] });
  });
  return { checked, guessed };
}

async function shot(page: Page, state: string): Promise<void> {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: `${SHOTS}/wizard-${state}-${width}.png`, fullPage: true });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

async function openWizard(page: Page): Promise<ReturnType<Page["getByTestId"]>> {
  await page.goto(`/projects/${PROJECT}/datasources/new?lang=en`);
  const wizard = page.getByTestId("datasource-templates");
  await expect(wizard).toHaveAttribute("open", "");
  return wizard;
}

test.describe("the new data source wizard", () => {
  test("an open data API: listed, picked, paged in words and checked to its first rows", async ({ page }) => {
    const { checked, guessed } = await stubApi(page);
    const wizard = await openWizard(page);
    await shot(page, "empty");
    await wizard.getByRole("button", { name: /WFS 2\.0/ }).click();
    await wizard.getByRole("textbox", { name: /Address of the service/ }).fill(WFS);
    await wizard.getByRole("button", { name: "Find", exact: true }).click();
    await wizard.getByRole("combobox", { name: /Feature type/ }).selectOption(LAYER);
    await wizard.getByRole("button", { name: "Use", exact: true }).click();
    await wizard.getByRole("button", { name: "Find how it pages" }).click();
    await expect(wizard.getByTestId("wizard-paging")).toHaveText(/Reads 3 pages of 200 \(453 records\)\./);
    expect(guessed).toHaveLength(1);
    await wizard.getByRole("textbox", { name: /^Name/ }).fill("landslide-points");
    await shot(page, "api-paged");
    await wizard.getByRole("button", { name: "Fill the form and check" }).click();
    await expect(page.getByTestId("datasource-probe")).toContainText("453 records");
    const body = checked[0] as { metadata: { name: string }; spec: { http: Record<string, unknown> } };
    expect(body.metadata.name).toBe("landslide-points");
    expect(body.spec.http).toMatchObject({ url: WFS, preset: { kind: "wfs", typeNames: LAYER } });
    await page.getByTestId("datasource-probe").scrollIntoViewIfNeeded();
    await shot(page, "api-probe");
    expect(await axeViolations(page)).toEqual([]);
  });

  test("a JSON API: its walk and records come from the first answer, keyboard only", async ({ page }) => {
    const { checked } = await stubApi(page, "odata");
    const wizard = await openWizard(page);
    const json = wizard.getByRole("button", { name: /REST API \(JSON\)/ });
    await json.focus();
    await page.keyboard.press("Enter");
    const address = wizard.getByRole("textbox", { name: /Address of the API/ });
    await address.focus();
    await page.keyboard.type("https://services.odata.org/V4/Northwind/Northwind.svc/Customers");
    await wizard.getByRole("button", { name: "Find how it pages" }).focus();
    await page.keyboard.press("Enter");
    await expect(wizard.getByTestId("wizard-paging")).toHaveText(/Reads pages of 20 until the service says there are no more\./);
    await expect(wizard.getByRole("textbox", { name: "Records are in" })).toHaveValue("value");
    await wizard.getByRole("button", { name: "Fill the form and check" }).focus();
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("datasource-probe")).toBeVisible();
    const body = checked[0] as { spec: { http: Record<string, unknown> } };
    expect(body.spec.http).toMatchObject({ records: "value", paging: { mode: "nextUrl", next: "@odata.nextLink" } });
    expect(await axeViolations(page)).toEqual([]);
  });

  test("a CSV file is split into rows and checked; no field asks for a password", async ({ page }) => {
    const { checked } = await stubApi(page);
    const wizard = await openWizard(page);
    await wizard.getByRole("button", { name: /CSV file at an address/ }).click();
    await expect(wizard.getByText("A file or a feed is read whole on each run: there are no pages.")).toBeVisible();
    await expect(wizard.getByRole("checkbox", { name: /One record per row/ })).toBeChecked();
    await expect(page.locator('input[type="password"]')).toHaveCount(0);
    await wizard.getByRole("textbox", { name: /Address of the CSV file/ }).fill("https://data.example.org/trees.csv");
    await shot(page, "file-filled");
    await wizard.getByRole("button", { name: "Fill the form and check" }).click();
    await expect(page.getByTestId("datasource-probe")).toBeVisible();
    expect((checked[0] as { spec: { http: Record<string, unknown> } }).spec.http).toMatchObject({ format: "csv" });
  });

  test("a source that did not answer, and a person who may not propose one, read why", async ({ page }) => {
    await stubApi(page, "skipped");
    let wizard = await openWizard(page);
    await wizard.getByRole("button", { name: /REST API \(JSON\)/ }).click();
    await wizard.getByRole("textbox", { name: /Address of the API/ }).fill("https://slow.example/items");
    await wizard.getByRole("button", { name: "Find how it pages" }).click();
    await expect(wizard.getByTestId("wizard-paging")).toHaveText(/Could not tell: the feed did not answer/);
    await shot(page, "error");

    await page.unrouteAll();
    await stubApi(page, "refused");
    wizard = await openWizard(page);
    await wizard.getByRole("button", { name: /REST API \(JSON\)/ }).click();
    await wizard.getByRole("textbox", { name: /Address of the API/ }).fill("https://api.example/items");
    await wizard.getByRole("button", { name: "Find how it pages" }).click();
    await expect(wizard.getByTestId("wizard-paging")).toHaveText(/No grant proposes a DataSource in project helsinki/);
    await shot(page, "refused");
  });

  test("a stream or a server opens its own form below", async ({ page }) => {
    await stubApi(page);
    const wizard = await openWizard(page);
    await wizard.getByRole("combobox", { name: /Something else/ }).selectOption("mqtt");
    await expect(page.getByRole("combobox", { name: /^Type/ }).first()).toHaveValue("mqtt");
  });
});

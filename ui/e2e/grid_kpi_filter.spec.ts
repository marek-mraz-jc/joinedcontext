import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { attributeTable } from "./attributeAccess";
import { narrowed } from "./stubQuery";

// T-6328 (owner): a Bratislava KPI dataset on its Endpoint page. The filter row under the headers
// filters a date column with a date picker and an enum column from its values; the header's funnel
// filters to one value in two clicks. Each reaches the endpoint as `q`.

const API = "joinedcontext.com/v1alpha1";
const SLUG = "bk7m2qz4tv6xh3n5jb2ryd3wca";
const ENDPOINT = {
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name: "kpi", namespace: "bratislava", labels: { "joinedcontext.com/space": "statistika" } },
  spec: { contextSpaceRef: "statistika", slug: SLUG, audience: "public", enabledRepresentations: ["ngsi-ld"] },
  status: { phase: "Live" },
};
const SCHEMA = {
  $defs: {
    KeyPerformanceIndicator: {
      properties: {
        name: { type: "string", description: "What the indicator counts" },
        kpiValue: { type: "number", description: "The indicator's value" },
        category: { enum: ["doprava", "zdravie", "skolstvo"], description: "The area the indicator belongs to" },
        dateIssued: { type: "string", format: "date", description: "When the value was published" },
        period: { type: "object", properties: { from: { type: "string", format: "date" }, to: { type: "string", format: "date" } }, description: "The period the value covers" },
      },
    },
  },
};
const CATEGORIES = ["doprava", "zdravie", "skolstvo"];
const ROWS = Array.from({ length: 30 }, (_, n) => ({
  id: `urn:ngsi-ld:KeyPerformanceIndicator:dopravne-nehody-s-usmrtenim.pocet.${101 + (n % 3)}.${2012 + (n % 10)}`,
  type: "KeyPerformanceIndicator",
  name: { type: "Property", value: `Dopravné nehody s usmrtením ${2012 + (n % 10)}` },
  kpiValue: { type: "Property", value: (n * 7) % 40 },
  category: { type: "Property", value: CATEGORIES[n % 3] },
  dateIssued: { type: "Property", value: `20${12 + (n % 10)}-12-31` },
  period: { type: "Property", value: { from: `20${12 + (n % 10)}-01-01`, to: `20${12 + (n % 10)}-12-31` } },
}));

async function stub(page: Page, asked: URL[]): Promise<void> {
  const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
  await page.route("**/api/v1/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "steward", name: "Steward", email: "s@bratislava.sk", roles: ["portal-editor"] });
    if (path === "/api/v1/projects") return json(list([{ name: "bratislava" }]));
    if (path === "/api/v1/projects/bratislava/endpoints/kpi") return json(ENDPOINT);
    return json(list([]));
  });
  await page.route("**/cs/**", (route) => route.fulfill({ contentType: "application/ld+json", body: "[]" }));
  await page.route(`**/api/endpoint/${SLUG}/**`, (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, headers: Record<string, string> = {}) => route.fulfill({ contentType: "application/json", headers, body: JSON.stringify(body) });
    if (url.pathname.endsWith("/access/attributes")) return json(attributeTable("KeyPerformanceIndicator", ["name", "kpiValue", "category", "dateIssued", "period"]));
    if (url.pathname.endsWith("/schema/index.json")) return json({ models: [{ name: "kpi", version: 1 }] });
    if (url.pathname.endsWith("/schema/v1/json-schema")) return json(SCHEMA);
    if (url.pathname.endsWith("/ngsi-ld/v1/entities")) {
      asked.push(url);
      const offset = Number(url.searchParams.get("offset") ?? 0);
      const limit = Number(url.searchParams.get("limit") ?? 25);
      const all = narrowed(ROWS, url);
      return json(all.slice(offset, offset + limit), { "NGSILD-Results-Count": String(all.length) });
    }
    return route.fulfill({ status: 404, body: "{}" });
  });
}

const dir = join("test-results", "T-6328");
mkdirSync(dir, { recursive: true });

for (const width of [1440, 400]) {
  test(`a KPI dataset filters a date and an enum column from the filter row, and a value from the header, at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const asked: URL[] = [];
    await stub(page, asked);
    await page.goto("/projects/bratislava/endpoints/kpi?lang=sk");
    const grid = page.getByRole("grid");
    await expect(grid).toBeVisible();
    // A period reads as one range, the id as its short end.
    await expect(grid.getByText(/^1\.\s1\.\s2012\s–\u2060?\s31\.\s12\.\s2012$/).first()).toBeVisible();
    await page.screenshot({ path: join(dir, `kpi-filled-${width}.png`) });

    if (width === 1440) {
      await grid.getByRole("combobox", { name: /^Filter: Vydané/ }).selectOption("gte");
      const date = grid.getByLabel(/^Hodnota: Vydané/);
      await expect(date).toHaveAttribute("type", "date");
      await date.fill("2018-01-01");
      await expect.poll(() => asked.at(-1)?.searchParams.get("q")).toBe("dateIssued>=2018-01-01");

      await grid.getByRole("combobox", { name: /^Filter: Kategória/ }).selectOption("anyOf");
      await grid.getByRole("combobox", { name: /^Hodnota: Kategória/ }).selectOption("zdravie");
      await expect.poll(() => asked.at(-1)?.searchParams.get("q")).toBe('category=="zdravie";dateIssued>=2018-01-01');
    } else {
      // A phone has no filter row (T-6463): the same date picker and values sit in Tools, Filter.
      await expect(grid.getByRole("combobox", { name: /^Filter: Vydané/ })).toHaveCount(0);
      const toolbar = page.getByTestId("grid-toolbar");
      await toolbar.locator("summary", { hasText: "Nástroje" }).click();
      await toolbar.locator("summary", { hasText: "Filter" }).click();
      await page.getByRole("button", { name: "Pridať podmienku", exact: true }).click();
      await page.getByRole("combobox", { name: "Pole" }).first().selectOption("dateIssued");
      await page.getByRole("combobox", { name: "Operátor" }).first().selectOption("gte");
      const date = page.getByLabel("Hodnota", { exact: true }).first();
      await expect(date).toHaveAttribute("type", "date");
      await date.fill("2018-01-01");
      await expect.poll(() => asked.at(-1)?.searchParams.get("q")).toBe("dateIssued>=2018-01-01");

      await page.getByRole("button", { name: "Pridať podmienku", exact: true }).click();
      await page.getByRole("combobox", { name: "Pole" }).nth(1).selectOption("category");
      await page.locator("select").filter({ has: page.locator('option[value="zdravie"]') }).selectOption("zdravie");
      await expect.poll(() => asked.at(-1)?.searchParams.get("q")).toMatch(/^(category=="zdravie";dateIssued>=2018-01-01|dateIssued>=2018-01-01;category=="zdravie")$/);
      await page.keyboard.press("Escape");
    }
    // The stub applies q: only health indicators from 2018 on are left.
    await expect(grid.getByText("Dopravné nehody s usmrtením 2015")).toHaveCount(0);
    await expect(page.getByRole("list", { name: "Aktívne filtre" })).toBeVisible();
    await page.screenshot({ path: join(dir, `kpi-row-filtered-${width}.png`) });
    // The filter row clears its own; the toolbar's filters clear under their chips.
    await page.getByRole("button", { name: width === 1440 ? "Zrušiť všetky filtre" : "Zrušiť filtre", exact: true }).first().click();
    await expect.poll(() => asked.at(-1)?.searchParams.get("q")).toBeNull();

    // Two clicks from the header: the funnel, then Only on a value.
    await grid.getByRole("button", { name: /^Filtrovať stĺpec: Hodnota/ }).click();
    const filter = page.getByRole("dialog", { name: /: Hodnota$/ });
    await filter.getByRole("button", { name: "Iba toto: 14" }).click();
    await expect.poll(() => asked.at(-1)?.searchParams.get("q")).toBe("kpiValue==14");
    await expect(grid.getByText("Dopravné nehody s usmrtením 2013")).toHaveCount(0);
    await expect(grid.getByText("Dopravné nehody s usmrtením 2014").first()).toBeVisible();
    await page.screenshot({ path: join(dir, `kpi-header-filtered-${width}.png`) });
  });
}

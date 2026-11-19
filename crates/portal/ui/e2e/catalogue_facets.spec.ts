import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// The catalogue's filters (T-7317) as a visitor meets them: each filter its own collapsible
// section, six options and "Show all N" in place, the chosen ones as chips above the results, a
// filter every dataset passes last and closed, and at 400 px a Filters sheet. The stub filters and
// counts as the server does (api/catalogue.rs page_of), so each shot shows what the filters would.

const PUBLISHERS = [
  "Hlavné mesto SR Bratislava",
  "Platforma joinedcontext",
  "Hlavní město Praha",
  "City of Helsinki",
  "Ajuntament de Barcelona",
  "Banskobystrický samosprávny kraj",
  "Mesto Banská Bystrica",
  "Mesto Žilina",
  "Projekt Žilina, platforma joinedcontext",
  "Žilinská univerzita v Žiline",
  "Forum Virium Helsinki",
  "Mesto Banská Bystrica, odbor dopravy",
];
const THEMES = ["ENVI", "TRAN", "SOCI", "ECON", "EDUC", "GOVE", "HEAL"];
const DATASETS = Array.from({ length: 48 }, (_, i) => ({
  name: `dataset-${i}`,
  title: `${["Air quality", "Traffic counts", "Events", "Schools", "Bridges", "Budget"][i % 6]} ${i + 1}`,
  notes: "Read live from its Endpoint.",
  publisher: { name: `p${i % 12}`, title: PUBLISHERS[i % 12] },
  licence: { id: "cc-by", title: "Creative Commons Attribution 4.0" },
  formats: ["CSV", "JSON", "NGSI-LD"],
  themes: [THEMES[i % THEMES.length]],
  modified: "2026-11-19T10:38:12Z",
}));

type Dataset = (typeof DATASETS)[number];
const facetsOf = (rows: Dataset[], pick: (d: Dataset) => [string, string][]) => {
  const counts = new Map<string, { label: string; count: number }>();
  for (const row of rows) for (const [value, label] of pick(row)) counts.set(value, { label, count: (counts.get(value)?.count ?? 0) + 1 });
  return [...counts].map(([value, { label, count }]) => ({ value, label, count })).sort((a, b) => b.count - a.count);
};

function answer(url: URL) {
  const wanted = { publisher: url.searchParams.getAll("publisher"), theme: url.searchParams.getAll("theme") };
  const keeps = (d: Dataset, skip?: "publisher" | "theme") =>
    (skip === "publisher" || wanted.publisher.length === 0 || wanted.publisher.includes(d.publisher.name)) &&
    (skip === "theme" || wanted.theme.length === 0 || d.themes.some((t) => wanted.theme.includes(t)));
  const kept = DATASETS.filter((d) => keeps(d));
  return {
    total: kept.length,
    page: 1,
    pageSize: 20,
    datasets: kept.slice(0, 20),
    facets: {
      // Each facet ignores its own filter, so its other values stay selectable.
      publisher: facetsOf(DATASETS.filter((d) => keeps(d, "publisher")), (d) => [[d.publisher.name, d.publisher.title]]),
      theme: facetsOf(DATASETS.filter((d) => keeps(d, "theme")), (d) => d.themes.map((t) => [t, t])),
      format: facetsOf(kept, (d) => d.formats.map((f) => [f, f])),
      licence: facetsOf(kept, (d) => [[d.licence.id, d.licence.title]]),
      spatial: [],
      year: facetsOf(kept, () => [["2026", "2026"]]),
    },
    unavailable: [],
  };
}

async function stub(page: Page): Promise<void> {
  await page.route("**/api/v1/**", (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    if (url.pathname.endsWith("/auth/me")) return json({ title: "Unauthorized", status: 401 }, 401);
    if (url.pathname === "/api/v1/catalogue") return json(answer(url));
    if (url.pathname === "/api/v1/catalogue/readable") return json({ datasets: [], unanswered: 0 });
    return json({ apiVersion: "v1", kind: "List", items: [] });
  });
}

/** Full height without fullPage, so a sticky header is not cut (knowledge/portal-ui.md, T-6423). */
async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  if (!dir) return;
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  const { width } = page.viewportSize() ?? { width: 1440 };
  await page.setViewportSize({ width, height: Math.min(height, 4000) });
  await page.screenshot({ path: `${dir}/${name}.png` });
  await page.setViewportSize({ width, height: 900 });
}

test.describe("the catalogue's filters (T-7317)", () => {
  test("a visitor narrows by two filters, removes one chip, opens a long filter (1440 px)", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await stub(page);
    await page.goto("/catalogue?lang=en");
    const filters = page.getByRole("complementary", { name: "Filters" });
    const publisher = filters.getByRole("group", { name: "Publisher" });
    await expect(publisher.getByRole("checkbox")).toHaveCount(6);
    await expect(page.getByRole("status").filter({ hasText: "48 datasets" })).toBeVisible();
    // Format, Licence and Period: every dataset has these values, so they wait after the others, closed.
    await expect(filters.locator("details > summary")).toHaveText(["Publisher", "Theme", "Format", "Licence", "Period"]);
    await expect(filters.locator("details[open]")).toHaveCount(2);
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, "facets-none-1440");

    await publisher.getByRole("checkbox").nth(5).check();
    await filters.getByRole("group", { name: "Theme" }).getByRole("checkbox").first().check();
    const chips = page.getByRole("group", { name: "Chosen filters" });
    await expect(chips.getByRole("button", { name: /^Remove filter/ })).toHaveCount(2);
    await expect(page.getByRole("status").filter({ hasText: /^\d+ datasets?$/ })).not.toHaveText("48 datasets");
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, "facets-two-chosen-1440");

    await chips.getByRole("button", { name: /^Remove filter Theme/ }).click();
    await expect(chips.getByRole("button", { name: /^Remove filter/ })).toHaveCount(1);
    await publisher.getByRole("button", { name: "Show all 12" }).click();
    await expect(publisher.getByRole("checkbox")).toHaveCount(12);
    await publisher.getByRole("searchbox", { name: "Find a Publisher value" }).fill("žilin");
    await expect(publisher.getByRole("checkbox")).toHaveCount(3);
    await publisher.getByRole("searchbox", { name: "Find a Publisher value" }).fill("");
    await shot(page, "facets-long-expanded-1440");
    await page.getByRole("button", { name: "Clear all" }).click();
    await expect(page.getByRole("group", { name: "Chosen filters" })).toHaveCount(0);
  });

  test("on a phone the filters are a sheet with Clear and Show (400 px)", async ({ page }) => {
    await page.setViewportSize({ width: 400, height: 900 });
    await stub(page);
    await page.goto("/catalogue?lang=en&publisher=p6&theme=ENVI");
    await expect(page.getByRole("group", { name: "Chosen filters" }).getByRole("button", { name: /^Remove filter/ })).toHaveCount(2);
    // A chip removes its filter anywhere on it: about 36 px (2.8 rem at the 12.8 px root) for a finger on a phone.
    const chip = await page.getByRole("button", { name: /^Remove filter/ }).first().boundingBox();
    expect(chip?.height ?? 0).toBeGreaterThanOrEqual(35);
    await shot(page, "facets-two-chosen-400");
    await page.goto("/catalogue?lang=en");
    await expect(page.getByRole("status").filter({ hasText: "48 datasets" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Filters", exact: true })).toBeVisible();
    await shot(page, "facets-none-400");
    await page.getByRole("button", { name: "Filters", exact: true }).click();
    const sheet = page.getByRole("dialog", { name: "Filters" });
    await sheet.getByRole("group", { name: "Publisher" }).getByRole("button", { name: "Show all 12" }).click();
    await sheet.getByRole("group", { name: "Publisher" }).getByRole("checkbox", { name: /Mesto Žilina/ }).first().check();
    await expect(sheet.getByRole("button", { name: "Show 4 datasets" })).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: `${process.env.SHOTS_DIR ?? "test-results"}/facets-sheet-open-400.png` });
    await sheet.getByRole("button", { name: "Clear" }).click();
    await expect(sheet.getByRole("button", { name: "Show 48 datasets" })).toBeVisible();
    await sheet.getByRole("button", { name: "Show 48 datasets" }).click();
    await expect(sheet).toHaveCount(0);
  });
});

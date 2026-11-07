/**
 * T-2729, T-2877 — every endpoint of every project on dev, in one table, for an administrator of
 * the organization only (EP-08, EP-44, PF-61, R20).
 *
 * dev holds more than one project (`helsinki`, the region's `bbsk` and the city's
 * `banskabystrica`, T-2455). `demo.steward` administers the organization: the Organization page
 * gives them the Endpoints tab, the old `/endpoints` address opens it, the table holds every
 * project's endpoints and a project name opens that project's own Endpoints page. `demo.viewer`
 * does not administer it: no tab and no menu entry offer the table, both addresses land on
 * Settings, the API answers `404`, and the viewer's own project Endpoints still answer.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { STEWARD, VIEWER, signIn } from "./portal";

test.setTimeout(180_000);

/** The projects of the endpoints the organization route gives this person. */
async function projectsServed(page: Page): Promise<string[]> {
  const answer = await page.request.get("/api/v1/endpoints");
  expect(answer.ok(), "the organization's endpoints are read").toBe(true);
  const items = ((await answer.json()) as { items: { metadata: { namespace?: string } }[] }).items;
  return [...new Set(items.map((item) => item.metadata.namespace ?? ""))].sort();
}

/** The project column of the table, as a person reads it. */
async function projectsShown(page: Page): Promise<string[]> {
  const table = page.getByRole("table", { name: "All endpoints" });
  await expect(table).toBeVisible({ timeout: 60_000 });
  const cells = await table.getByRole("row").locator("td:first-child a").allTextContents();
  return [...new Set(cells.map((cell) => cell.trim()))].sort();
}

test("an administrator reads every project's endpoints in one table and opens one project's", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, "/endpoints?lang=en");
  try {
    await expect(page).toHaveURL(/\/organization\/endpoints/);
    await expect(page.getByRole("heading", { level: 1, name: "Organization" })).toBeVisible();
    await expect(page.getByRole("tab", { name: "All endpoints" })).toHaveAttribute("aria-selected", "true");
    await expect(page.getByRole("heading", { level: 2, name: "All endpoints" })).toBeVisible();
    const served = await projectsServed(page);
    expect(served.length, "the organization publishes endpoints in more than one project").toBeGreaterThan(1);
    expect(await projectsShown(page), "the table is what the route answers").toEqual(served);

    const other = served.find((project) => project !== "helsinki") ?? served[0];
    await page.getByRole("table", { name: "All endpoints" }).getByRole("link", { name: other, exact: true }).first().click();
    await expect(page).toHaveURL(new RegExp(`/projects/${other}/endpoints`));
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  } finally {
    await context.close();
  }
});

test("a person who does not administer the organization is offered no cross-project table", async ({ browser }) => {
  const { context, page } = await signIn(browser, VIEWER, "/endpoints?lang=en");
  try {
    // UI-75: the Administration page tells anybody else whose page it is and shows no tab.
    await expect(page.getByRole("heading", { level: 1, name: "Administration" })).toBeVisible({ timeout: 60_000 });
    await expect(page.getByText(/This page is for organization administrators/)).toBeVisible();
    await expect(page.getByRole("tab")).toHaveCount(0);
    await expect(page.getByRole("link", { name: "All endpoints" })).toHaveCount(0);

    await page.goto("/organization/endpoints?lang=en");
    await expect(page.getByText(/This page is for organization administrators/)).toBeVisible({ timeout: 60_000 });
    await expect(page.getByRole("table", { name: "All endpoints" })).toHaveCount(0);

    const refused = await page.request.get("/api/v1/endpoints");
    expect(refused.status(), "the server refuses the list, not only the menu").toBe(404);
    // The viewer's own project keeps its Endpoints, as before.
    const own = await page.request.get("/api/v1/projects/helsinki/endpoints");
    expect(own.ok(), "a project's own endpoints stay readable").toBe(true);
  } finally {
    await context.close();
  }
});

/**
 * From a list to the thing itself: the administrator clicks a row of the imported Helsinki
 * configuration and its page shows what that resource is. Each journey starts from the list page,
 * clicks the link a person reads, and waits for the detail page's own heading.
 */
import { expect, shoot, test } from "./fixtures";
import type { Page } from "@playwright/test";

async function opens(page: Page, link: string, heading: RegExp | string): Promise<void> {
  await page.getByRole("main").getByRole("link", { name: link, exact: true }).first().click();
  await expect(page.getByRole("main").getByRole("heading", { level: 1 }).first()).toHaveText(heading);
  await expect(page.getByText("The Portal did not load")).toBeHidden();
}

test("J050 the administrator opens the Endpoint Helsinki open data from the Endpoints list", async ({ as }, info) => {
  const page = await as("admin", "/projects/helsinki/endpoints");
  await opens(page, "Helsinki open data", /Helsinki open data/);
  await expect(page).toHaveURL(/\/projects\/helsinki\/endpoints\/[^/]+/);
  await shoot(page, info, "endpoint");
});

test("J051 the administrator opens the data model of Helsinki's city context and its types", async ({ as }, info) => {
  const page = await as("admin", "/projects/helsinki/models");
  await opens(page, "Helsinki city context", /Helsinki city context/);
  await expect(page.getByRole("main").getByText("BikeHireDockingStation").first()).toBeVisible();
  await shoot(page, info, "model");
});

test("J052 the administrator opens a pipeline from the Pipelines list", async ({ as }, info) => {
  const page = await as("admin", "/projects/helsinki/pipelines");
  await page.getByRole("main").getByRole("link", { name: "Open the pipeline" }).first().click();
  await expect(page).toHaveURL(/\/projects\/helsinki\/pipelines\/[^/]+/);
  await expect(page.getByRole("main").getByRole("heading", { level: 1 }).first()).toBeVisible();
  await shoot(page, info, "pipeline");
});

test("J053 the administrator opens the Policy app-air-quality-1 and reads whom it grants what", async ({ as }, info) => {
  const page = await as("admin", "/projects/helsinki/policies");
  await opens(page, "app-air-quality-1", /app-air-quality-1/);
  await expect(page.getByRole("main")).toContainText("app-air-quality");
  await shoot(page, info, "policy");
});

test("J054 the administrator opens the data source Air quality stations (FMI open data)", async ({ as }, info) => {
  const page = await as("admin", "/projects/helsinki/datasources");
  // A data source opens on its form: what it reads, ready to change.
  await opens(page, "Air quality stations (FMI open data)", "Edit data source");
  await expect(page.getByRole("main")).toContainText("fmi-air-quality");
  await shoot(page, info, "datasource");
});

test("J055 the administrator opens the Context Space Helsinki city context", async ({ as }, info) => {
  const page = await as("admin", "/projects/helsinki/spaces");
  await opens(page, "Helsinki city context", /Helsinki city context/);
  await shoot(page, info, "space");
});

test("J056 the administrator searches the catalogue for Žilina's datasets", async ({ as }, info) => {
  const page = await as("admin", "/catalogue");
  const search = page.getByRole("search", { name: "Search datasets" }).or(page.locator("form[role=search], [aria-label='Search datasets']")).first();
  await search.locator("input").first().fill("Žilina");
  await page.getByRole("button", { name: "Search", exact: true }).click();
  await expect(page.getByRole("main").getByRole("link", { name: /Žilina/ }).first()).toBeVisible();
  await expect(page.getByRole("main").getByRole("link", { name: /— Helsinki$/ })).toHaveCount(0);
  await shoot(page, info, "search");
});

test("J057 the administrator opens a public dataset of the catalogue", async ({ as }, info) => {
  const page = await as("admin", "/catalogue");
  await page.getByRole("main").getByRole("link", { name: /— Žilina$/ }).first().click();
  await expect(page).toHaveURL(/\/catalogue\/[^/]+/);
  await expect(page.getByRole("main").getByRole("heading", { level: 1 }).first()).toContainText("Žilina");
  await shoot(page, info, "dataset");
});

test("J058 the administrator opens the public connect page of the MCP server Helsinki, open data", async ({ as }, info) => {
  const page = await as("admin", "/projects/helsinki/mcp");
  await page.getByRole("main").getByRole("link", { name: "Open the public connect page" }).first().click();
  await expect(page).toHaveURL(/\/connect\/helsinki\/[^/]+/);
  await expect(page.getByRole("heading", { level: 1 }).first()).toBeVisible();
  await shoot(page, info, "connect");
});

test("J059 the administrator explores the entities of the space Helsinki, all of it in a table", async ({ as }, info) => {
  const page = await as("admin", "/projects/helsinki/explore");
  await page.getByRole("main").getByLabel("Context space").first().selectOption({ label: "Helsinki, all of it" });
  // The header, the filter row and at least one entity row; no "Not loaded".
  await expect.poll(() => page.getByRole("main").getByRole("row").count(), { timeout: 30_000 }).toBeGreaterThan(2);
  await expect(page.getByRole("main").getByText(/Not loaded/)).toHaveCount(0);
  await shoot(page, info, "table");
});

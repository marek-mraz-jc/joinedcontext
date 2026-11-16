/**
 * What a person without the right meets: the viewer of Žilina and a signed-in outsider (no group)
 * are refused Helsinki's project and the Organization's administration in the page and by the API
 * the page calls; the viewer reads Žilina but finds no control that writes. Anonymous visitors get
 * the public catalogue and the sign-in, nothing more (PF-61, PF-128).
 */
import { expect, shoot, test } from "./fixtures";

const NOT_OPEN = "This project is not open to you";

/** The status the Portal's API answers this page's own fetch (its session cookie, its CSRF rules). */
const status = (page: import("@playwright/test").Page, path: string) => page.evaluate((p) => fetch(p).then((r) => r.status), path);

test("J040 the viewer of Žilina opens Helsinki's project and is refused", async ({ as }, info) => {
  const page = await as("viewer", "/projects/helsinki/home");
  await expect(page.getByRole("main").getByText(NOT_OPEN)).toBeVisible();
  await shoot(page, info, "refused");
  await page.getByRole("main").getByRole("link", { name: "Go to the start" }).click();
  await expect(page).not.toHaveURL(/\/projects\/helsinki/);
});

test("J041 the viewer opens the Organization's administration and is sent to an administrator", async ({ as }, info) => {
  const page = await as("viewer", "/organization");
  await expect(page.getByRole("main").getByText("Ask an organization administrator")).toBeVisible();
  await expect(page.getByRole("main").getByRole("button", { name: "Edit" })).toHaveCount(0);
  await shoot(page, info, "refused");
});

test("J042 the API refuses the viewer Helsinki's spaces", async ({ as }) => {
  const page = await as("viewer", "/settings");
  expect([403, 404]).toContain(await status(page, "/api/v1/projects/helsinki/spaces"));
});

test("J043 the viewer reads Žilina's Context Spaces and finds no control that writes", async ({ as }, info) => {
  const page = await as("viewer", "/projects/zilina/spaces");
  await expect(page.getByRole("main").getByRole("heading", { level: 1 })).toHaveText("Context Spaces");
  await expect(page.getByRole("main").getByRole("table").first()).toBeVisible();
  const create = page.getByRole("main").getByRole("button", { name: "New Context Space" });
  if (await create.count()) await expect(create).toBeDisabled();
  await shoot(page, info, "read-only");
});

test("J044 a signed-in outsider sees only their own settings", async ({ as }, info) => {
  const page = await as("outsider", "/settings");
  await expect(page.getByRole("main").getByText("PERSONAL", { exact: false }).first()).toBeVisible();
  await expect(page.getByRole("main").getByText("Shared by everyone in the project")).toHaveCount(0);
  await shoot(page, info, "settings");
});

test("J045 the outsider opens Helsinki's project and is refused, in the page and by the API", async ({ as }, info) => {
  const page = await as("outsider", "/projects/helsinki/spaces");
  await expect(page.getByRole("main").getByText(NOT_OPEN)).toBeVisible();
  expect([403, 404]).toContain(await status(page, "/api/v1/projects/helsinki/spaces"));
  await shoot(page, info, "refused");
});

test("J046 the outsider is refused the Organization's administration", async ({ as }) => {
  const page = await as("outsider", "/organization");
  await expect(page.getByRole("main").getByText("Ask an organization administrator")).toBeVisible();
});

test("J047 an anonymous visitor gets the public catalogue, and a project address asks for the sign-in", async ({ browser, baseURL }, info) => {
  const context = await browser.newContext({ baseURL, viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();
  await page.goto("/catalogue");
  await expect(page.getByRole("heading", { name: "Catalogue", level: 1 })).toBeVisible();
  await expect(page.getByRole("button", { name: "Publish a dataset" })).toHaveCount(0);
  await shoot(page, info, "public-catalogue");
  await page.goto("/projects/helsinki/spaces");
  await expect(page.getByRole("button", { name: "Sign in" }).first()).toBeVisible();
  expect(await status(page, "/api/v1/projects/helsinki/spaces")).toBe(401);
  await context.close();
});

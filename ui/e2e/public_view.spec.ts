import { expect, test } from "@playwright/test";
import type { Page, Route } from "@playwright/test";
import { axeViolations } from "./axe";

// A data view published as a public link (T-3108, API/01 §33), opened by somebody who is not
// signed in: the page reads the public Endpoint anonymously and asks the Portal for nothing.
// T-5621: each failure of that read has its own state, and the page names the view by its
// catalogue title, never by the link's slug.

const SHOTS = process.env.SHOTS_DIR;
const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

/** The Portal's own API: nobody is signed in and nothing else answers. */
async function anonymous(page: Page, asked: string[]): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    asked.push(url.pathname);
    const unauthorized = url.pathname.endsWith("/auth/me");
    await route.fulfill({
      status: unauthorized ? 401 : 404,
      contentType: "application/problem+json",
      body: JSON.stringify({ title: unauthorized ? "Unauthorized" : "Not Found", status: unauthorized ? 401 : 404 }),
    });
  });
}

/** The public Endpoint: its catalogue entry, one type and its rows, or `fail` for every read. */
async function endpoint(page: Page, fail?: (route: Route) => Promise<void>): Promise<void> {
  await page.route("**/api/endpoint/**", async (route) => {
    if (fail) return fail(route);
    const url = new URL(route.request().url());
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (url.pathname === `/api/endpoint/${SLUG}/`) {
      return json({ "dct:title": [{ "@language": "fi", "@value": "Kaupunkipyöräasemat" }, { "@language": "en", "@value": "City bike stations in Helsinki" }] });
    }
    if (url.pathname.endsWith("/ngsi-ld/v1/types")) return json({ typeList: ["BikeHireDockingStation"] });
    return json([
      { id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:bikes:001", type: "BikeHireDockingStation", name: "Kamppi", availableBikeNumber: 4, freeSlotNumber: 12, status: "working" },
      { id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:bikes:002", type: "BikeHireDockingStation", name: "Töölöntori", availableBikeNumber: 0, freeSlotNumber: 20, status: "working" },
    ]);
  });
}

const problem = (status: number, headers: Record<string, string> = {}) => (route: Route) =>
  route.fulfill({ status, contentType: "application/problem+json", headers, body: JSON.stringify({ status }) });

async function shoot(page: Page, name: string): Promise<void> {
  if (!SHOTS) return;
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: `${SHOTS}/public-view-${name}-${width}.png`, fullPage: true });
  }
}

test.describe("a published data view", () => {
  test("a visitor reads the published type under its title and labelled heads", async ({ page }) => {
    const asked: string[] = [];
    await anonymous(page, asked);
    await endpoint(page);

    await page.goto(`/v/${SLUG}?lang=en`);
    await expect(page.getByRole("heading", { level: 1, name: "City bike stations in Helsinki" })).toBeVisible();
    const table = page.getByRole("table", { name: "Bike hire docking station" });
    await expect(table.getByText("Kamppi")).toBeVisible();
    await expect(table.getByRole("columnheader", { name: "Available bike number" })).toBeVisible();
    await expect(table.getByText("001")).toHaveAttribute("title", "urn:ngsi-ld:BikeHireDockingStation:hel.fi:bikes:001");
    expect(await table.textContent()).not.toContain("urn:");
    expect(await page.getByTestId("public-view").textContent()).not.toContain(SLUG);
    await expect(page.getByRole("button", { name: "Sign in" })).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    expect(asked.filter((path) => path.startsWith("/api/v1/projects"))).toEqual([]);
    await shoot(page, "filled");
  });

  const states: Array<[string, (route: Route) => Promise<void>, string | RegExp]> = [
    ["not-published", problem(404), "This view is not published, or not any more."],
    ["closed", problem(403), "This view is not open to the public."],
    ["busy", problem(429, { "Retry-After": "30" }), "Too many visits just now. Try again in 30 s."],
    ["down", problem(503), /the data service did not answer\. Try again in a minute/],
    ["offline", (route) => route.abort("internetdisconnected"), /the data service did not answer\. Try again in a minute/],
  ];
  for (const [name, fail, message] of states) {
    test(`a ${name} read shows its own state`, async ({ page }) => {
      await anonymous(page, []);
      await endpoint(page, fail);
      await page.goto(`/v/${SLUG}?lang=en`);
      await expect(page.getByRole("alert")).toContainText(message, { timeout: 10_000 });
      await expect(page.getByRole("heading", { level: 1, name: "Public data view" })).toBeVisible();
      expect(await page.getByTestId("public-view").textContent()).not.toContain(SLUG);
      expect(await axeViolations(page)).toEqual([]);
      await shoot(page, name);
    });
  }

  test("a failed read names the view when its catalogue entry answered", async ({ page }) => {
    await anonymous(page, []);
    await endpoint(page);
    await page.route("**/ngsi-ld/v1/types", problem(503));
    await page.goto(`/v/${SLUG}?lang=en`);
    await expect(page.getByRole("alert")).toContainText("“City bike stations in Helsinki” cannot be shown just now", { timeout: 10_000 });
    await expect(page.getByRole("heading", { level: 1, name: "City bike stations in Helsinki" })).toBeVisible();
    await shoot(page, "down-named");
  });

  test("a failed read comes back on Retry", async ({ page }) => {
    let down = true;
    await anonymous(page, []);
    await endpoint(page, async (route) => {
      if (down) return problem(503)(route);
      const url = new URL(route.request().url());
      if (url.pathname.endsWith("/ngsi-ld/v1/types")) {
        return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ typeList: ["BikeHireDockingStation"] }) });
      }
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify([{ id: "urn:ngsi-ld:BikeHireDockingStation:x:1", name: "Kamppi" }]) });
    });
    await page.goto(`/v/${SLUG}?lang=en`);
    await expect(page.getByRole("alert")).toContainText("the data service did not answer.", { timeout: 10_000 });
    down = false;
    await page.getByRole("button", { name: "Retry" }).click();
    await expect(page.getByRole("table").getByText("Kamppi")).toBeVisible();
  });
});

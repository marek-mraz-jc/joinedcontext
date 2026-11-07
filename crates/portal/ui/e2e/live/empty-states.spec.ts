/**
 * T-3246 — a list holding nothing says what belongs there and offers the first step, on the built
 * Portal, as a steward at a phone's and a desktop's width. The empty answer is made in this browser
 * only (`page.route`), so dev's own data is never touched: every request the journey sends is a read.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

test.setTimeout(180_000);

const PROJECT = "helsinki";
const EMPTY = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] };

const PAGES: { page: string; api: string; action: RegExp }[] = [
  { page: "pipelines", api: `**/api/v1/projects/${PROJECT}/pipelines`, action: /^New pipeline$/ },
  { page: "syncsources", api: `**/api/v1/projects/${PROJECT}/syncsources`, action: /^Add source$/ },
  { page: "workspaces", api: `**/api/v1/projects/${PROJECT}/workspaces`, action: /^Work on a copy$/ },
];

for (const width of [375, 1440]) {
  test(`at ${width}px an empty list says what belongs there and offers the first step`, async ({ browser }, info) => {
    const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/pipelines?lang=en`);
    try {
      await page.setViewportSize({ width, height: 900 });
      for (const { page: path, api, action } of PAGES) {
        await page.route(api, (route) =>
          route.request().method() === "GET"
            ? route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(EMPTY) })
            : route.fallback(),
        );
        await page.goto(`/projects/${PROJECT}/${path}?lang=en`);
        const empty = page.locator("[data-empty-state]").first();
        await expect(empty).toBeVisible({ timeout: 60_000 });
        await expect(empty.locator("p").nth(1)).not.toBeEmpty();
        await expect(empty.getByRole("link", { name: action }).or(empty.getByRole("button", { name: action }))).toBeVisible();
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
        await info.attach(`empty-${path}-${width}.png`, { body: await page.screenshot(), contentType: "image/png" });
      }
    } finally {
      await context.close();
    }
  });
}

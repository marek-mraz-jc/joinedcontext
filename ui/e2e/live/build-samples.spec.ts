/**
 * Build through an endpoint on dev (T-0666, AP-57, AG-49): a steward generates an app on
 * helsinki-news, the kit pass reads its samples through the agent proxy and the gateway, and the
 * preview carries data. Before the proxy's token named the gateway-wide audience, every type
 * read "No sample of … could be read: 401" and the preview's cards were empty.
 */
import { expect, test } from "@playwright/test";
import { guideShot } from "./guide";
import { STEWARD, signIn } from "./portal";

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const NAME = `news-samples-${SUFFIX}`;

test.setTimeout(600_000);

test("a build on helsinki-news reads its samples and the preview shows rows", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  const page = steward.page;

  await page.getByRole("button", { name: "Generate your own app" }).first().click();
  await page.getByLabel("Endpoint").first().selectOption({ label: "Helsinki city news" });
  await page
    .getByLabel("What should the app do?")
    .first()
    .fill("A table of the latest city news: the headline and when it was published.");
  await page.getByText("Details:", { exact: false }).first().click();
  await page.getByLabel("App name").fill(NAME);
  await page.getByRole("button", { name: "Generate the app" }).click();
  await page.waitForURL(new RegExp(`/projects/${PROJECT}/apps/${NAME}`), { timeout: 60_000 });

  await expect(page.getByText(/through the endpoint/).first()).toBeVisible({ timeout: 300_000 });
  const preview = page.locator("iframe").first();
  await expect(preview).toBeVisible({ timeout: 480_000 });
  await expect(page.getByText(/No sample of .* could be read/)).toHaveCount(0);

  // Rows in a table, or a tile counting more than zero: either is data the run read.
  const app = page.frameLocator("iframe").first();
  const rows = app.locator("table tbody tr").filter({ hasNotText: "No rows" });
  const counted = app.locator(".stat-value").filter({ hasText: /[1-9]/ });
  await expect
    .poll(async () => (await rows.count()) + (await counted.count()), { timeout: 90_000, intervals: [3_000] })
    .toBeGreaterThan(0);
  await guideShot(page, "app-1-generated-preview");
  await steward.context.close();
});

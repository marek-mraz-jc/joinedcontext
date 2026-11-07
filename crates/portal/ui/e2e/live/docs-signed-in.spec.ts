/**
 * T-6200 — the API chapter and the User Guide on dev are for signed-in people only: a steward
 * reads a guide page, a visitor asking for the same address is sent to sign in and reads nothing.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

test.setTimeout(120_000);

const GUIDE = "/docs/User-Guide/01-getting-started";

test("a steward reads a guide page, a visitor is sent to sign in", async ({ browser }, info) => {
  const { context, page } = await signIn(browser, STEWARD, GUIDE, (page) =>
    page.getByRole("heading", { level: 1, name: "Getting Started" }),
  );
  try {
    await expect(page).toHaveTitle("Getting Started");
    await info.attach("docs-signed-in.png", { body: await page.screenshot(), contentType: "image/png" });
  } finally {
    await context.close();
  }
  const visitor = await browser.newContext();
  try {
    const page = await visitor.newPage();
    await page.goto(GUIDE, { waitUntil: "load" });
    await expect(page).not.toHaveURL(new RegExp(`${GUIDE}$`));
    await expect(page.getByRole("heading", { name: "Getting Started" })).toHaveCount(0);
    await expect(page.locator("input[type=password]")).toBeVisible({ timeout: 60_000 });
  } finally {
    await visitor.close();
  }
});

import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const COPY = `try-${SUFFIX}`;

/**
 * A copy's preview on dev (T-1239, T-1240, T-1247; CC-78, CC-81, PF-83): the gateway serves the
 * copy's Endpoints on minted slugs while the preview runs and stops when it stops. The copy is
 * discarded at the end, whichever way the journey goes.
 */
test("a copy's preview answers on its own addresses and stops answering when stopped", async ({ browser }) => {
  const { page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/workspaces?lang=en`);
  try {
    // The dialog's own address (T-2749): what the button opens, a link and a reload open too.
    // Signed in first: the open dialog hides the navigation `signIn` waits for.
    await page.goto(`/projects/${PROJECT}/workspaces/new?lang=en`);
    const dialog = page.getByRole("dialog", { name: "Work on a copy" });
    await dialog.getByLabel(/^Name/).fill(COPY);
    await dialog.getByRole("button", { name: "Start the copy" }).click();
    await expect(dialog).toBeHidden({ timeout: 30_000 });

    await page.goto(`/projects/${PROJECT}/workspaces/${COPY}/try-it?lang=en`);
    await page.getByRole("button", { name: "Start the preview" }).click();
    await expect(page.getByTestId("preview-state")).toHaveText("Running", { timeout: 120_000 });
    const address = page.locator("section[aria-labelledby=preview-addresses] a").first();
    await expect(address).toBeVisible();
    const url = new URL((await address.getAttribute("href")) ?? "");
    const types = `${url.pathname}/ngsi-ld/v1/types`;

    // The gateway asks the Portal every ten seconds; a slug it does not know answers 404.
    await expect
      .poll(async () => (await page.request.get(types)).status(), { timeout: 60_000, intervals: [3_000] })
      .not.toBe(404);

    await page.getByRole("button", { name: "Stop" }).click();
    await expect(page.getByTestId("preview-state")).toHaveText("Stopped", { timeout: 30_000 });
    await expect
      .poll(async () => (await page.request.get(types)).status(), { timeout: 60_000, intervals: [3_000] })
      .toBe(404);
  } finally {
    const csrf = (await page.context().cookies()).find((c) => c.name.toLowerCase().includes("csrf"))?.value ?? "";
    await page.request.delete(`/api/v1/projects/${PROJECT}/workspaces/${COPY}`, { headers: { "x-csrf-token": csrf } });
  }
});

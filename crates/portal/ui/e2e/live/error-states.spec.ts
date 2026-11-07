/**
 * T-3244 — a page whose API fails says what failed, why, what to do, offers Retry and the
 * reference to copy; once the API answers again, Retry brings the page back. The failure is
 * made in this browser only (`page.route`), so dev is never touched: every request the journey
 * sends is a read.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, VIEWER, signIn } from "./portal";

test.setTimeout(180_000);

const PROJECT = "helsinki";

for (const [who, person] of [
  ["steward", STEWARD],
  ["viewer", VIEWER],
] as const) {
  for (const width of [375, 1440]) {
    test(`${who} at ${width}px: a failed list says why, offers Retry and the reference, and Retry brings it back`, async ({
      browser,
    }, info) => {
      const { context, page } = await signIn(browser, person, `/projects/${PROJECT}/approvals?lang=en`);
      try {
        await page.setViewportSize({ width, height: 900 });
        let failing = true;
        await page.route(`**/api/v1/projects/${PROJECT}/changes`, async (route) => {
          if (!failing) return route.fallback();
          await route.fulfill({
            status: 503,
            contentType: "application/problem+json",
            headers: { "x-request-id": "journey-0f3c" },
            body: JSON.stringify({ type: "about:blank", title: "Service Unavailable", status: 503, detail: "The forge does not answer." }),
          });
        });
        await page.reload();
        const alert = page.getByRole("alert").filter({ hasText: "The forge does not answer." });
        await expect(alert).toBeVisible({ timeout: 60_000 });
        await expect(alert.getByText("journey-0f3c")).toBeVisible();
        await expect(alert.getByRole("button", { name: "Copy the reference journey-0f3c" })).toBeVisible();
        // Nothing scrolls sideways at a phone's width.
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
        await info.attach(`failed-${who}-${width}.png`, { body: await page.screenshot(), contentType: "image/png" });

        failing = false;
        await alert.getByRole("button", { name: "Retry" }).click();
        await expect(page.getByRole("table", { name: "Approvals" })).toBeVisible({ timeout: 60_000 });
        await expect(alert).toBeHidden();
        await info.attach(`recovered-${who}-${width}.png`, { body: await page.screenshot(), contentType: "image/png" });
      } finally {
        await context.close();
      }
    });
  }
}

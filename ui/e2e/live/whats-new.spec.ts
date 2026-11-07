/**
 * T-3271 — a person sees that something changed: Help carries a dot until "What's new" is read,
 * the list reads in their language, and once read the dot is gone. Read-only.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

test.setTimeout(120_000);

const WORDS = {
  en: { help: /^Help, \d+ new change/, menu: /^What's new \(\d+ new\)$/, title: "What's new", read: "Help" },
  sk: { help: /^Pomocník, \d+ nov/, menu: /^Čo je nové \(\d+ nové\)$/, title: "Čo je nové", read: "Pomocník" },
} as const;

for (const lang of ["en", "sk"] as const) {
  for (const width of [375, 1440]) {
    test(`${lang} at ${width}px: Help shows what changed until it is read`, async ({ browser }, info) => {
      const { context, page } = await signIn(browser, STEWARD, `/projects/helsinki/spaces?lang=${lang}`);
      try {
        await page.setViewportSize({ width, height: 900 });
        // A first visit in this browser: nothing read yet.
        await page.evaluate(() => localStorage.removeItem("jc.whatsNewSeen"));
        await page.reload();
        const words = WORDS[lang];
        await page.getByRole("button", { name: words.help }).click({ timeout: 60_000 });
        await page.getByRole("menuitem", { name: words.menu }).click();
        const dialog = page.getByRole("dialog", { name: words.title });
        await expect(dialog).toBeVisible();
        expect(await dialog.getByRole("heading", { level: 3 }).count()).toBeGreaterThanOrEqual(10);
        await info.attach(`whats-new-${lang}-${width}.png`, { body: await page.screenshot(), contentType: "image/png" });
        await page.keyboard.press("Escape");
        await expect(page.getByRole("button", { name: words.read, exact: true })).toBeVisible();
      } finally {
        await context.close();
      }
    });
  }
}

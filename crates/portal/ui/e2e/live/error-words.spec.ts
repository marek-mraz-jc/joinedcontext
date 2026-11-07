/**
 * T-3243 — an error says what to do about it, in the person's language: an address that names
 * nothing reads the API's sentence and the hint of its problem type, in English and in Slovak, at
 * a phone's and a desktop's width. Every request is a read of something that does not exist.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

test.setTimeout(180_000);

const PROJECT = "helsinki";
const MISSING = "no-such-space-t3243";
const HINT = {
  en: "Check the name in the address; it does not exist or is not shared with you.",
  sk: "Skontrolujte názov v adrese; neexistuje alebo s vami nie je zdieľaný.",
} as const;

for (const lang of ["en", "sk"] as const) {
  for (const width of [375, 1440]) {
    test(`${lang} at ${width}px: a missing space says why and what to do`, async ({ browser }, info) => {
      const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces?lang=${lang}`);
      try {
        await page.setViewportSize({ width, height: 900 });
        // The API's own answer: a catalogued type, a sentence, nothing internal.
        const answer = await page.request.get(`/api/v1/projects/${PROJECT}/spaces/${MISSING}`);
        expect(answer.status()).toBe(404);
        const problem = (await answer.json()) as { type?: string; detail?: string };
        expect(problem.type).toBe("https://joinedcontext.com/errors/resource-not-found");
        expect(problem.detail ?? "").not.toMatch(/panic|stack|::|src\//);

        await page.goto(`/projects/${PROJECT}/spaces/${MISSING}?lang=${lang}`);
        await expect(page.getByText(HINT[lang])).toBeVisible({ timeout: 60_000 });
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
        await info.attach(`missing-space-${lang}-${width}.png`, { body: await page.screenshot(), contentType: "image/png" });
      } finally {
        await context.close();
      }
    });
  }
}

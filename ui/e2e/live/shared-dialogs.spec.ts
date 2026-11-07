/**
 * T-3281 — the App lifecycle question is the shared dialog on dev: it takes the focus, Tab stays
 * inside it, Escape answers no and nothing is proposed. Read-only: the question is asked and never
 * answered, at a phone's and a desktop's width, in English and Slovak.
 */
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { STEWARD, goSignedIn } from "./portal";

type Words = { rowActions: { more: string }; apps: { publishAction: string; retireAction: string } };
const here = dirname(fileURLToPath(import.meta.url));
const wordsOf = (lang: string): Words => JSON.parse(readFileSync(join(here, `../../src/locales/${lang}.json`), "utf8")) as Words;

test.setTimeout(240_000);

const PROJECT = "helsinki";

for (const [lang, words] of [
  ["en", wordsOf("en")],
  ["sk", wordsOf("sk")],
] as const) {
  for (const width of [375, 1440]) {
    test(`at ${width} px in ${lang} the lifecycle question holds the focus and Escape answers no`, async ({ browser }, info) => {
      const context = await browser.newContext({ viewport: { width, height: 900 } });
      const page = await context.newPage();
      try {
        await goSignedIn(page, STEWARD, `/projects/${PROJECT}/apps?lang=${lang}`);
        const listed = await page.request.get(`/api/v1/projects/${PROJECT}/apps`);
        expect(listed.ok(), await listed.text()).toBe(true);
        const apps = ((await listed.json()) as { items?: { metadata: { name: string } }[] }).items ?? [];
        test.skip(apps.length === 0, "dev holds no App in helsinki");

        const writes: string[] = [];
        page.on("request", (request) => {
          if (request.method() !== "GET" && request.url().includes("/api/v1/projects/")) writes.push(request.url());
        });
        const more = page.getByRole("main").getByRole("button", { name: new RegExp(`^${words.rowActions.more.split("{")[0]}`) });
        await more.first().click({ timeout: 60_000 });
        const item = page
          .getByRole("menuitem", { name: new RegExp(`^(${words.apps.publishAction}|${words.apps.retireAction})`) })
          .and(page.locator(":not([aria-disabled=true])"));
        test.skip((await item.count()) === 0, "the first App may be neither published nor retired by the steward");
        await item.first().click();

        const dialog = page.getByRole("dialog");
        await expect(dialog).toBeVisible();
        expect(await dialog.evaluate((node) => node.contains(document.activeElement))).toBe(true);
        for (let step = 0; step < 6; step += 1) {
          await page.keyboard.press("Tab");
          expect(await dialog.evaluate((node) => node.contains(document.activeElement))).toBe(true);
        }
        await info.attach(`lifecycle-${lang}-${width}.png`, { body: await page.screenshot(), contentType: "image/png" });
        await page.keyboard.press("Escape");
        await expect(dialog).toBeHidden();
        expect(writes, "the question was not answered, so nothing was proposed").toEqual([]);
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
      } finally {
        await context.close();
      }
    });
  }
}

/**
 * T-3269 — help for the page a person is on: what it is for, its three steps, and a question for
 * the assistant about it, in English and Slovak at a phone's and a desktop's width. Read-only: the
 * question is written into the dock for the person to send, and nothing is sent.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

test.setTimeout(120_000);

const WORDS = {
  en: { help: /^Help/, menu: "Help for this page", title: "Pipelines", ask: "Ask the assistant about this page" },
  sk: { help: /^Pomocník/, menu: "Pomoc k tejto stránke", title: "Pipeliny", ask: "Spýtať sa asistenta na túto stránku" },
} as const;

for (const lang of ["en", "sk"] as const) {
  for (const width of [375, 1440]) {
    test(`${lang} at ${width}px: the pipelines page explains itself and hands the assistant a question`, async ({ browser }, info) => {
      const { context, page } = await signIn(browser, STEWARD, `/projects/helsinki/pipelines?lang=${lang}`);
      try {
        await page.setViewportSize({ width, height: 900 });
        const words = WORDS[lang];
        await page.getByRole("button", { name: words.help }).click({ timeout: 60_000 });
        await page.getByRole("menuitem", { name: words.menu }).click();
        const dialog = page.getByRole("dialog", { name: words.title });
        await expect(dialog).toBeVisible();
        expect(await dialog.getByRole("listitem").count()).toBe(3);
        await info.attach(`page-help-${lang}-${width}.png`, { body: await page.screenshot(), contentType: "image/png" });
        await dialog.getByRole("button", { name: words.ask }).click();
        await expect(dialog).toBeHidden();
      } finally {
        await context.close();
      }
    });
  }
}

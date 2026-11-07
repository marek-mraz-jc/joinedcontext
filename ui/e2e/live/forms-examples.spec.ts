/**
 * Every create form shows at most one example, and the field accepts it (T-2882; T-1606, UI-02).
 *
 * The owner's rule of 2026-09-25: an example on field after field is clutter that drowns the one
 * that helps, so a form carries one, on the field whose value carries the pattern. This journey
 * held T-1606's older promise, that a form filled from nothing but its examples checks green; that
 * promise went with the rule, and the red-verdict path it also walked is verdict-gate.spec.ts's.
 *
 * What one kind's case does: open New, count the "Use the example" offers, take the one there is,
 * and read the field: no error beside it, since the example is a value the field accepts. Nothing
 * is checked or proposed, so dev keeps what it had.
 *
 * The pages are discovered from the navigation rather than listed here: a kind whose form is added
 * later is covered without editing this file, and the report says which pages were walked.
 */
import { writeFileSync } from "node:fs";
import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

const PROJECT = "helsinki";
const REPORT = process.env.FORMS_REPORT ?? "test-results/forms-examples.json";

test.setTimeout(1_800_000);

interface Walked {
  page: string;
  examples: number;
  finding?: string;
}

/** Every page of the project the navigation links to, in the order a person meets them. */
async function projectPages(page: Page): Promise<string[]> {
  const hrefs = await page
    .getByRole("navigation", { name: "Main navigation" })
    .locator(`a[href^="/projects/${PROJECT}/"]`)
    .evaluateAll((links) =>
      links.map((a) => new URL((a as HTMLAnchorElement).href).pathname),
    );
  return [...new Set(hrefs)];
}

/** What the form is saying right now: its alerts, its field errors, in one line. */
async function said(dialog: Locator): Promise<string> {
  const lines = await dialog
    .locator("[role=alert], [role=status], .text-danger")
    .allInnerTexts();
  return lines.join(" | ").replace(/\s+/g, " ").slice(0, 400) || "nothing";
}

test("every create form shows at most one example, and the field accepts it", async ({
  browser,
}) => {
  const steward = await signIn(
    browser,
    STEWARD,
    `/projects/${PROJECT}/spaces?lang=en`,
  );
  const walked: Walked[] = [];
  const wrong: string[] = [];

  try {
    for (const path of await projectPages(steward.page)) {
      await steward.page.goto(`${path}?lang=en`, { waitUntil: "load" });
      const opener = steward.page
        .getByRole("main")
        .getByRole("button", { name: /^New / })
        .first();
      if (!(await opener.count()) || (await opener.isDisabled())) {
        continue;
      }
      await opener.click();
      const dialog = steward.page.getByTestId("form-page");
      if (!(await dialog.isVisible().catch(() => false))) {
        continue;
      }

      const offers = dialog.getByRole("button", { name: "Use the example" });
      const examples = await offers.count();
      let finding: string | undefined;
      if (examples > 1) {
        finding = `${examples} examples on one form`;
      } else if (examples === 1) {
        const offer = offers.first();
        // The input the offer fills sits in the same row (theme.tsx BaseInputTemplate).
        const field = offer.locator("xpath=ancestor::div[contains(@class, 'flex')][1]").locator("input, textarea").first();
        await offer.click();
        await field.blur();
        if ((await field.getAttribute("aria-invalid")) === "true") {
          finding = `the field refuses its own example: ${await said(dialog)}`;
        }
      }
      walked.push({ page: path, examples, finding });
      if (finding !== undefined) {
        wrong.push(`${path}: ${finding}`);
      }

      await steward.page.keyboard.press("Escape");
      await steward.page.waitForTimeout(300);
    }

    writeFileSync(REPORT, JSON.stringify({ walked }, null, 2));
    console.log(`forms examples: ${walked.length} forms -> ${REPORT}`);
    expect(
      walked.length,
      "at least the six kinds with a create form were walked",
    ).toBeGreaterThan(3);
    expect(wrong, "a form with more than one example, or one its field refuses").toEqual([]);
  } finally {
    await steward.context.close();
  }
});

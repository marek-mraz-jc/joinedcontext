/**
 * T-6454, journey 2: Complete this space with a workbook, the page path. T-6445's journey owns the
 * approval and the load through the assistant, so this clip stops at the checked drafts and proposes
 * nothing; `finally` sweeps its drafts.
 */
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { EDITOR, sweepDrafts } from "./portal";
import { SIZES, clipOf } from "./clips";

const PROJECT = "helsinki";
const WORKBOOK = join(dirname(fileURLToPath(import.meta.url)), "fixtures/kulturne-miesta-bbsk.xlsx");
const SHEET = "Kultúrne miesta";

test.setTimeout(240_000);

for (const size of SIZES) {
  test(`Complete this space with a workbook at ${size.width}`, async ({ browser }) => {
    const space = `t6454-clip-${size.width}-${Date.now().toString().slice(-6)}`;
    const clip = await clipOf(browser, EDITOR, "space-complete", size, `/projects/${PROJECT}/spaces/complete?lang=en`);
    const { page, context } = clip;
    try {
      await clip.step("Complete this space: name a space and bring a file", async () => {
        await expect(page.getByRole("heading", { name: "Complete this space", level: 1 })).toBeVisible({ timeout: 60_000 });
      });
      await clip.step("Name the space and choose the .xlsx", async () => {
        await page.locator("#complete-space").fill(space);
        await page.keyboard.press("Tab");
        await page.getByLabel("Files", { exact: true }).setInputFiles(WORKBOOK);
      });
      await clip.step("Pick the sheet: the page counts its rows", async () => {
        await page.locator("#complete-sheet-0").selectOption(SHEET);
        await expect(page.locator("#complete-sheet-0")).toContainText("rows");
      });
      await clip.step("Complete: the page drafts the model and the space and checks each", async () => {
        await page.locator("#complete-btn").click();
        await expect(page.locator("#complete-propose")).toBeVisible({ timeout: 90_000 });
        await page.getByTestId("complete-draft-DataModel").scrollIntoViewIfNeeded();
        await expect(page.getByTestId("complete-draft-DataModel")).toContainText("Check passed");
      });
      await clip.step("Every draft checked, ready to propose", async () => {
        await page.locator("#complete-propose").scrollIntoViewIfNeeded();
      });
    } finally {
      await sweepDrafts(context, page, PROJECT, new RegExp(`^${space}`)).catch((failed: unknown) => console.log(String(failed)));
      await clip.finish();
    }
  });
}

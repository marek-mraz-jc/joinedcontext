/**
 * T-3238…T-3242 (UI-88…UI-92): a person new to the Portal reaches a pipeline with Ctrl+K and its
 * name, reads where they are in the breadcrumb, finds it again among the recent pages, stars it
 * and unstars it, sends its link to a colleague who opens the same page, and reads the page's
 * shortcuts with "?". Read-only on dev: the only write is the steward's own star, taken away
 * again before the journey ends.
 */
import { expect, test } from "@playwright/test";
import { signIn, STEWARD, VIEWER } from "./portal";

test.setTimeout(240_000);

const PROJECT = "helsinki";
const PIPELINE = "air-quality";

test("Ctrl+K and a name reach a pipeline; its link opens the same page for a colleague", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);
  const page = steward.page;
  try {
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible({ timeout: 60_000 });

    // Two keystrokes and a name (UI-88).
    await page.keyboard.press("Control+k");
    const search = page.getByRole("combobox", { name: "Name to go to" });
    await expect(search).toBeFocused();
    await search.fill(PIPELINE);
    const pipelines = page.getByRole("group", { name: "Pipelines" });
    await expect(pipelines.getByRole("option").first()).toBeVisible({ timeout: 30_000 });
    await pipelines.getByRole("option").first().click();
    await expect(page).toHaveURL(new RegExp(`/projects/${PROJECT}/pipelines/${PIPELINE}/edit`), { timeout: 30_000 });

    // Where one is: organization › project › section › item, each a link (UI-89).
    const crumbs = page.getByRole("navigation", { name: "Breadcrumb" });
    await expect(crumbs.getByRole("link", { name: "Pipelines" })).toBeVisible({ timeout: 30_000 });
    await expect(crumbs.getByRole("link", { name: PIPELINE })).toHaveAttribute("aria-current", "page");

    // The page's shortcuts (UI-92).
    await page.locator("main h1").click();
    await page.keyboard.press("Shift+Slash");
    const help = page.getByRole("dialog", { name: "Keyboard shortcuts" });
    await expect(help.getByText("Go to a page, an item or an action; twice asks the assistant")).toBeVisible();
    await page.keyboard.press("Escape");

    // Starred, and the star taken away again: the steward's own preference (UI-90).
    const star = page.getByRole("button", { name: "Star this page" });
    await star.click();
    await expect(star).toHaveAttribute("aria-pressed", "true");
    await page.goto(`/projects/${PROJECT}/spaces?lang=en`);
    await page.keyboard.press("Control+k");
    await expect(page.getByRole("group", { name: "Starred" })).toContainText(PIPELINE, { timeout: 30_000 });
    await expect(page.getByRole("group", { name: "Recently opened" })).toContainText(PIPELINE);
    await page.getByRole("group", { name: "Starred" }).getByRole("option").first().click();
    await expect(page).toHaveURL(new RegExp(`/pipelines/${PIPELINE}/edit`));
    await page.getByRole("button", { name: "Star this page" }).click();
    await expect(page.getByRole("button", { name: "Star this page" })).toHaveAttribute("aria-pressed", "false");

    // The link a colleague is sent opens the same page for them (UI-91).
    await steward.context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await page.getByRole("button", { name: "Copy link" }).click();
    const link = await page.evaluate(() => navigator.clipboard.readText());
    expect(link).toMatch(new RegExp(`/projects/${PROJECT}/pipelines/${PIPELINE}/edit$`));
    const viewer = await signIn(browser, VIEWER, `${new URL(link).pathname}?lang=en`);
    try {
      await expect(viewer.page.getByRole("heading", { level: 1 })).toContainText(PIPELINE, { timeout: 60_000 });
    } finally {
      await viewer.context.close();
    }
  } finally {
    await steward.context.close();
  }
});

/**
 * T-3220 — the assistant's administration and the project's settings, as a person who has never
 * filled them meets them on dev. Each form is opened, read and left: nothing is proposed.
 *
 * - A knowledge source: the start address says how a PDF is read, and a cron typed wrong is
 *   refused in words that name the format.
 * - An assistant deployment on a public channel: its origins, rate limit and budget are asked for
 *   at their fields before anything is sent (MF-52), instead of jc-core refusing the proposal.
 * - The project's settings: Edit shows each field's help, as its create form does.
 */
import { expect, test } from "@playwright/test";
import { openedForm } from "./kindJourney";
import { STEWARD, signIn } from "./portal";

const PROJECT = "helsinki";

test.setTimeout(240_000);

test("a steward reads what each assistant and settings form needs, and proposes nothing", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/knowledgesources?lang=en`);
  try {
    await page.getByRole("main").getByRole("button", { name: "New knowledge source" }).first().click();
    let form = await openedForm(page);
    await expect(form.getByText("The address of a PDF or another document is read as that document.")).toBeVisible();
    await form.locator("#root_schedule").fill("every night");
    await form.locator("#root_schedule").blur();
    await expect(form.getByText(/Five fields separated by spaces/)).toBeVisible();
    await page.keyboard.press("Escape");

    await page.goto(`/projects/${PROJECT}/assistantdeployments?lang=en`, { waitUntil: "load" });
    await page.getByRole("main").getByRole("button", { name: "New assistant deployment" }).first().click();
    form = await openedForm(page);
    await form.locator("#root_name").fill("town-help-check");
    await form.locator("#root_publicId").fill("town-help-check");
    await form.locator("#root_channel").selectOption("public");
    await form.getByRole("button", { name: /Check|Propose/ }).first().click();
    // The three a public channel needs, each refused at its own field, before the API is asked.
    await expect(form.locator("#root_allowedOrigins__error, #root_rateLimit__error, #root_budget__error").first()).toBeVisible({
      timeout: 30_000,
    });
    await page.keyboard.press("Escape");

    await page.goto(`/projects/${PROJECT}/settings?lang=en`, { waitUntil: "load" });
    await page.getByRole("button", { name: /^Edit / }).first().click();
    form = await openedForm(page);
    await expect(form.getByLabel(/^Title/)).toHaveAccessibleDescription(
      "The name people read in the project switcher and at the top of every page of it.",
      { timeout: 30_000 },
    );
  } finally {
    await context.close();
  }
});

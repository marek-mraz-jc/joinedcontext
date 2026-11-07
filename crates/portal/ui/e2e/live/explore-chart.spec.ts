/**
 * T-3257 — a chart from the explorer on dev: the steward picks a type the endpoint grants, charts
 * the first attribute the dialog offers with the chart it suggests, sees it drawn with its axis in
 * words, and saves it to a new dashboard; the save is a proposed change, which the journey rejects
 * afterwards so dev keeps no dashboard of its own.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, proposedChange, reject, signIn } from "./portal";

test.setTimeout(300_000);

test("a person charts an attribute from the explorer and proposes it on a new dashboard", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, "/projects/helsinki/explore?lang=en");
  let change = "";
  try {
    await page.getByRole("button", { name: "Helsinki city context" }).click();
    const types = page.locator("#explore-type");
    await expect(types.locator("optgroup option").first()).toBeAttached({ timeout: 60_000 });
    const granted = await types
      .locator("option")
      .evaluateAll((options) =>
        options
          .filter((option) => option.getAttribute("value") && option.parentElement?.tagName !== "OPTGROUP")
          .map((option) => option.getAttribute("value") ?? ""),
      );
    await types.selectOption(granted[0]);
    await expect(page.locator("tbody tr").first()).toBeVisible({ timeout: 60_000 });

    await page.getByRole("button", { name: "Chart", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: /^Chart an attribute of / });
    const attribute = dialog.getByLabel("Attribute");
    const first = await attribute.locator("option[value]:not([value=''])").first().getAttribute("value");
    expect(first, "the type has an attribute to chart").toBeTruthy();
    await attribute.selectOption(first ?? "");
    // Whatever it suggests, the chart is drawn with its axis said in words.
    await expect(dialog.getByText(/^(Number of entities|Time in )/).first()).toBeVisible({ timeout: 120_000 });

    await dialog.getByLabel("Name of the new dashboard").fill(`journey-chart-${Date.now()}`);
    await dialog.getByRole("button", { name: "Save to the dashboard" }).click();
    change = await proposedChange(page);
  } finally {
    if (change) await reject(page, "helsinki", change, "Rejected by a live journey: the chart was only proved.");
    await context.close();
  }
  expect(change, "the chart was proposed as a change").not.toBe("");
});

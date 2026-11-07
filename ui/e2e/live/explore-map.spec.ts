/**
 * T-3256 — every located type in helsinki's seeded spaces opens on the explorer's map on dev: for
 * each space and each type its chosen endpoint grants, a type with a Map tab draws its located
 * entities ("n of m on the map", n > 0). Read only.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

test.setTimeout(900_000);

test("each located type of the seeded spaces opens on the map", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, "/projects/helsinki/explore?lang=en");
  const opened: string[] = [];
  const empty: string[] = [];
  try {
    await expect(page.getByRole("heading", { level: 1, name: "Explore data" })).toBeVisible({ timeout: 60_000 });
    const space = page.getByLabel("Context space");
    await expect(space.locator("option[value]:not([value=''])").first()).toBeAttached({ timeout: 60_000 });
    const spaces = await space.locator("option").evaluateAll((options) =>
      options.map((option) => option.getAttribute("value") ?? "").filter(Boolean),
    );
    for (const name of spaces) {
      await space.selectOption(name);
      const types = page.locator("#explore-type");
      const granted = await types
        .locator("option")
        .evaluateAll((options) =>
          options
            .filter((option) => option.getAttribute("value") && !option.textContent?.includes("not granted"))
            .map((option) => option.getAttribute("value") ?? ""),
        );
      for (const type of granted) {
        await types.selectOption(type);
        const map = page.getByRole("tab", { name: "Map" });
        if ((await map.count()) === 0) continue;
        await map.click();
        const status = page.getByRole("status").filter({ hasText: /on the map|has a location/ });
        await expect(status).toBeVisible({ timeout: 120_000 });
        const said = (await status.textContent()) ?? "";
        (/^[1-9][\d,. ]* of /.test(said) ? opened : empty).push(`${name}/${type}: ${said}`);
        await page.getByRole("tab", { name: "Entities" }).click();
      }
    }
  } finally {
    await context.close();
  }
  expect(opened.length, "at least one located type is seeded").toBeGreaterThan(0);
  expect(empty, `located types whose map stayed empty:\n${empty.join("\n")}`).toEqual([]);
});

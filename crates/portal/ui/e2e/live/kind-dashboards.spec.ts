/**
 * T-1541 — UI-44, PF-50: a Dashboard, end to end on dev by a person.
 *
 * Created on the Dashboards page ("New dashboard": a name, a title and one page showing one of the
 * project's layers), approved, retitled through the dashboard's own menu, removed; the assistant
 * opens the same form; a viewer finds the page's write controls disabled with a reason.
 */
import { expect } from "@playwright/test";
import { guideShot } from "./guide";
import { PROJECT, kindJourney, proposeFrom, rowAction } from "./kindJourney";

kindJourney({
  task: "t1541",
  kind: "Dashboard",
  plural: "dashboards",
  page: `/projects/${PROJECT}/dashboards`,
  create: async (page, name) => {
    await page.getByRole("main").getByRole("button", { name: "New dashboard" }).first().click();
    const form = page.getByTestId("form-page");
    await expect(form).toBeVisible({ timeout: 30_000 });
    await form.locator("#root_name").fill(name);
    await form.locator("#root_title").fill(name);
    // A dashboard shows at least one page, and a page at least one layer (jc-core `dashboard.rs`).
    if (!(await form.locator('[id^="root_pages_0"]').count())) {
      await form.locator("#root_pages__add").click();
    }
    await form.locator('input[type="checkbox"][id^="root_pages_0_layers"]').first().check();
    await guideShot(page, "dashboard-1-form");
    await proposeFrom(form);
  },
  change: {
    apply: async (page, name) => {
      // More than one dashboard: the header's chooser picks this one before its menu is its own.
      const chooser = page.getByRole("main").getByLabel("Dashboards", { exact: true });
      if (await chooser.count()) {
        await chooser.selectOption(name);
      }
      await rowAction(page, name, "Edit");
      const form = page.getByTestId("form-page");
      await expect(form).toBeVisible({ timeout: 30_000 });
      await form.locator("#root_title").fill(`${name} renamed`);
      await proposeFrom(form);
    },
    value: (name) => `${name} renamed`,
  },
  assistant: {
    ask: (name) => `Create a dashboard called ${name}`,
    opened: (page) => page.getByTestId("form-page"),
  },
});

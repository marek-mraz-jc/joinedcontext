/**
 * T-1547 — UI-44, PF-50, DM-33: a Mapping, end to end on dev by a person.
 *
 * Created on a model's Mappings view (a source and a target model, the name the journey gives it,
 * then Propose change), approved, one field changed through the Mappings list's Edit, removed; the
 * assistant opens a form; a viewer finds the list's write controls disabled with a reason.
 *
 * The name is the journey's own (`t1547-…`), never the `{source}-to-{target}` the pair makes, so the
 * project's real mapping of a pair is neither proposed over nor removed by the cleanup. The target
 * is the first model of the project whose required slots the editor fills by itself: the journey
 * proves the page, not a hand-drawn alignment.
 */
import { expect } from "@playwright/test";
import type { Page } from "@playwright/test";
import { PROJECT, kindJourney, openedForm, proposeFrom, rowAction } from "./kindJourney";

/** Picks a target the source maps onto with nothing required left unfilled. */
async function mappableTarget(page: Page): Promise<void> {
  const source = page.getByLabel("Source model");
  const target = page.getByLabel("Target model");
  const propose = page.getByRole("button", { name: "Propose change" });
  const from = await source.inputValue();
  for (const option of await target.locator("option").all()) {
    const value = await option.getAttribute("value");
    if (!value || value === from) {
      continue;
    }
    await target.selectOption(value);
    if ((await propose.getAttribute("aria-disabled")) !== "true") {
      return;
    }
  }
  throw new Error(`no model of ${PROJECT} takes a mapping from ${from} without a slot drawn by hand`);
}

kindJourney({
  task: "t1547",
  kind: "Mapping",
  plural: "mappings",
  page: `/projects/${PROJECT}/mappings`,
  create: async (page, name) => {
    await page.goto(`/projects/${PROJECT}/models?lang=en`, { waitUntil: "load" });
    await page.getByRole("main").getByRole("table").getByRole("link").first().click();
    await page.getByRole("tab", { name: "Mappings" }).click();
    await mappableTarget(page);
    await page.getByLabel("Name", { exact: true }).fill(name);
    const propose = page.getByRole("button", { name: "Propose change" });
    await expect(propose).not.toHaveAttribute("aria-disabled", "true");
    await propose.click();
  },
  change: {
    apply: async (page, name) => {
      await rowAction(page, name, "Edit");
      const form = await openedForm(page);
      await form.getByLabel(/Bloblang/).first().fill(`./artifacts/${name}.blobl`);
      await proposeFrom(form);
    },
    value: (name) => `"bloblang":"./artifacts/${name}.blobl"`,
  },
  assistant: {
    ask: (name) => `Create a Mapping called ${name}`,
    opened: (page) => page.getByTestId("form-page").or(page.getByRole("tab", { name: "Mappings" })),
  },
});

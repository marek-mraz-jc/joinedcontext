/**
 * T-1546 — UI-44, PF-50: a Layer, end to end on dev by a person.
 *
 * Created on the Dashboards page ("New layer": which endpoint it draws, which entity type, a
 * style), approved, restyled through its YAML editor on the Layers list — a layer on no
 * dashboard has no legend to edit it from — removed; the assistant opens the same form; a viewer
 * finds the page's write controls disabled with a reason.
 */
import { expect } from "@playwright/test";
import type { Locator } from "@playwright/test";
import { PROJECT, editAsYaml, kindJourney, pickFirst, proposeFrom, rowAction } from "./kindJourney";

/** The entity types follow the endpoint chosen: a select once they are read, a text box before. */
async function entityType(field: Locator): Promise<void> {
  if ((await field.evaluate((element) => element.tagName)) === "SELECT") {
    await pickFirst(field);
  } else {
    await field.fill("BikeHireDockingStation");
  }
}

kindJourney({
  task: "t1546",
  kind: "Layer",
  plural: "layers",
  page: `/projects/${PROJECT}/dashboards`,
  create: async (page, name) => {
    await page.getByRole("main").getByRole("button", { name: "New layer" }).first().click();
    const form = page.getByTestId("form-page");
    await expect(form).toBeVisible({ timeout: 30_000 });
    await form.locator("#root_name").fill(name);
    await pickFirst(form.locator("#root_sourceEndpointRef"));
    await entityType(form.locator("#root_entityType"));
    await form.locator("#root_style").selectOption("circle");
    await proposeFrom(form);
  },
  change: {
    apply: async (page, name) => {
      await page.goto(`/projects/${PROJECT}/layers?lang=en`, { waitUntil: "load" });
      await editAsYaml(page, "layers", name, () => rowAction(page, name, "Edit"), (spec) => {
        spec.style = "heatmap";
      });
    },
    value: () => '"style":"heatmap"',
  },
  assistant: {
    ask: (name) => `Create a map layer called ${name}`,
    opened: (page) => page.getByTestId("form-page"),
  },
});

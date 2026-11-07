/**
 * T-1540 — UI-44, PF-50: a ContextSourceRegistration, end to end on dev by a person.
 *
 * Created on the Registrations page through its routed form (T-2345: "New registration", Check,
 * Propose change), approved, its claimed entity type changed through the row's Edit, removed; the
 * assistant opens the same form; a viewer finds the page's write controls disabled with a reason.
 */
import type { Locator, Page } from "@playwright/test";
import { expect } from "@playwright/test";
import { PROJECT, kindJourney, pickFirst, proposeFrom, rowAction } from "./kindJourney";

/** A reference field is a select when the project has something to offer, and a text box when not. */
async function reference(field: Locator, fallback: string): Promise<void> {
  if ((await field.evaluate((element) => element.tagName)) === "SELECT") {
    await pickFirst(field);
  } else {
    await field.fill(fallback);
  }
}

async function form(page: Page): Promise<Locator> {
  const routed = page.getByTestId("form-page");
  await expect(routed).toBeVisible({ timeout: 30_000 });
  return routed;
}

kindJourney({
  task: "t1540",
  kind: "ContextSourceRegistration",
  plural: "csrs",
  page: `/projects/${PROJECT}/csrs`,
  create: async (page, name) => {
    await page.getByRole("main").getByRole("button", { name: "New registration" }).first().click();
    const routed = await form(page);
    await routed.locator("#root_name").fill(name);
    await reference(routed.locator("#root_contextSpaceRef"), "citybikes");
    await reference(routed.locator("#root_endpointRef"), "helsinki-bikes");
    await routed.locator("#root_information__add").click();
    await routed.locator("#root_information_0_entities__add").click();
    await routed.locator("#root_information_0_entities_0_type").fill("WeatherObserved");
    await proposeFrom(routed);
  },
  change: {
    apply: async (page, name) => {
      await rowAction(page, name, "Edit");
      const routed = await form(page);
      await routed.locator("#root_information_0_entities_0_type").fill("AirQualityObserved");
      await proposeFrom(routed);
    },
    value: () => "AirQualityObserved",
  },
  assistant: {
    ask: (name) => `Create a context source registration called ${name}`,
    opened: (page) => page.getByTestId("form-page"),
  },
});

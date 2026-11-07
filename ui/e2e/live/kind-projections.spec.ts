/**
 * T-1548 — UI-44, PF-50, MP-01: a ModelProjection, end to end on dev by a person.
 *
 * Created on the Model projections list through its form ("New model projection": a name, the helsinki
 * space, its seeded helsinki model at major 1, the WeatherObserved class with identity only; Check,
 * then Propose change), approved, given a scope filter through the row's Edit, removed; the
 * assistant opens the same form; a viewer finds the page's write controls disabled with a reason.
 *
 * No endpoint references the projection while it stands, so nothing anybody reads changes.
 */
import { PROJECT, kindJourney, openedForm, proposeFrom, rowAction } from "./kindJourney";

kindJourney({
  task: "t1548",
  kind: "ModelProjection",
  plural: "projections",
  page: `/projects/${PROJECT}/projections`,
  create: async (page, name) => {
    await page.getByRole("main").getByRole("button", { name: "New model projection" }).first().click();
    const form = await openedForm(page);
    await form.locator("#root_name").fill(name);
    await form.locator("#root_contextSpaceRef").fill(PROJECT);
    await form.locator("#root_dataModelRef_name").fill(PROJECT);
    await form.locator("#root_classes_0_name").fill("WeatherObserved");
    await proposeFrom(form);
  },
  change: {
    apply: async (page, name) => {
      await rowAction(page, name, "Edit");
      const form = await openedForm(page);
      await form.locator("#root_filter_scopeQ").fill(`/${PROJECT}`);
      await proposeFrom(form);
    },
    value: () => `"scopeQ":"/${PROJECT}"`,
  },
  assistant: {
    ask: (name) => `Create a ModelProjection called ${name}`,
    opened: (page) => page.getByTestId("form-page").or(page.getByRole("dialog", { name: "New model projection" })),
  },
});

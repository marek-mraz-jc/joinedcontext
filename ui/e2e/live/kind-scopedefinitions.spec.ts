/**
 * T-1549 — UI-44, PF-50, R19: a ScopeDefinition, end to end on dev by a person.
 *
 * Created on the ScopeDefinitions list through its form ("New scope definition": a name and a path
 * under /admin that only this journey uses; Check, then Propose change), approved, moved to another
 * path through the row's Edit, removed; the assistant opens the same form; a viewer finds the page's
 * write controls disabled with a reason.
 *
 * The path hangs under a node nothing else names, so no Policy grants by it and no entity carries it
 * while it stands.
 */
import { PROJECT, kindJourney, openedForm, proposeFrom, rowAction } from "./kindJourney";

const path = (name: string): string => `/admin/journeys/${name}`;

kindJourney({
  task: "t1549",
  kind: "ScopeDefinition",
  plural: "scopedefinitions",
  page: `/projects/${PROJECT}/scopedefinitions`,
  create: async (page, name) => {
    await page.getByRole("main").getByRole("button", { name: "New scope definition" }).first().click();
    const form = await openedForm(page);
    await form.locator("#root_name").fill(name);
    await form.locator("#root_scopeString").fill(path(name));
    await proposeFrom(form);
  },
  change: {
    apply: async (page, name) => {
      await rowAction(page, name, "Edit");
      const form = await openedForm(page);
      await form.locator("#root_scopeString").fill(`${path(name)}/moved`);
      await proposeFrom(form);
    },
    // The form derives the parent from the path: the move is visible in both members.
    value: (name) => `"isChildOf":"${path(name)}"`,
  },
  assistant: {
    ask: (name) => `Create a ScopeDefinition called ${name}`,
    opened: (page) => page.getByTestId("form-page").or(page.getByRole("dialog", { name: "New scope definition" })),
  },
});

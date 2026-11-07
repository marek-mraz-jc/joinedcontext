/**
 * T-1550 — UI-44, PF-50, PF-34, PF-36: a ServiceAccount, end to end on dev by a person.
 *
 * Created on Project settings → Service accounts ("New service account": why it exists, who
 * answers for it — the form starts with the steward — a viewer grant on this project, one Keycloak
 * client; Check, then
 * Propose change), approved, its purpose changed through its card's Edit, removed; the assistant
 * opens the same form; a viewer finds the section's write controls disabled with a reason.
 *
 * The form has no field a secret could be typed into (PF-36): the client is declared by name and
 * nothing here reads or prints a credential.
 */
import { PROJECT, kindJourney, openedForm, proposeFrom } from "./kindJourney";

kindJourney({
  task: "t1550",
  kind: "ServiceAccount",
  plural: "serviceaccounts",
  page: `/projects/${PROJECT}/settings/service-accounts`,
  create: async (page, name) => {
    await page.getByRole("main").getByRole("button", { name: "New service account" }).first().click();
    const form = await openedForm(page);
    await form.locator("#root_name").fill(name);
    await form.locator("#root_purpose").fill(`Reads ${PROJECT} for the ${name} journey`);
    await form.locator("#root_roles_0_role").fill("viewer");
    await form.locator("#root_credentials_0_name").fill(`${name}-client`);
    await proposeFrom(form);
  },
  change: {
    apply: async (page, name) => {
      await page.getByRole("main").getByRole("button", { name: `Edit ${name}`, exact: true }).click();
      const form = await openedForm(page);
      await form.locator("#root_purpose").fill(`Reads ${PROJECT} for the ${name} journey, changed`);
      await proposeFrom(form);
    },
    value: (name) => `"purpose":"Reads ${PROJECT} for the ${name} journey, changed"`,
  },
  assistant: {
    ask: (name) => `Create a ServiceAccount called ${name}`,
    opened: (page) => page.getByTestId("form-page").or(page.getByRole("dialog", { name: "New service account" })),
  },
});

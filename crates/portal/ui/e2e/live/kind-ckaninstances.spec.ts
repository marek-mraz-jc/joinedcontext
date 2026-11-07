/**
 * T-1539 — UI-44, PF-50: a CkanInstance, end to end on dev by a person.
 *
 * Proposed on the open-data page's inline form (the dev catalogue and the token Secret the seeded
 * `hel-fi` instance already uses, so nothing new is asked of the cluster), approved, its default
 * organization changed through its YAML editor, removed; the assistant opens the same page; a
 * viewer finds the page's write controls disabled with a reason.
 */
import { PROJECT, editAsYaml, kindJourney } from "./kindJourney";

kindJourney({
  task: "t1539",
  kind: "CkanInstance",
  plural: "ckaninstances",
  page: `/projects/${PROJECT}/ckan`,
  create: async (page, name) => {
    await page.locator("#ckan-instance-name").fill(name);
    await page.locator("#ckan-instance-url").fill("https://opendata.dev.joinedcontext.com");
    await page.locator("#ckan-instance-secret").fill("ckan-api-token");
    await page.getByRole("button", { name: "Propose catalogue" }).click();
  },
  change: {
    apply: async (page, name) => {
      await editAsYaml(
        page,
        "ckaninstances",
        name,
        () => page.getByRole("button", { name: `Edit ${name}` }).click(),
        (spec) => {
          spec.organizationDefault = `${name}-org`;
        },
      );
    },
    value: (name) => `${name}-org`,
  },
  assistant: {
    ask: (name) => `Connect a CKAN catalogue called ${name}`,
    opened: (page) => page.locator("#ckan-instance-name"),
  },
});

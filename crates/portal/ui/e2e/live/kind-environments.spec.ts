/**
 * T-1545 — UI-44, PF-50, CC-73, CC-75: an Environment, end to end on dev by a person.
 *
 * Created on Organization → Environments through its form ("New environment": a name and the
 * organization's domain there; Check, then Propose change) by `demo.steward`, approved by
 * `demo.approver`, its domain changed through the row's Edit, removed. The assistant opens the same
 * form; a viewer finds the tab's write controls disabled with a reason.
 *
 * An installation renders with the environment its `JC_ENVIRONMENT` names; this one has a name of
 * its own that no installation selects, and its domain is on example.org, so dev renders nothing
 * differently while it stands.
 */
import { kindJourney, openedForm, proposeFrom, rowAction } from "./kindJourney";

kindJourney({
  task: "t1545",
  kind: "Environment",
  plural: "environments",
  page: "/organization/environments",
  organization: true,
  create: async (page, name) => {
    await page.getByRole("main").getByRole("button", { name: "New environment" }).first().click();
    const form = await openedForm(page);
    await form.locator("#root_name").fill(name);
    await form.locator("#root_orgDomain").fill(`${name}.example.org`);
    await proposeFrom(form);
  },
  change: {
    apply: async (page, name) => {
      await rowAction(page, name, "Edit");
      const form = await openedForm(page);
      await form.locator("#root_orgDomain").fill(`${name}-changed.example.org`);
      await proposeFrom(form);
    },
    value: (name) => `"orgDomain":"${name}-changed.example.org"`,
  },
  assistant: {
    ask: (name) => `Create an Environment called ${name}`,
    // On the Organization page, where an environment belongs (T-2845), never a project's list.
    opened: (page) =>
      page
        .locator("#organization-tab-environments[aria-selected=true]")
        .locator("xpath=/ancestor::body")
        .locator('[data-testid="form-page"], [role="dialog"]')
        .first(),
  },
});

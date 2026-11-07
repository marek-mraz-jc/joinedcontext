/**
 * T-1537 — UI-44, PF-50, CC-23…CC-27, CC-59: a Blueprint, end to end on dev by a person.
 *
 * Created on Organization → Blueprints through its form ("New blueprint": a name, a version, one
 * role, a parameter schema and one template; Check, then Propose change) by `demo.steward`, who
 * administers the organization, approved by `demo.approver`, who approves across it; its version
 * raised through the row's Edit; removed. The assistant opens the same form; a viewer finds the
 * tab's write controls disabled with a reason.
 *
 * The blueprint is offered to a role nobody holds, so it never shows in anyone's gallery while it
 * stands.
 */
import { kindJourney, openedForm, proposeFrom, rowAction } from "./kindJourney";

kindJourney({
  task: "t1537",
  kind: "Blueprint",
  plural: "blueprints",
  page: "/organization/blueprints",
  organization: true,
  create: async (page, name) => {
    await page.getByRole("main").getByRole("button", { name: "New blueprint" }).first().click();
    const form = await openedForm(page);
    await form.locator("#root_name").fill(name);
    await form.locator("#root_version").fill("1.0.0");
    await form.locator("#root_allowedRoles_0").fill(`${name}-nobody`);
    await form.locator("#root_parameterSchema").fill('{"type": "object", "properties": {"city": {"type": "string"}}}');
    await form.locator("#root_templates_0_name").fill(`${name}-source`);
    await form.locator("#root_templates_0_template").fill("kind: DataSource\nmetadata:\n  name: '{{ city }}'\n");
    await proposeFrom(form);
  },
  change: {
    apply: async (page, name) => {
      await rowAction(page, name, "Edit");
      const form = await openedForm(page);
      await form.locator("#root_version").fill("1.0.1");
      await proposeFrom(form);
    },
    value: () => `"version":"1.0.1"`,
  },
  assistant: {
    ask: (name) => `Create a Blueprint called ${name}`,
    // On the Organization page, where a blueprint belongs, never a project's list of the same name.
    opened: (page) =>
      page
        .locator("#organization-tab-blueprints[aria-selected=true]")
        .locator("xpath=/ancestor::body")
        .locator('[data-testid="form-page"], [role="dialog"]')
        .first(),
  },
});

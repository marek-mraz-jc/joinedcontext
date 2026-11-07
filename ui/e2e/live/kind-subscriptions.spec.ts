/**
 * T-1552 — UI-44, PF-50: a Subscription, end to end on dev by a person.
 *
 * Created on the Subscriptions page through its routed form ("New Subscription": a space, one
 * entity type watched, where the notification goes; Check, then Propose change), approved, its
 * description changed through the row's Edit, removed; the assistant opens the same form; a viewer
 * finds the page's write controls disabled with a reason.
 *
 * The watched type is one no source of dev writes, so the broker never notifies the receiver while
 * the subscription stands, and the receiver is on example.org (RFC 2606), which nobody runs.
 */
import { PROJECT, kindJourney, openedForm, proposeFrom, reference, rowAction } from "./kindJourney";

kindJourney({
  task: "t1552",
  kind: "Subscription",
  plural: "subscriptions",
  page: `/projects/${PROJECT}/subscriptions`,
  create: async (page, name) => {
    await page.getByRole("main").getByRole("button", { name: "New Subscription" }).first().click();
    const form = await openedForm(page);
    await form.locator("#root_name").fill(name);
    await reference(form.locator("#root_contextSpaceRef"), "citybikes");
    await form.locator("#root_entities__add").click();
    await form.locator("#root_entities_0_type").fill("JourneyProbe");
    await form.locator("#root_notification_endpoint_uri").fill(`https://example.org/${name}/notify`);
    await proposeFrom(form);
  },
  change: {
    apply: async (page, name) => {
      await rowAction(page, name, "Edit");
      const form = await openedForm(page);
      await form.locator("#root_description").fill(`Watched by the ${name} journey`);
      await proposeFrom(form);
    },
    value: (name) => `"description":"Watched by the ${name} journey"`,
  },
  assistant: {
    ask: (name) => `Create a Subscription called ${name}`,
    opened: (page) => page.getByTestId("form-page"),
  },
});

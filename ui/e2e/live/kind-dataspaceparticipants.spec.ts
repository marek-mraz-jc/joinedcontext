/**
 * T-1544 — UI-44, PF-50, DS-04, DS-07: the organization's DataSpaceParticipant, end to end on dev.
 *
 * Created on Organization → Data space through its form ("New data space membership": a name, a
 * did:web identifier, the credential issuer and the connector address; Check, then Propose change) by
 * `demo.steward`, approved by `demo.approver`, its engine changed through the row's Edit, removed. The
 * assistant opens the same form; a viewer finds the tab's write controls disabled with a reason.
 *
 * An organization has one participant, so this journey needs a dev with none (the seed has none) and
 * leaves none. Every address is on example.org (RFC 2606) and dev runs no connector, so nobody
 * negotiates with it while it stands.
 */
import { kindJourney, openedForm, proposeFrom, rowAction } from "./kindJourney";

kindJourney({
  task: "t1544",
  kind: "DataSpaceParticipant",
  plural: "dataspaceparticipants",
  page: "/organization/dataspaceparticipants",
  organization: true,
  create: async (page, name) => {
    await page.getByRole("main").getByRole("button", { name: "New data space membership" }).first().click();
    const form = await openedForm(page);
    await form.locator("#root_name").fill(name);
    await form.locator("#root_did").fill("did:web:participant.example.org");
    await form.locator("#root_credentialIssuer").fill("https://issuer.example.org");
    await form.locator("#root_connectorUrl").fill("https://connector.example.org");
    await proposeFrom(form);
  },
  change: {
    apply: async (page, name) => {
      await rowAction(page, name, "Edit");
      const form = await openedForm(page);
      await form.locator("#root_engine").selectOption({ label: "Eclipse Dataspace Components" });
      await proposeFrom(form);
    },
    value: () => `"engine":"edc"`,
  },
  assistant: {
    ask: (name) => `Create a DataSpaceParticipant called ${name}`,
    // On the Organization page, where the participant belongs (T-2845), never a project's list.
    opened: (page) =>
      page
        .locator("#organization-tab-dataspaceparticipants[aria-selected=true]")
        .locator("xpath=/ancestor::body")
        .locator('[data-testid="form-page"], [role="dialog"]')
        .first(),
  },
});

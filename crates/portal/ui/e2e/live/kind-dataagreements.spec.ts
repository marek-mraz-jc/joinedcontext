/**
 * T-1542 — UI-44, PF-50, DS-09: a DataAgreement, end to end on dev by a person.
 *
 * Created on the DataAgreements list through its routed form ("New data agreement": a name, the other
 * participant, the agreement identifier; consumer and requested by default; Check, then Propose
 * change), approved, its state moved to offered through the row's Edit, removed; the assistant opens
 * the same form; a viewer finds the page's write controls disabled with a reason.
 *
 * The agreement is never finalized, so the gateway serves nothing under it while it stands, and the
 * other participant is on example.org (RFC 2606), which nobody runs.
 */
import { PROJECT, kindJourney, openedForm, proposeFrom, rowAction } from "./kindJourney";

kindJourney({
  task: "t1542",
  kind: "DataAgreement",
  plural: "dataagreements",
  page: `/projects/${PROJECT}/dataagreements`,
  create: async (page, name) => {
    await page.getByRole("main").getByRole("button", { name: "New data agreement" }).first().click();
    const form = await openedForm(page);
    await form.locator("#root_name").fill(name);
    await form.locator("#root_remoteParticipant").fill("did:web:partner.example.org");
    await form.locator("#root_agreementId").fill(`urn:uuid:${name}`);
    await proposeFrom(form);
  },
  change: {
    apply: async (page, name) => {
      await rowAction(page, name, "Edit");
      const form = await openedForm(page);
      await form.locator("#root_state").selectOption({ label: "Offered" });
      await proposeFrom(form);
    },
    value: () => `"state":"offered"`,
  },
  assistant: {
    ask: (name) => `Create a DataAgreement called ${name}`,
    opened: (page) => page.getByTestId("form-page"),
  },
});

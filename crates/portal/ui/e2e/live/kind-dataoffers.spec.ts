/**
 * T-1543 — UI-44, PF-50, DS-07, DS-14: a DataOffer, end to end on dev by a person.
 *
 * Created on the DataOffers list through its form ("New data offer": a name, the helsinki space, the
 * seeded weather endpoint; the open offer the form fills; Check, then Propose change), approved,
 * given a purpose through the row's Edit, removed; the assistant opens the same form; a viewer finds
 * the page's write controls disabled with a reason.
 *
 * dev runs no data space connector, so the offer is published nowhere while it stands.
 */
import { PROJECT, kindJourney, openedForm, proposeFrom, rowAction } from "./kindJourney";

const PURPOSE = "https://w3id.org/dpv#ServiceProvision";

kindJourney({
  task: "t1543",
  kind: "DataOffer",
  plural: "dataoffers",
  page: `/projects/${PROJECT}/dataoffers`,
  create: async (page, name) => {
    await page.getByRole("main").getByRole("button", { name: "New data offer" }).first().click();
    const form = await openedForm(page);
    await form.locator("#root_name").fill(name);
    await form.locator("#root_contextSpaceRef").fill(PROJECT);
    await form.locator("#root_endpointRefs_0").fill(`${PROJECT}-weather`);
    await proposeFrom(form);
  },
  change: {
    apply: async (page, name) => {
      await rowAction(page, name, "Edit");
      const form = await openedForm(page);
      await form.locator("#root_purpose").fill(PURPOSE);
      await proposeFrom(form);
    },
    value: () => `"purpose":"${PURPOSE}"`,
  },
  assistant: {
    ask: (name) => `Create a DataOffer called ${name}`,
    opened: (page) => page.getByTestId("form-page").or(page.getByRole("dialog", { name: "New data offer" })),
  },
});

/**
 * T-1553 — UI-44, PF-50, MF-27, MF-28: a SyncSource, end to end on dev by a person.
 *
 * Created on the Sync sources page ("Add source", a Git origin, every 6h; Check, then Propose the
 * source), approved, its interval changed through its YAML editor — the card's menu opens no form
 * for a source — removed; the assistant opens the same form; a viewer finds Add source, Sync now,
 * Pause and Detach disabled with the verb they lack (T-2569).
 *
 * The origin is on example.org (RFC 2606): the loop's first run fails to reach it and imports
 * nothing, so a source that stands for a few minutes never opens a merge request on dev.
 */
import { PROJECT, editAsYaml, kindJourney, openedForm, proposeFrom, rowAction } from "./kindJourney";

kindJourney({
  task: "t1553",
  kind: "SyncSource",
  plural: "syncsources",
  page: `/projects/${PROJECT}/syncsources`,
  create: async (page, name) => {
    await page.getByLabel("Origin").selectOption("git");
    await page.getByRole("main").getByRole("button", { name: "Add source" }).click();
    const form = await openedForm(page);
    await form.locator("#root_name").fill(name);
    await form.locator("#root_git_url").fill(`https://git.example.org/${name}/config.git`);
    await form.locator("#root_git_ref").fill("main");
    await proposeFrom(form);
  },
  change: {
    apply: async (page, name) => {
      await editAsYaml(page, "syncsources", name, () => rowAction(page, name, "Edit"), (spec) => {
        spec.schedule = { interval: "12h" };
      });
    },
    value: () => '"interval":"12h"',
  },
  assistant: {
    ask: (name) => `Create a SyncSource called ${name}`,
    opened: (page) => page.getByTestId("form-page").or(page.getByRole("dialog", { name: "New sync source" })),
  },
  writeControls: /^(Add source|Sync now|Pause|Resume|Detach)$/,
});

/**
 * T-1536 — UI-44, PF-50, AG-26, AG-47, AG-70: an AgentProfile, end to end on dev by a person.
 *
 * Created on Organization → Agent profiles through its form ("New agent profile": a name, the runtime
 * image and its digest, the model and its token budget; a steward profile with the reference limits
 * the form fills; Check, then Propose change) by `demo.steward`, who administers the organization,
 * approved by `demo.approver`, who approves across it; its step limit changed through the row's
 * Edit; removed. The assistant opens the same form; a viewer finds the tab's write controls disabled
 * with a reason.
 *
 * The profile has a name of its own that no run selects (a run loads its profile by name), it is a
 * steward profile that reaches no host, and it grants no access block, so the live assistant's reach
 * does not change while it stands. No key is typed anywhere: the proxy holds those.
 */
import { kindJourney, openedForm, proposeFrom, rowAction } from "./kindJourney";

const DIGEST = `sha256:${"0".repeat(64)}`;

kindJourney({
  task: "t1536",
  kind: "AgentProfile",
  plural: "agentprofiles",
  page: "/organization/agentprofiles",
  organization: true,
  create: async (page, name) => {
    await page.getByRole("main").getByRole("button", { name: "New agent profile" }).first().click();
    const form = await openedForm(page);
    await form.locator("#root_name").fill(name);
    await form.locator("#root_runtime_image").fill("registry.example.org/agents/runner");
    await form.locator("#root_runtime_digest").fill(DIGEST);
    await form.locator("#root_model_name").fill("example-model-1");
    await form.locator("#root_model_maxTokensPerRun").fill("1000");
    await proposeFrom(form);
  },
  change: {
    apply: async (page, name) => {
      await rowAction(page, name, "Edit");
      const form = await openedForm(page);
      await form.locator("#root_limits_stepsPerRun").fill("10");
      await proposeFrom(form);
    },
    value: () => `"stepsPerRun":10`,
  },
  assistant: {
    ask: (name) => `Create an AgentProfile called ${name}`,
    // On the Organization page, where a profile belongs (T-2845), never a project's list.
    opened: (page) =>
      page
        .locator("#organization-tab-agentprofiles[aria-selected=true]")
        .locator("xpath=/ancestor::body")
        .locator('[data-testid="form-page"], [role="dialog"]')
        .first(),
  },
});

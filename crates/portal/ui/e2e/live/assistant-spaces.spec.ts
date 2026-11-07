/**
 * The assistant answers from the data of each project's own space on dev (T-3044): the steward
 * asks one question in praha, helsinki, banskabystrica and bbsk; the answer arrives finished, no
 * alert stands on the page, and the assistant read at least one endpoint of the space. The answer
 * and the steps go into the report as the run's evidence.
 *
 * Four model calls, one per project: every title names the assistant, so the hourly sweep leaves
 * it to the nightly eval batch (`sweep.sh`, T-2733).
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { STEWARD, ask, signIn } from "./portal";

const PROJECTS = ["praha", "helsinki", "banskabystrica", "bbsk"];
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");

interface Step {
  tool?: string;
  status?: string;
  error?: string;
}

/** The tool steps of the run the dock follows, replayed from its event stream. */
async function toolSteps(page: Page, project: string): Promise<Step[]> {
  return page.evaluate(async (project) => {
    const raw = window.sessionStorage.getItem("jc.assistant.run");
    const run = raw === null ? null : (JSON.parse(raw) as { runId?: string } | string | null);
    const id = typeof run === "object" && run !== null ? run.runId : typeof run === "string" ? run : null;
    if (typeof id !== "string") {
      return [];
    }
    return new Promise<Step[]>((resolve) => {
      const seen: Step[] = [];
      const source = new EventSource(`/api/v1/projects/${project}/agent-runs/${id}/events`, { withCredentials: true });
      source.addEventListener("tool", (message) => {
        seen.push(JSON.parse((message as MessageEvent<string>).data) as Step);
      });
      setTimeout(() => {
        source.close();
        resolve(seen);
      }, 2000);
    });
  }, project);
}

test.setTimeout(900_000);

for (const project of PROJECTS) {
  test(`the assistant answers from the ${project} space's own data`, async ({ browser }) => {
    const { page, context } = await signIn(browser, STEWARD, `/projects/${project}/assistant?lang=en`);
    try {
      await expect(page.getByRole("heading", { level: 1, name: "Assistant" })).toBeVisible({ timeout: 60_000 });
      await ask(
        page,
        `Read one endpoint of ${project} and name two of its entities with one value each. (journey ${SUFFIX})`,
      );
      const answer = page
        .getByRole("list", { name: "Conversation" })
        .getByRole("listitem")
        .filter({ hasText: /^Assistant/ })
        .last();
      await expect(answer).toBeVisible({ timeout: 300_000 });
      await expect(answer).not.toContainText(/cannot|not allowed|forbidden|does not grant|The answer failed/i);
      await expect(page.getByRole("alert")).toHaveCount(0);

      const steps = await toolSteps(page, project);
      test.info().annotations.push(
        { type: "answer", description: (await answer.innerText()).slice(0, 600) },
        { type: "steps", description: JSON.stringify(steps.map((step) => [step.tool, step.status])) },
      );
      // A call the model shapes wrong is answered with the reason and made again (the prompt says
      // so); what has to hold is that it then read the space. The failed steps stay in the report.
      expect(
        steps.some((step) => step.tool === "query_endpoint" && step.status === "ok"),
        "the assistant read an endpoint of the space",
      ).toBe(true);
    } finally {
      await context.close();
    }
  });
}

/**
 * What the docked assistant can answer about the project it sits in, on dev (T-1475, T-1603;
 * AG-64, AG-70).
 *
 * The dock runs the `app-builder` profile, and a run may only call what that profile names
 * (`joinedcontext-deployment/components/agent-runner/seed/app-builder.yaml`). Before T-1475 the
 * profile named eleven operations, so "what waits for approval" and "is the pipeline running" had
 * nothing behind them and the assistant said it could not answer. This is those tasks' own
 * measurement, kept: the questions are asked as a person asks them, and the proof is the operation
 * line in the conversation — the platform answered from `jc_change_list` and `jc_pipeline_metrics`,
 * not from the model's memory of the page.
 *
 * Both questions run in one conversation and one sign-in: a second OIDC login inside a minute meets
 * the edge's rate limit (429, T-2237).
 *
 * Nothing is proposed, approved or changed here, so the journey may run against dev at any time.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { STEWARD, ask, signIn } from "./portal";

const PROJECT = "helsinki";

/** One question a person types, and the operation that must answer it. */
const ASKED: { question: string; operation: string }[] = [
  { question: `What waits for approval in ${PROJECT}?`, operation: "jc_change_list" },
  { question: "Is the citybikes-gbfs pipeline running?", operation: "jc_pipeline_metrics" },
];

test.setTimeout(1_800_000);

/** The conversation line of one operation, with the icon that says how it ended. */
function toolLine(page: Page, operation: string) {
  return page.getByRole("listitem").filter({ hasText: operation }).first();
}

test("the assistant answers about changes and pipelines from the platform's own operations", async ({
  browser,
}) => {
  const { page, context } = await signIn(browser, STEWARD, `/projects/${PROJECT}/activity?lang=en`);
  try {
    for (const { question, operation } of ASKED) {
      await ask(page, question);

      const line = toolLine(page, operation);
      await expect(line, `"${question}" is answered from ${operation}`).toBeVisible({
        timeout: 300_000,
      });
      await expect(
        line.getByRole("img", { name: "Succeeded" }),
        "the operation the profile names is granted at call time too (AG-70)",
      ).toBeVisible();

      // And the person gets an answer in words, not a refusal and not a failed model call.
      const answer = page.getByRole("listitem").filter({ hasText: "Assistant" }).last();
      await expect(answer).toBeVisible({ timeout: 300_000 });
      await expect(answer).not.toContainText(
        /cannot|not allowed|forbidden|does not grant|The answer failed/i,
      );
    }
  } finally {
    await context.close();
  }
});

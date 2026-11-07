/**
 * A steward runs a queued pipeline once now on dev (T-4349, PL-78). The journey takes the first
 * helsinki pipeline the queue runs (its status carries `Scheduled`), opens its runs from the row,
 * presses Run now, is told which run was queued, and waits until the run list shows that run: the
 * run went through the queue and the runner and wrote. Running a pipeline is its ordinary work, so
 * nothing is left to clean up.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

const PROJECT = "helsinki";

interface Pipeline {
  metadata: { name: string };
  status?: { conditions?: { type: string; reason?: string }[] };
}

test("a steward runs a queued pipeline now and the run appears in its runs", async ({ browser }) => {
  test.setTimeout(20 * 60_000);
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/pipelines?lang=en`);
  try {
    const listed = (await (await page.request.get(`/api/v1/projects/${PROJECT}/pipelines`)).json()) as {
      items: Pipeline[];
    };
    // An idle one, so the run is not refused as running.
    const queued = listed.items.find((one) =>
      one.status?.conditions?.some((c) => c.type === "Scheduled" && c.reason === "Waiting"),
    );
    expect(queued, "dev holds a pipeline the queue runs (PL-78)").toBeDefined();
    const name = queued!.metadata.name;

    await page.getByRole("button", { name: `More actions for ${name}` }).click();
    await page.getByRole("menuitem", { name: "Runs and log" }).click();
    const dialog = page.getByRole("dialog", { name: `Runs of ${name}` });
    const answer = page.waitForResponse(
      (response) =>
        response.request().method() === "POST" && response.url().endsWith(`/pipelines/${name}/runs`),
    );
    await dialog.getByRole("button", { name: "Run now" }).click();
    const started = await answer;
    expect(started.status(), await started.text()).toBe(202);
    const { run } = (await started.json()) as { run: string };
    await expect(dialog.getByText(/is queued\. It starts when its turn comes/)).toBeVisible();

    // The queue starts it in its lane's turn; the run list names it once it wrote.
    await expect
      .poll(
        async () => {
          const runs = (await (await page.request.get(`/api/v1/projects/${PROJECT}/pipelines/${name}/runs`)).json()) as {
            items: { run: string }[];
          };
          return runs.items.some((one) => one.run === run);
        },
        { timeout: 18 * 60_000, intervals: [10_000] },
      )
      .toBe(true);
  } finally {
    await context.close();
  }
});

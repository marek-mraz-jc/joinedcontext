/**
 * An approver answers a snapshot pipeline's source deletes on dev (T-4891, PL-97). The journey
 * takes the first helsinki pipeline whose `spec.sourceDeletes.mode` is `snapshot`, reads the dry
 * run (ledger size, ids one more complete run would delete, any held run) and answers `keep`, which
 * deletes nothing: it clears the misses, or is refused with 409 when nothing is due. Nothing is
 * left to clean up.
 */
import { expect, test } from "@playwright/test";
import { APPROVER, csrf, signIn } from "./portal";

const PROJECT = "helsinki";

interface Pipeline {
  metadata: { name: string };
  spec?: { sourceDeletes?: { mode?: string } };
}

test("an approver reads a snapshot pipeline's source deletes and keeps the entities", async ({ browser }) => {
  test.setTimeout(5 * 60_000);
  const { context, page } = await signIn(browser, APPROVER, `/projects/${PROJECT}/pipelines?lang=en`);
  try {
    const listed = (await (await page.request.get(`/api/v1/projects/${PROJECT}/pipelines`)).json()) as {
      items: Pipeline[];
    };
    const snapshot = listed.items.find((one) => one.spec?.sourceDeletes?.mode === "snapshot");
    expect(snapshot, "dev holds a snapshot pipeline (PL-97)").toBeDefined();
    const path = `/api/v1/projects/${PROJECT}/pipelines/${snapshot!.metadata.name}/source-deletes`;

    const preview = await page.request.get(path);
    expect(preview.status(), await preview.text()).toBe(200);

    const kept = await page.request.post(path, {
      headers: { "x-csrf-token": await csrf(context), "content-type": "application/json" },
      data: { decision: "keep" },
    });
    expect([202, 409], await kept.text()).toContain(kept.status());
    if (kept.status() === 202) expect(((await kept.json()) as { deleting: number }).deleting).toBe(0);
  } finally {
    await context.close();
  }
});

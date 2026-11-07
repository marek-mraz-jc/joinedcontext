/**
 * The node's two preview slots, played on dev (T-1593; PF-83, CC-78, ADR-N-024 §10).
 *
 * `tests/workspace_preview_tests.rs::one_preview_per_workspace_and_two_on_the_node` proves the
 * refusal against a stub. What no test proved is that a person meets it on dev: that the third
 * start is refused, that the refusal **names the two copies holding the slots** so they know which
 * one to stop, and that stopping one frees a slot for real.
 *
 * Three copies are opened and discarded again, so dev keeps what it had. A preview on this node is
 * a slug minted inside the shared gateway and not a pod, so this costs the node nothing to speak of.
 */
import { expect, test } from "@playwright/test";
import type { APIResponse, Page } from "@playwright/test";
import { STEWARD, csrf, signIn } from "./portal";

const PROJECT = "helsinki";
/** The node takes two (`MAX_ON_NODE`), so three copies are what it takes to be refused. */
const COPIES = ["t1593-first", "t1593-second", "t1593-third"];

test.setTimeout(900_000);

async function open(page: Page, token: string, name: string): Promise<APIResponse> {
  return page.request.post(`/api/v1/projects/${PROJECT}/workspaces`, {
    headers: { "x-csrf-token": token },
    data: { name, title: "the node's preview slots", ttlDays: 1, scope: { kind: "project" } },
  });
}

async function startPreview(page: Page, token: string, name: string): Promise<APIResponse> {
  return page.request.post(`/api/v1/projects/${PROJECT}/workspaces/${name}/preview`, {
    headers: { "x-csrf-token": token },
  });
}

/** The copy's preview state, as its own route answers it. */
async function previewState(page: Page, name: string): Promise<string> {
  const answer = await page.request.get(`/api/v1/projects/${PROJECT}/workspaces/${name}`);
  if (!answer.ok()) {
    return "gone";
  }
  return String((await answer.json()).previewState ?? "");
}

test("the third preview on the node is refused, and the refusal names the two that hold the slots", async ({
  browser,
}) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/workspaces?lang=en`);
  const token = await csrf(context);
  try {
    // A slot another journey left running would make this test about that journey instead.
    const listed = await page.request.get(`/api/v1/projects/${PROJECT}/workspaces`);
    const already = ((await listed.json()).items ?? []).filter(
      (copy: { previewState: string }) => copy.previewState !== "none",
    );
    expect(already, "a preview is already running on the node: run the residue sweep first").toEqual(
      [],
    );

    for (const name of COPIES) {
      const answer = await open(page, token, name);
      expect([201, 409], `open ${name}: ${await answer.text()}`).toContain(answer.status());
    }

    for (const name of COPIES.slice(0, 2)) {
      const answer = await startPreview(page, token, name);
      expect(answer.ok(), `start ${name}: ${await answer.text()}`).toBe(true);
      await expect
        .poll(() => previewState(page, name), { timeout: 300_000, intervals: [5_000] })
        .toMatch(/running|starting/i);
    }

    const third = await startPreview(page, token, COPIES[2]);
    const said = await third.text();
    expect(third.status(), said).toBe(409);
    // The names are the point: "stop one first" without saying which is advice a person cannot act on.
    expect(said, "the refusal names the copies holding the slots").toContain(COPIES[0]);
    expect(said, "the refusal names the copies holding the slots").toContain(COPIES[1]);
    expect(said).toMatch(/stop one/i);
    // A refused start leaves the copy alone: it is not left Starting for ever.
    expect(await previewState(page, COPIES[2])).toBe("none");

    // Stopping one frees the slot, so the limit is a limit and not a ceiling nobody comes back from.
    const stop = await page.request.delete(
      `/api/v1/projects/${PROJECT}/workspaces/${COPIES[0]}/preview`,
      { headers: { "x-csrf-token": token } },
    );
    expect(stop.ok(), `stop ${COPIES[0]}: ${await stop.text()}`).toBe(true);
    // `stopped`, not `none`: a copy that has run keeps the fact, and the slot is free either way.
    await expect
      .poll(() => previewState(page, COPIES[0]), { timeout: 120_000, intervals: [5_000] })
      .toBe("stopped");

    const retry = await startPreview(page, token, COPIES[2]);
    expect(retry.ok(), `the freed slot takes the third: ${await retry.text()}`).toBe(true);
  } finally {
    for (const name of COPIES) {
      await page.request
        .delete(`/api/v1/projects/${PROJECT}/workspaces/${name}/preview`, {
          headers: { "x-csrf-token": token },
        })
        .catch(() => undefined);
      await page.request
        .delete(`/api/v1/projects/${PROJECT}/workspaces/${name}`, {
          headers: { "x-csrf-token": token },
        })
        .catch(() => undefined);
    }
    await context.close();
  }
});

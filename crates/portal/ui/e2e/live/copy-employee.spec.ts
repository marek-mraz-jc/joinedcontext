/**
 * Scenario 2 on dev (T-1263; CC-76, CC-79, CC-80): an employee works on a copy of the project and
 * brings it back as one Change a person approves.
 *
 * The whole point of a copy is that nothing in it is live until it comes back, so the journey
 * proves both halves: the edit lands in the copy and not in the project, and the project only
 * changes when an approver merges the one Change the copy became. The change is rejected at the
 * end, so dev keeps what it had.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { APPROVER, STEWARD, csrf, reject, signIn } from "./portal";

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const COPY = `emp-${SUFFIX}`;
/** The pipeline the copy edits: pausing it is one field of one file, and one click in the UI. */
const PIPELINE = process.env.E2E_PIPELINE ?? "hel-news";

test.setTimeout(600_000);

/**
 * Whether the pipeline runs, as the project (or a copy of it) holds it now. An absent
 * `spec.enabled` runs: a pipeline is paused only by an explicit `false`, which is what the
 * pipelines page reads too, and every pipeline on dev leaves the field out.
 */
async function running(page: Page, workspace?: string): Promise<boolean> {
  const query = workspace ? `?workspace=${workspace}` : "";
  const answer = await page.request.get(`/api/v1/projects/${PROJECT}/pipelines/${PIPELINE}${query}`);
  expect(answer.ok(), `reading ${PIPELINE}${query}: ${answer.status()}`).toBe(true);
  const manifest = (await answer.json()) as { spec?: { enabled?: boolean } };
  return manifest.spec?.enabled !== false;
}

test("an employee copies the project, edits in the copy, and brings it back as one change", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/workspaces?lang=en`);
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  const page = steward.page;
  const before = await running(page);

  try {
    // 1. Work on a copy of the whole project.
    await page.getByRole("button", { name: "Work on a copy" }).first().click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel(/^Name/).fill(COPY);
    await dialog.getByRole("button", { name: "Start the copy" }).click();
    await expect(dialog).toBeHidden({ timeout: 60_000 });
    await expect(page.getByRole("cell", { name: COPY })).toBeVisible({ timeout: 30_000 });

    // 2. Inside the copy, the bar says so: a person editing has to be able to see that nothing
    //    they do here is live (CC-77).
    await page.goto(`/projects/${PROJECT}/pipelines?workspace=${COPY}&lang=en`, { waitUntil: "load" });
    const bar = page.getByRole("region", { name: "Copy" });
    await expect(bar).toContainText("You are working on the copy", { timeout: 30_000 });
    await expect(bar).toContainText("No changes yet");

    // 3. The edit: one field of one pipeline, in the copy. Inside a copy the same button commits
    //    to the copy's branch and opens no Change (CC-76) — which is what step 4 reads back.
    const row = page.getByRole("row").filter({ hasText: PIPELINE });
    await expect(row).toHaveCount(1, { timeout: 30_000 });
    // The one button says which way it goes: `Pause` on a pipeline that runs, `Resume` on one that
    // does not. A pipeline whose manifest leaves `enabled` out runs, so the text is read rather
    // than assumed.
    const toggle = row.getByRole("button", { name: /^(Pause|Resume)$/ });
    const wasRunning = (await toggle.textContent())?.trim() === "Pause";
    expect(wasRunning, "the button offers the wrong way for what the copy holds").toBe(
      await running(page, COPY),
    );
    await toggle.click();

    // 4. The edit is in the copy and the project is untouched — the property the whole feature
    //    exists for.
    await expect
      .poll(() => running(page, COPY), { timeout: 120_000, intervals: [3_000] })
      .toBe(!wasRunning);
    expect(await running(page), "the project changed before anyone approved anything").toBe(before);

    // 5. Compare says what the copy changes, in the copy's own words: one file changed, its kind,
    //    and the field the edit touched (CC-79).
    await page.goto(`/projects/${PROJECT}/workspaces/${COPY}/compare?lang=en`, { waitUntil: "load" });
    const compare = page.getByRole("region", { name: "What the copy changes" });
    await expect(compare.getByRole("heading", { name: "What the copy changes" })).toBeVisible({
      timeout: 30_000,
    });
    await expect(compare).toContainText("1 changed");
    await expect(compare.getByRole("heading", { name: "Pipeline" })).toBeVisible();
    // The fields are folded into a `details`; opened, they name what changed rather than only that
    // something did.
    const changed = compare.getByRole("listitem").filter({ hasText: "Pipeline" }).first();
    await changed.getByText("Fields", { exact: true }).click();
    await expect(changed).toContainText("spec.enabled");

    // 6. Bring it back: one Change, in the lane of its riskiest file, which an approver decides.
    await page.goto(`/projects/${PROJECT}/workspaces/${COPY}/bring-back?lang=en`, { waitUntil: "load" });
    await page.getByRole("button", { name: "Propose as one change" }).click();
    await expect(page.getByText(/Proposed as/)).toBeVisible({ timeout: 120_000 });
    // The page links the change it opened; the id is what the approvals route takes.
    const link = page.getByRole("link", { name: /^chg-|^[a-z0-9-]+$/ }).last();
    const change = ((await link.getAttribute("href")) ?? "").split("/approvals/")[1]?.split(/[?#]/)[0] ?? "";
    expect(change, "the bring back names no change").not.toBe("");

    // 7. The project still has its own value until the approver decides, and the change carries
    //    the copy it came from (CC-79).
    expect(await running(page)).toBe(before);
    const detail = await page.request.get(`/api/v1/projects/${PROJECT}/changes/${change}`);
    expect(detail.ok()).toBe(true);
    expect(JSON.stringify(await detail.json())).toContain(COPY);

    await reject(approver.page, PROJECT, change);
    expect(await running(page), "a rejected change reached the project").toBe(before);
  } finally {
    const token = await csrf(page.context());
    await page.request.delete(`/api/v1/projects/${PROJECT}/workspaces/${COPY}`, {
      headers: { "x-csrf-token": token },
    });
  }
});

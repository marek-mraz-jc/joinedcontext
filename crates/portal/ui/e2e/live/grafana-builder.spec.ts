import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";

import { APPROVER, EDITOR, STEWARD, approve, csrf, proposedChange, signIn } from "./portal";

// The dashboard builder on dev: the steward builds a dashboard from choices, previews a panel as
// themselves and proposes it; the approver accepts it; the reconciler provisions it into the
// project's daiya and the dashboard page reads it back; an edit and the removal go the same way.
// A Portal without the builder (an older pin) or without the project's daiya stops the walk where
// the page says so; a person without Propose on Dashboard meets the read-only page.

const PROJECT = "helsinki";
const TITLE = `t-builder ${Date.now()}`;

type Signed = { context: BrowserContext; page: Page };
let steward: Signed | undefined;
let approver: Signed | undefined;
const opened: string[] = [];

/** Whether the deployed Portal serves the builder at all (dev may still run an older pin). */
async function served(page: Page): Promise<boolean> {
  const answer = await page.request.get(`/api/v1/projects/${PROJECT}/grafana/builder`);
  return answer.status() !== 404;
}

test.afterAll(async () => {
  // A change this journey left open is rejected, so dev keeps neither a half-built dashboard nor
  // an open proposal of one.
  if (approver) {
    const token = await csrf(approver.context);
    for (const change of opened) {
      const state = await approver.page.request.get(`/api/v1/projects/${PROJECT}/changes/${change}`);
      const phase = state.ok() ? ((await state.json()) as { status?: { phase?: string } }).status?.phase : undefined;
      if (phase !== "PendingApproval") continue;
      await approver.page.request.post(`/api/v1/projects/${PROJECT}/changes/${change}/reject`, {
        headers: { "x-csrf-token": token, "content-type": "application/json" },
        data: { reason: "Made by the live journey grafana-builder, which cleans up after itself." },
      });
    }
  }
  await steward?.context.close();
  await approver?.context.close();
});

test("a person without the right meets the builder read-only", async ({ browser }) => {
  test.setTimeout(120_000);
  const editor = await signIn(browser, EDITOR, `/projects/${PROJECT}/grafana?lang=en`);
  try {
    if (!(await served(editor.page))) return;
    await editor.page.goto(`/projects/${PROJECT}/grafana/new?lang=en`);
    const readOnly = editor.page.getByText("You cannot build dashboards in this project");
    const form = editor.page.getByRole("button", { name: "Propose dashboard" });
    await expect(readOnly.or(form)).toBeVisible({ timeout: 30_000 });
    // demo.editor holds no Dashboard verb in helsinki (KNOWLEDGE T-6516): the page offers nothing.
    if (await readOnly.isVisible()) await expect(editor.page.getByRole("button", { name: "Add panel" })).toHaveCount(0);
  } finally {
    await editor.context.close();
  }
});

test("a person builds a dashboard from choices, previews it, and it opens once approved, then edits and removes it", async ({ browser }) => {
  test.setTimeout(600_000);
  steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/grafana?lang=en`);
  const page = steward.page;
  if (!(await served(page))) return;
  await page.goto(`/projects/${PROJECT}/grafana/new?lang=en`);
  const save = page.getByRole("button", { name: "Propose dashboard" });
  const readOnly = page.getByText("You cannot build dashboards in this project");
  await expect(save.or(readOnly)).toBeVisible({ timeout: 30_000 });
  if (await readOnly.isVisible()) return;

  await page.locator("#builder-title").fill(TITLE);
  await page.getByRole("button", { name: "Add panel" }).click();
  const form = page.locator("section").filter({ has: page.getByRole("heading", { level: 2, name: /^Panel: / }) });
  for (const field of ["endpoint", "type", "attribute"]) {
    const select = form.locator(`#builder-panel-1-${field}`);
    await expect(select.locator("option").nth(1)).toBeAttached({ timeout: 30_000 });
    await select.selectOption({ index: 1 });
  }
  await form.getByRole("button", { name: "Preview" }).click();
  await expect(form.locator("section[data-panel-type]").or(page.getByText("The preview could not be read"))).toBeVisible({ timeout: 60_000 });

  await save.click();
  const created = await proposedChange(page);
  opened.push(created);
  await expect(page).toHaveURL(/\/grafana\/dashboards\/jcd-[0-9a-f]{16}\/edit/);
  const uid = /dashboards\/(jcd-[0-9a-f]{16})\//.exec(page.url())?.[1] ?? "";

  approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  await approve(approver.page, PROJECT, created);
  // The reconciler provisions the approved file into daiya; the page reads it back by its uid.
  await expect
    .poll(async () => (await page.request.get(`/api/v1/projects/${PROJECT}/grafana/dashboards/${uid}`)).status(), { timeout: 240_000, intervals: [10_000] })
    .toBe(200);
  await page.goto(`/projects/${PROJECT}/grafana/dashboards/${uid}?lang=en`);
  await expect(page.getByRole("heading", { level: 1, name: TITLE })).toBeVisible({ timeout: 30_000 });

  // An edit is a second proposal against the saved file; the dashboard's Edit link leads there.
  await expect(page.getByRole("link", { name: "Edit" })).toHaveAttribute("href", `/projects/${PROJECT}/grafana/dashboards/${uid}/edit`);
  await page.goto(`/projects/${PROJECT}/grafana/dashboards/${uid}/edit?lang=en`);
  await page.locator("#builder-title").fill(`${TITLE} edited`);
  await page.getByRole("button", { name: "Propose dashboard" }).click();
  const edited = await proposedChange(page);
  opened.push(edited);
  await approve(approver.page, PROJECT, edited);

  // The removal goes through approval too, and the dashboard leaves daiya.
  const token = await csrf(steward.context);
  const removal = await page.request.delete(`/api/v1/projects/${PROJECT}/grafana/builder/${uid}`, { headers: { "x-csrf-token": token } });
  if (removal.status() === 409) {
    // The edit is still being merged: wait it out once.
    await page.waitForTimeout(30_000);
  }
  const removed = removal.status() === 202 ? removal : await page.request.delete(`/api/v1/projects/${PROJECT}/grafana/builder/${uid}`, { headers: { "x-csrf-token": token } });
  expect(removed.status(), await removed.text()).toBe(202);
  const change = ((await removed.json()) as { change: { metadata: { name: string } } }).change.metadata.name;
  opened.push(change);
  await approve(approver.page, PROJECT, change);
  await expect
    .poll(async () => (await page.request.get(`/api/v1/projects/${PROJECT}/grafana/dashboards/${uid}`)).status(), { timeout: 240_000, intervals: [10_000] })
    .toBe(404);
});

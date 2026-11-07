/**
 * T-2474, UI-27: a bigger form is a page with an address of its own, on the deployed Portal.
 *
 * The create form of a kind opens at `/new` and survives a reload; one space's edit form is
 * opened straight from its address, survives a reload, and saving it lands on the list with the
 * change it opened. The change is rejected afterwards, so dev is left as it was.
 *
 * The edit is proposed by demo.editor, whose own change always waits for somebody else (T-2231):
 * the steward is an administrator, and an administrator's change merges as it is proposed (PF-58),
 * which is how this spec once renamed the real helsinki space (T-2764). The spec checks the change
 * is still waiting before it rejects it, and fails loudly if it is not.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { APPROVER, EDITOR, STEWARD, proposedChange, reject, signIn } from "./portal";

const PROJECT = "helsinki";
/** A space named on purpose, never "the first one listed". */
const SPACE = "helsinki-kpi";

async function phase(page: Page, change: string): Promise<string> {
  const answer = await page.request.get(`/api/v1/projects/${PROJECT}/changes/${change}`);
  expect(answer.ok(), `read change ${change}`).toBe(true);
  return String((await answer.json()).status?.phase ?? "");
}
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");

test("a create form is a page at /new that a reload keeps", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/policies/new?lang=en`);
  try {
    const form = page.getByTestId("form-page");
    await expect(form).toBeVisible({ timeout: 30_000 });
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await page.reload({ waitUntil: "load" });
    await expect(form).toBeVisible({ timeout: 30_000 });

    await form.getByRole("button", { name: "Back to the list" }).click();
    await expect(page).toHaveURL(new RegExp(`/projects/${PROJECT}/policies(\\?|$)`));
    await expect(page.getByRole("heading", { level: 1, name: "Policies" })).toBeVisible();
  } finally {
    await context.close();
  }
});

test("an edit form opened from its address survives a reload, and saving lands on the list", async ({
  browser,
}) => {
  const editor = await signIn(browser, EDITOR, `/projects/${PROJECT}/spaces?lang=en`);
  let change = "";
  try {
    await editor.page.goto(`/projects/${PROJECT}/spaces/${SPACE}/edit?lang=en`, { waitUntil: "load" });
    const form = editor.page.getByTestId("form-page");
    await expect(form).toBeVisible({ timeout: 30_000 });
    await editor.page.reload({ waitUntil: "load" });
    await expect(form).toBeVisible({ timeout: 30_000 });
    await expect(form.locator("#root_name")).toHaveValue(SPACE);

    await form.locator("#root_title").fill(`Form pages journey ${SUFFIX}`);
    await form.getByRole("button", { name: "Propose change" }).click();

    change = await proposedChange(editor.page);
    await expect(editor.page).toHaveURL(new RegExp(`/projects/${PROJECT}/spaces(\\?|$)`));
    await expect(form).toHaveCount(0);
    await expect(editor.page.getByRole("heading", { level: 1, name: /spaces/i })).toBeVisible();
  } finally {
    if (change) {
      // A merged change here renamed a real space: say so, never reject it and pass.
      expect(await phase(editor.page, change), `${change} merged: ${SPACE} carries the journey's title`).toBe(
        "PendingApproval",
      );
      const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
      await reject(approver.page, PROJECT, change);
      await approver.context.close();
    }
    await editor.context.close();
  }
});

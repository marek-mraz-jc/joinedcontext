/**
 * Changing and removing through the assistant on dev (T-0736, AG-77, AG-73, UI-45): a steward
 * asks in the dock, the kind's page opens on the change or on the removal, nothing is proposed
 * until the steward proposes it there, and an approver rejects each proposal again so dev keeps
 * what it had.
 */
import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { APPROVER, STEWARD, ask, proposedChange, reject, signIn } from "./portal";

const PROJECT = "helsinki";
/** A context space to open the removal of; the removal is rejected, never merged. */
const SPACE = process.env.E2E_SPACE ?? "citybikes-2046";

test.setTimeout(600_000);

/** Proposes from a form, running its check first when the form asks for a fresh one. */
async function proposeFrom(form: Locator): Promise<void> {
  const propose = form.getByRole("button", { name: "Propose change" });
  if (await propose.isDisabled()) {
    await form.getByRole("button", { name: "Check", exact: true }).click();
  }
  await expect(propose).toBeEnabled({ timeout: 120_000 });
  await propose.click();
}

/** The planned fields of a change, as the approvals route reads them. */
async function planned(page: Page, change: string): Promise<string> {
  const detail = await page.request.get(`/api/v1/projects/${PROJECT}/changes/${change}`);
  expect(detail.ok()).toBe(true);
  return JSON.stringify((await detail.json()).planFields ?? []);
}

test("the assistant opens a change or a removal on the kind's page, and the person proposes it", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  const page = steward.page;

  await ask(page, "Set the helsinki-bikes endpoint's rate limit to 300 requests per minute");
  await expect(page).toHaveURL(/\/projects\/helsinki\/endpoints/, { timeout: 180_000 });
  const endpointForm = page.getByTestId("form-page");
  await expect(endpointForm).toBeVisible();
  await proposeFrom(endpointForm);
  const rateLimit = await proposedChange(page);
  expect(await planned(page, rateLimit)).toContain("300");
  await reject(approver.page, PROJECT, rateLimit);

  await page.goto(`/projects/${PROJECT}/spaces?lang=en`, { waitUntil: "load" });
  await ask(page, "Pause the hel-news pipeline");
  await expect(page).toHaveURL(/\/projects\/helsinki\/pipelines\?edit=hel-news/, { timeout: 180_000 });
  const pipelineForm = page.getByTestId("form-page");
  await expect(pipelineForm).toBeVisible();
  await proposeFrom(pipelineForm);
  const pause = await proposedChange(page);
  expect(await planned(page, pause)).toContain("enabled");
  await reject(approver.page, PROJECT, pause);

  await page.goto(`/projects/${PROJECT}/pipelines?lang=en`, { waitUntil: "load" });
  await ask(page, `Remove the ${SPACE} space`);
  await expect(page).toHaveURL(new RegExp(`/projects/helsinki/spaces\\?delete=${SPACE}`), { timeout: 180_000 });
  const removal = page.getByRole("dialog");
  await expect(removal).toContainText("Remove");
  await removal.getByLabel(`Type ${SPACE} to confirm`).fill(SPACE);
  await removal.getByRole("button", { name: "Propose removal" }).click();
  const id = removal.getByText(/^chg-[0-9a-f]{8}$/);
  await expect(id).toBeVisible({ timeout: 60_000 });
  await reject(approver.page, PROJECT, (await id.textContent()) ?? "");

  await steward.context.close();
  await approver.context.close();
});

test("the assistant drafts a role grant into the Members form, and the person proposes it as a red change", async ({ browser }) => {
  // The demo steward is also the organization's administrator, so the grant is within their rights.
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/settings/members?lang=en`);
  const page = steward.page;

  await ask(page, "Give jana.kovacova the steward role on the helsinki project");
  await expect(page).toHaveURL(/\/projects\/helsinki\/settings\/members(\/new)?\?(.*&)?grant=jana-kovacova-steward-helsinki/, { timeout: 180_000 });
  const grant = page.getByRole("region", { name: "Grant a role" });
  await expect(grant).toBeVisible();
  await expect(grant.getByLabel(/Username or e-mail/)).toHaveValue("jana.kovacova");
  await expect(grant.getByLabel(/^Role/)).toHaveValue("steward");
  await expect(grant.getByLabel("Where it applies")).toHaveValue("project");
  await grant.getByRole("button", { name: "Propose grant" }).click();
  const id = await proposedChange(page);
  const detail = await page.request.get(`/api/v1/projects/${PROJECT}/changes/${id}`);
  expect((await detail.json()).status.lane).toBe("red");
  // Nobody else approves access here, so the steward withdraws it again.
  await reject(page, PROJECT, id);

  await steward.context.close();
});

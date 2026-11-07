/**
 * T-3275: a steward shares an Endpoint with another project until tomorrow, an approver approves,
 * the page says until when, the steward stops it, and once that is approved the project is off the
 * list and its grant has ended. The Policy and the empty Group the share made are removed after.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { APPROVER, STEWARD, approve, proposedChange, removeCompletely, signIn } from "./portal";

test.setTimeout(600_000);
test.describe.configure({ mode: "serial" });

const PROJECT = "helsinki";
const ENDPOINT = "helsinki-bikes-mobility";
const TARGET = "banskabystrica";
let steward: { context: BrowserContext; page: Page };
let groupMade = false;

test.afterAll(async () => {
  if (steward) {
    await removeCompletely(steward, PROJECT, "policies", `${ENDPOINT}-${TARGET}`).catch(() => undefined);
    if (groupMade) await removeCompletely(steward, PROJECT, "groups", TARGET).catch(() => undefined);
    await steward.context.close();
  }
});

async function approvedBy(browser: import("@playwright/test").Browser, change: string): Promise<void> {
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  try {
    await approve(approver.page, PROJECT, change, ENDPOINT);
  } finally {
    await approver.context.close();
  }
}

test("a share until tomorrow is proposed, approved, and says until when", async ({ browser }) => {
  steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/endpoints/${ENDPOINT}?lang=en`);
  const page = steward.page;
  const flow = page.getByTestId("share-flow");
  await expect(flow).toBeVisible({ timeout: 60_000 });
  await flow.getByLabel("Project").selectOption(TARGET);
  await expect(flow.getByText(/What banskabystrica will see/)).toBeVisible();
  groupMade = await flow.getByText(/does not exist yet/).isVisible();
  const tomorrow = new Date(Date.now() + 24 * 3600 * 1000).toISOString().slice(0, 10);
  await flow.getByLabel("Until").fill(tomorrow);
  await flow.getByRole("button", { name: "Propose the share" }).click();
  await approvedBy(browser, await proposedChange(page));
  await page.goto(`/projects/${PROJECT}/endpoints/${ENDPOINT}?lang=en`);
  const shared = page.getByTestId("share-flow").getByRole("listitem").filter({ hasText: TARGET });
  await expect(shared).toContainText("until", { timeout: 120_000 });
});

test("stopping the share takes the project off and ends its grant", async ({ browser }) => {
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/endpoints/${ENDPOINT}?lang=en`);
  await page.getByRole("button", { name: `Stop sharing with ${TARGET}` }).click();
  await approvedBy(browser, await proposedChange(page));
  await page.goto(`/projects/${PROJECT}/endpoints/${ENDPOINT}?lang=en`);
  await expect(page.getByTestId("share-flow").getByRole("listitem").filter({ hasText: TARGET })).toHaveCount(0, { timeout: 120_000 });
  const grant = (await (await page.request.get(`/api/v1/projects/${PROJECT}/policies/${ENDPOINT}-${TARGET}`)).json()) as { spec?: { validity?: { to?: string } } };
  expect(new Date(grant.spec?.validity?.to ?? "2999-01-01").getTime()).toBeLessThanOrEqual(Date.now());
});

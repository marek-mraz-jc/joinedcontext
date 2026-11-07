/**
 * T-3247 — a person removes a space, reads what goes with it before typing the name, and brings it
 * back from Approvals → History once the removal was approved. The space is a sandbox this journey
 * creates and removes again, so dev is as it was.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { STEWARD, approve, checkManifest, cleanUp, csrf, proposeDelete, signIn } from "./portal";

test.setTimeout(420_000);
test.describe.configure({ mode: "serial" });

const PROJECT = "helsinki";
const NAME = `restore-${Date.now().toString(36)}`;
const MANIFEST = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name: NAME, namespace: PROJECT },
  spec: { isSandbox: true },
};

let steward: { context: BrowserContext; page: Page };
let removal = "";

async function live(page: Page): Promise<void> {
  await expect
    .poll(async () => (await page.request.get(`/api/v1/projects/${PROJECT}/spaces/${NAME}`)).status(), {
      timeout: 180_000,
      intervals: [3_000],
    })
    .toBe(200);
}

test.afterAll(async () => {
  if (steward) {
    try {
      // Loud on purpose: a cleanup that fails leaves a Change in Approvals (T-6081).
      await cleanUp(steward, PROJECT, "spaces", NAME, "delete-restore");
    } finally {
      await steward.context.close();
    }
  }
});

test("a sandbox space exists to remove", async ({ browser }) => {
  steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);
  await checkManifest(steward.page, steward.context, PROJECT, MANIFEST);
  const answer = await steward.page.request.post(`/api/v1/projects/${PROJECT}/spaces`, {
    headers: { "x-csrf-token": await csrf(steward.context) },
    data: MANIFEST,
  });
  expect(answer.status(), await answer.text()).toBe(202);
  const change = (await answer.json()) as { metadata: { name: string }; status: { phase: string } };
  if (change.status.phase === "PendingApproval") {
    await approve(steward.page, PROJECT, change.metadata.name);
  }
  await live(steward.page);
});

test("the removal says what comes back before the name is typed, and is approved", async () => {
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/spaces?lang=en`);
  const row = page.locator("tr, li").filter({ hasText: NAME }).first();
  await row.getByRole("button", { name: /^More actions for / }).click();
  await page.getByRole("menuitem", { name: "Remove" }).click();
  await expect(page.getByRole("dialog").getByText(/bring the configuration back from Approvals → History/)).toBeVisible({
    timeout: 30_000,
  });
  await page.keyboard.press("Escape");
  removal = await proposeDelete(page, PROJECT, "spaces", NAME);
  // Approving a removal needs `delete` on the kind, which the steward holds (see removeCompletely).
  await approve(page, PROJECT, removal, NAME);
  await expect
    .poll(async () => (await page.request.get(`/api/v1/projects/${PROJECT}/spaces/${NAME}`)).status(), {
      timeout: 180_000,
      intervals: [3_000],
    })
    .toBe(404);
});

test("the history brings it back as a new change, and once approved the space is there again", async () => {
  expect(removal, "the step above removed the space").toMatch(/^chg-/);
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/approvals?lang=en`);
  await page.getByRole("tab", { name: "History" }).click();
  const search = page.getByRole("search", { name: "Filter the history" });
  await search.getByLabel("Name contains").fill(NAME);
  await search.getByRole("button", { name: "Filter", exact: true }).click();
  const row = page.getByRole("table", { name: "Closed changes" }).getByRole("row").filter({ hasText: removal });
  await row.getByRole("button", { name: `Restore ${NAME}` }).click({ timeout: 60_000 });
  const review = page.getByRole("link", { name: "Review it in Approvals" });
  await expect(review).toBeVisible({ timeout: 60_000 });
  const restoring = ((await review.getAttribute("href")) ?? "").split("/approvals/")[1] ?? "";
  expect(restoring).toMatch(/^chg-/);
  await approve(page, PROJECT, restoring);
  await live(page);
});

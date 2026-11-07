/**
 * T-3274: the history says each change in words, with the fields it touched, and a merged update
 * is undone by a new change. The journey's own Policy (validity ended in 2020, so it grants
 * nothing) is created, changed, read in the history, undone, and removed again.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { APPROVER, STEWARD, approve, checkManifest, cleanUp, csrf, signIn } from "./portal";

test.setTimeout(600_000);
test.describe.configure({ mode: "serial" });

const PROJECT = "helsinki";
const NAME = `history-${Date.now().toString(36)}`;
let steward: { context: BrowserContext; page: Page };

test.afterAll(async () => {
  if (steward) {
    try {
      // Loud on purpose: a cleanup that fails leaves a Change in Approvals (T-6081).
      await cleanUp(steward, PROJECT, "policies", NAME, "change-history");
    } finally {
      await steward.context.close();
    }
  }
});

async function propose(page: Page, context: BrowserContext, method: "post" | "put", q: string): Promise<string> {
  const spaces = (await (await page.request.get(`/api/v1/projects/${PROJECT}/spaces`)).json()) as { items: { metadata: { name: string } }[] };
  const manifest = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Policy",
    metadata: { name: NAME, namespace: PROJECT },
    spec: {
      contextSpaceRef: spaces.items[0].metadata.name,
      assigner: "did:web:hel.fi",
      assignee: { kind: "role", id: "viewer" },
      operations: ["retrieveOps"],
      q,
      validity: { to: "2020-01-01T00:00:00Z" },
    },
  };
  await checkManifest(page, context, PROJECT, manifest);
  const at = method === "post" ? `/api/v1/projects/${PROJECT}/policies` : `/api/v1/projects/${PROJECT}/policies/${NAME}`;
  const answer = await page.request[method](at, {
    headers: { "x-csrf-token": await csrf(context), "content-type": "application/json" },
    data: manifest,
  });
  expect(answer.status(), await answer.text()).toBe(202);
  return ((await answer.json()) as { metadata?: { name?: string } }).metadata?.name ?? "";
}

async function approved(browser: import("@playwright/test").Browser, change: string): Promise<void> {
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  try {
    await approve(approver.page, PROJECT, change, NAME);
  } finally {
    await approver.context.close();
  }
}

test("a policy is created and then changed", async ({ browser }) => {
  steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/approvals?lang=en`);
  await approved(browser, await propose(steward.page, steward.context, "post", "pm10>20"));
  await approved(browser, await propose(steward.page, steward.context, "put", "pm10>50"));
});

test("the history says the change and its field in words, and undoes it", async ({ browser }) => {
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/approvals?tab=history&lang=en`);
  await page.getByLabel("Name").fill(NAME);
  await page.getByRole("button", { name: "Apply" }).click();
  const row = page.getByRole("row").filter({ has: page.getByRole("link", { name: new RegExp(`changed Policy ${NAME}`) }) });
  await expect(row).toBeVisible({ timeout: 120_000 });
  await row.getByRole("button", { name: "What it changed" }).click();
  await expect(row.getByText("changed Q from pm10>20 to pm10>50")).toBeVisible({ timeout: 60_000 });
  await row.getByRole("button", { name: `Undo the change to ${NAME}` }).click();
  const review = page.getByRole("link", { name: "Review it in Approvals" });
  await expect(review).toBeVisible({ timeout: 60_000 });
  const undo = ((await review.getAttribute("href")) ?? "").split("/approvals/")[1] ?? "";
  await approved(browser, undo);
  const stored = (await (await page.request.get(`/api/v1/projects/${PROJECT}/policies/${NAME}`)).json()) as { spec?: { q?: string } };
  expect(stored.spec?.q).toBe("pm10>20");
});

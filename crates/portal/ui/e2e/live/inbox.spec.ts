/**
 * T-3273: the approver decides a steward's change from the inbox, reading its diff there, without
 * opening another page. The change is a Policy whose validity ended in 2020, so it grants nothing
 * while it exists, and the journey removes it again.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { APPROVER, STEWARD, checkManifest, csrf, removeCompletely, signIn } from "./portal";

test.setTimeout(300_000);
test.describe.configure({ mode: "serial" });

const PROJECT = "helsinki";
const NAME = `inbox-${Date.now().toString(36)}`;
let steward: { context: BrowserContext; page: Page };
let change = "";

test.afterAll(async () => {
  if (steward) {
    await removeCompletely(steward, PROJECT, "policies", NAME).catch(() => undefined);
    await steward.context.close();
  }
});

test("the steward proposes a policy that grants nothing", async ({ browser }) => {
  steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/policies?lang=en`);
  const spaces = (await (await steward.page.request.get(`/api/v1/projects/${PROJECT}/spaces`)).json()) as { items: { metadata: { name: string } }[] };
  const manifest = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Policy",
    metadata: { name: NAME, namespace: PROJECT },
    spec: {
      contextSpaceRef: spaces.items[0].metadata.name,
      assigner: "did:web:hel.fi",
      assignee: { kind: "role", id: "viewer" },
      operations: ["retrieveOps"],
      validity: { to: "2020-01-01T00:00:00Z" },
    },
  };
  await checkManifest(steward.page, steward.context, PROJECT, manifest);
  const answer = await steward.page.request.post(`/api/v1/projects/${PROJECT}/policies`, {
    headers: { "x-csrf-token": await csrf(steward.context), "content-type": "application/json" },
    data: manifest,
  });
  expect(answer.status(), await answer.text()).toBe(202);
  change = ((await answer.json()) as { metadata?: { name?: string } }).metadata?.name ?? "";
  expect(change).toMatch(/\S/);
});

test("the approver reads its diff and approves it from the inbox", async ({ browser }) => {
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/inbox?lang=en`);
  try {
    const page = approver.page;
    await expect(page.getByRole("heading", { level: 1, name: "Inbox" })).toBeVisible({ timeout: 60_000 });
    const item = page.getByTestId("inbox-decisions").getByRole("listitem").filter({ hasText: NAME });
    await expect(item).toBeVisible({ timeout: 90_000 });
    await item.getByRole("button", { name: "Show what it changes" }).click();
    await expect(item.getByRole("table")).toBeVisible({ timeout: 60_000 });
    const confirm = item.getByLabel(/type the resource name/i);
    if (await confirm.count()) await confirm.fill(NAME);
    await item.getByRole("button", { name: "Approve" }).click();
    await expect(item.getByRole("status")).toHaveText("Approved: it is being deployed.", { timeout: 90_000 });
    await expect(page).toHaveURL(new RegExp(`/projects/${PROJECT}/inbox`));
  } finally {
    await approver.context.close();
  }
});

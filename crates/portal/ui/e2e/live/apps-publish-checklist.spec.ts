/**
 * T-3267 (AP-140): publishing an App that lacks a title and a description lists both gaps, each
 * with a link to where it is fixed, before anything is proposed. The journey's own preview App is
 * a copy of a seeded one, approved in, read, and removed again; publishing itself is never sent.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { APPROVER, STEWARD, approve, checkManifest, cleanUp, csrf, signIn } from "./portal";

test.setTimeout(420_000);
test.describe.configure({ mode: "serial" });

const PROJECT = "helsinki";
const SEED = "helsinki-events";
const NAME = `checklist-${Date.now().toString(36)}`;

let steward: { context: BrowserContext; page: Page };

test.afterAll(async () => {
  if (steward) {
    try {
      // Loud on purpose: a cleanup that fails leaves a Change in Approvals (T-6081).
      await cleanUp(steward, PROJECT, "apps", NAME, "apps-publish-checklist");
    } finally {
      await steward.context.close();
    }
  }
});

test("a preview App without a title or description is proposed and approved", async ({ browser }) => {
  steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  const page = steward.page;
  const seed = (await (await page.request.get(`/api/v1/projects/${PROJECT}/apps/${SEED}`)).json()) as {
    apiVersion: string;
    kind: string;
    spec: Record<string, unknown>;
  };
  const manifest = {
    apiVersion: seed.apiVersion,
    kind: seed.kind,
    metadata: { name: NAME, namespace: PROJECT },
    spec: { ...seed.spec, lifecycle: "preview", visibility: "project" },
  };
  await checkManifest(page, steward.context, PROJECT, manifest);
  const answer = await page.request.post(`/api/v1/projects/${PROJECT}/apps`, {
    headers: { "x-csrf-token": await csrf(steward.context), "content-type": "application/json" },
    data: manifest,
  });
  expect(answer.status(), await answer.text()).toBe(202);
  const change = ((await answer.json()) as { metadata?: { name?: string } }).metadata?.name ?? "";
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  try {
    await approve(approver.page, PROJECT, change, NAME);
  } finally {
    await approver.context.close();
  }
});

test("publishing it lists the missing title and description, each with where it is fixed", async () => {
  const page = steward.page;
  await page.goto(`/projects/${PROJECT}/apps?lang=en`);
  const card = page.locator("li").filter({ hasText: NAME }).first();
  await expect(card).toBeVisible({ timeout: 120_000 });
  await card.getByRole("button", { name: new RegExp(`^More actions for ${NAME}`) }).click();
  await page.getByRole("menuitem", { name: /^Publish/ }).click();
  const dialog = page.getByRole("dialog");
  const list = dialog.getByRole("region", { name: "Before it is published" });
  await expect(list).toContainText("It has no title", { timeout: 60_000 });
  await expect(list).toContainText("It has no description");
  await expect(list.getByRole("link", { name: "Add a description" })).toHaveAttribute("href", `/projects/${PROJECT}/apps/${NAME}/edit`);
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(dialog).toBeHidden();
});

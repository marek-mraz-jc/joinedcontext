import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";
import { manifestReadsAsRest } from "./manifestReads";

test.beforeEach(({ page }) => manifestReadsAsRest(page));

// T-7071, journey 4 (refusal half; approving is approvals.spec.ts): a Change waits for approval,
// an approver refuses it with a reason the author reads, and a refusal the server answers 403
// shows its reason and changes nothing.

const PROPOSAL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeProposal",
  metadata: { name: "chg-1a2b3c4d", namespace: "helsinki" },
  summary: { key: "change.summary.update", params: { kind: "Endpoint", name: "air-quality", fields: 1 } },
  author: { name: "Marek Mráz", email: "marek@banskabystrica.sk" },
  createdAt: "2026-03-03T12:00:00Z",
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
  planFields: [{ path: "spec.audience", from: "public", to: "organization" }],
};

async function stub(page: Page, refuse: boolean, rejects: string[]): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "jana", name: "Jana Kováčová", email: "jana@banskabystrica.sk", roles: [] });
    }
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/reject")) {
      rejects.push(request.postData() ?? "");
      if (refuse) {
        return route.fulfill({
          status: 403,
          contentType: "application/problem+json",
          body: JSON.stringify({ type: "https://joinedcontext.com/errors/forbidden", title: "Forbidden", status: 403, detail: "rejecting needs approve on Endpoint in project helsinki" }),
        });
      }
      return json({ ...PROPOSAL, kind: "Change", status: { lane: "yellow", phase: "Rejected", plan: { update: 1 } } }, 202);
    }
    if (path.endsWith(`/changes/${PROPOSAL.metadata.name}`)) return json(PROPOSAL);
    if (path.endsWith("/changes")) return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ChangeList", items: [PROPOSAL] });
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

async function reject(page: Page): Promise<void> {
  await page.goto(`/projects/helsinki/approvals/${PROPOSAL.metadata.name}?lang=en`);
  await page.getByRole("button", { name: "Reject" }).click();
  const dialog = page.getByRole("dialog", { name: "Reject this change?" });
  await dialog.getByLabel("Why are you rejecting this?").fill("The air data stays public until the council decides");
  await dialog.getByRole("button", { name: "Reject the change" }).click();
}

for (const width of [1440, 400]) {
  test(`an approver refuses a waiting change with a reason at ${width} px (T-7071)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const rejects: string[] = [];
    await stub(page, false, rejects);
    await reject(page);
    await expect(page.getByText("Rejected", { exact: true }).first()).toBeVisible();
    expect(rejects.map((body) => JSON.parse(body))).toEqual([{ reason: "The air data stays public until the council decides" }]);
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `journey-change-refused-${width}`);
  });

  test(`a refusal the server answers 403 shows its reason at ${width} px (T-7071)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const rejects: string[] = [];
    await stub(page, true, rejects);
    await reject(page);
    await expect(page.getByText(/rejecting needs approve on Endpoint/).first()).toBeVisible();
    await shot(page, `journey-change-refusal-denied-${width}`);
  });
}

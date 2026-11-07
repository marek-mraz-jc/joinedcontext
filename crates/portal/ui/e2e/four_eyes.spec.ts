import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// T-4833, PF-58: access is approved by a second person. An administrator of role bindings opens
// the member form and reads, before the click, that their change waits for another
// administrator; on their own proposal the approve button is off and says why. `vite preview`
// has no Portal API behind it, so the API is answered in the browser. With `JC_EVIDENCE_DIR` set
// the 1440 px and 400 px screenshots are kept there.

const PROJECT = "helsinki";
const EMAIL = "jana.kovacova@hel.fi";
const EVIDENCE = process.env.JC_EVIDENCE_DIR;

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", email: EMAIL, roles: [] };

const PERMISSIONS = {
  project: PROJECT,
  bootstrap: false,
  grants: [
    {
      binding: "admins",
      role: "org-admin",
      scope: "organization",
      rule: { kinds: ["Role", "RoleBinding", "Group", "ServiceAccount", "ContextSpace"], verbs: ["read", "propose", "approve", "delete"] },
    },
  ],
};

const PROPOSAL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeProposal",
  metadata: { name: "chg-4a33b0c1", namespace: PROJECT },
  summary: { key: "change.summary.create", params: { kind: "RoleBinding", name: "jana-admin" } },
  author: { name: "Jana Kováčová", email: EMAIL },
  createdAt: "2026-10-20T12:00:00Z",
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
  planFields: [{ path: "spec.role", to: "org-admin" }],
};

async function stubApi(page: Page): Promise<{ approvals: string[] }> {
  const approvals: string[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path.endsWith("/permissions/me")) return json(PERMISSIONS);
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path.endsWith("/approve")) {
      approvals.push(path);
      return json({ title: "self-approval" }, 403);
    }
    if (path.endsWith("/changes")) return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ChangeList", items: [PROPOSAL] });
    if (path.endsWith(`/changes/${PROPOSAL.metadata.name}`)) return json(PROPOSAL);
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return { approvals };
}

async function shot(page: Page, name: string): Promise<void> {
  if (!EVIDENCE) return;
  mkdirSync(EVIDENCE, { recursive: true });
  await page.screenshot({ path: join(EVIDENCE, `${name}.png`), fullPage: true });
}

for (const width of [1440, 400]) {
  test.describe(`four eyes on access changes at ${width} px`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("the grant and role forms say a second administrator approves them", async ({ page }) => {
      await stubApi(page);
      await page.goto(`/projects/${PROJECT}/settings/members/new?lang=en`);
      await expect(page.getByText(/A second administrator approves it: nobody approves their own grant\./)).toBeVisible();
      await shot(page, `four-eyes-grant-${width}`);
      expect(await axeViolations(page)).toEqual([]);

      await page.goto(`/projects/${PROJECT}/settings/roles/new?lang=en`);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      await expect(page.getByText(/Approved as soon as you propose it/)).toHaveCount(0);
      await shot(page, `four-eyes-role-${width}`);
      expect(await axeViolations(page)).toEqual([]);
    });

    test("an administrator's own access change cannot be approved by them", async ({ page }) => {
      const { approvals } = await stubApi(page);
      await page.goto(`/projects/${PROJECT}/approvals/${PROPOSAL.metadata.name}?lang=en`);
      await expect(page.getByText(/^Access changes need a second person/).first()).toBeVisible();
      await expect(page.getByRole("button", { name: "Approve" })).toBeDisabled();
      await shot(page, `four-eyes-approval-${width}`);
      expect(approvals).toEqual([]);
      expect(await axeViolations(page)).toEqual([]);
    });
  });
}

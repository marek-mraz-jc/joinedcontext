import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { doorsAsRest, manifestReadsAsRest } from "./manifestReads";

// Single-person approvals (T-7388) as the one person of an Organization and its operator meet
// them: the approval of their own access change, refused while the mode is off and allowed with
// a banner while it is on, the step-up the server asks for, the mode on Administration settings
// and the operator's switch. `vite preview` has no Portal behind it, so the API answers here.

test.beforeEach(({ page }) => {
  manifestReadsAsRest(page);
  doorsAsRest(page);
});

const SHOTS = join("test-results", "T-7388");
mkdirSync(SHOTS, { recursive: true });

const IDENTITY = {
  subject: "3f2a9c1e-7b4d-4e8a-9c61-0d5e2b7f8a14",
  username: "marek",
  name: "Marek Mráz",
  email: "marek@marek-mraz.sk",
  roles: [],
};

const PROPOSAL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeProposal",
  metadata: { name: "chg-5e6f7a8b", namespace: "ovzdusie" },
  summary: { key: "change.summary.create", params: { kind: "RoleBinding", name: "marek-air-steward", fields: 2 } },
  author: { name: "Marek Mráz", email: IDENTITY.email },
  createdAt: "2026-11-19T12:00:00Z",
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
  planFields: [
    { path: "spec.role", to: "air-steward" },
    { path: "spec.scope.project", to: "ovzdusie" },
  ],
};

const ADMIN = {
  project: "ovzdusie",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding", "Policy", "Endpoint", "Organization"], verbs: ["read", "propose", "approve", "delete"] } }],
};

const ORGANIZATION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Organization",
  metadata: { name: "marek-mraz", namespace: "org" },
  spec: { domain: "marek-mraz.sk", installation: true, locales: ["sk", "en"], defaultLocale: "sk" },
};

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

async function stub(page: Page, mode: Record<string, unknown>, stepUp = false): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (url.pathname.endsWith("/auth/me")) return json({ ...IDENTITY, secondFactor: true });
    // The operators' group on the Organization namespace, an administrator by binding elsewhere.
    if (url.pathname.endsWith("/permissions/me")) {
      return json(url.pathname.includes("/projects/org/") ? { ...ADMIN, project: "org", bootstrap: true } : ADMIN);
    }
    if (url.pathname === "/api/v1/projects") return json({ items: [{ name: "ovzdusie" }] });
    if (url.pathname === "/api/v1/organization/approval-mode") return json(mode);
    if (url.pathname.endsWith("/approve") && stepUp) {
      return route.fulfill({
        status: 403,
        contentType: "application/problem+json",
        body: JSON.stringify({ type: "https://joinedcontext.com/errors/second-factor-required", title: "Second Factor Required", status: 403 }),
      });
    }
    if (url.pathname.endsWith(`/changes/${PROPOSAL.metadata.name}`)) return json(PROPOSAL);
    if (url.pathname.endsWith("/changes")) return json({ ...list([PROPOSAL]), kind: "ChangeList" });
    if (url.pathname === "/api/v1/projects/org/organizations") return json(list([ORGANIZATION]));
    if (url.pathname === "/api/v1/organization/limits") return json({ entries: [], projects: [] });
    if (url.pathname === "/api/v1/operator/organizations") {
      return json({
        items: [
          { name: "marek-mraz", title: "Marek Mráz", domain: "marek-mraz.sk", hosts: { served: 2 }, approvals: { organization: "marek-mraz", singlePerson: true, peopleWithARole: 1 } },
          { name: "bbsk", title: "Banskobystrický kraj", domain: "bbsk.sk", hosts: { served: 3 }, approvals: { organization: "bbsk", singlePerson: false, peopleWithARole: 6 } },
          { name: "slovensko", title: "Slovensko", domain: null, hosts: {}, approvals: { organization: "slovensko", singlePerson: false, peopleWithARole: 1 } },
        ],
        ids: [],
      });
    }
    return json(list([]));
  });
}

const ON = { organization: "marek-mraz", singlePerson: true, peopleWithARole: 1, changedAt: "2026-11-19T12:30:00Z" };
const OFF = { organization: "marek-mraz", singlePerson: false, peopleWithARole: 1 };

async function shoot(page: Page, name: string): Promise<void> {
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
    await page.screenshot({ path: join(SHOTS, `${name}-${width}.png`), fullPage: true });
  }
}

test("the one person approves their own access change in single-person mode", async ({ page }) => {
  await stub(page, ON, true);
  await page.goto(`/projects/ovzdusie/approvals/${PROPOSAL.metadata.name}?lang=en`);
  await expect(page.getByText("Single-person mode: you approve this alone")).toBeVisible();
  const approve = page.getByRole("button", { name: "Approve", exact: true });
  await expect(approve).toBeEnabled();
  await shoot(page, "approval-single-person-on");
  await page.setViewportSize({ width: 1440, height: 900 });
  await approve.click();
  await expect(page.getByRole("dialog", { name: "Confirm with your one-time code" })).toBeVisible();
  await page.screenshot({ path: join(SHOTS, "approval-step-up-1440.png") });
  await page.setViewportSize({ width: 400, height: 860 });
  await page.screenshot({ path: join(SHOTS, "approval-step-up-400.png") });
});

test("while the mode is off a second person approves", async ({ page }) => {
  await stub(page, OFF);
  await page.goto(`/projects/ovzdusie/approvals/${PROPOSAL.metadata.name}?lang=en`);
  const approve = page.getByRole("button", { name: "Approve", exact: true });
  await expect(approve).toHaveAttribute("aria-disabled", "true");
  await expect(page.getByText("Single-person mode: you approve this alone")).toHaveCount(0);
  await shoot(page, "approval-single-person-off");
});

test("Administration settings say who approves access", async ({ page }) => {
  await stub(page, { ...OFF, peopleWithARole: 2, offReason: "2 people hold a role in the Organization now, and single-person approvals need exactly one. A second person approves access changes again.", changedAt: "2026-11-19T13:00:00Z" });
  await page.goto("/organization/settings?lang=en");
  await expect(page.getByRole("heading", { name: "Approvals" })).toBeVisible();
  await page.getByRole("heading", { name: "Approvals" }).scrollIntoViewIfNeeded();
  await shoot(page, "settings-approvals-turned-off");
});

test("the operator turns single-person approvals on or off per Organization", async ({ page }) => {
  await stub(page, ON);
  await page.goto("/organization/operator-organizations?lang=en");
  await expect(page.getByText("Single-person approvals").first()).toBeVisible();
  await shoot(page, "operator-organizations-approvals");
});

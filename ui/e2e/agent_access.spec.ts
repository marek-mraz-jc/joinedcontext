import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// An agent account's Access view (T-4850, API/01 §52, AG-130…AG-132): its grants with their state,
// the kind × verb matrix with the grant behind each cell, and Revoke now behind a confirmation.
// `vite preview` has no Portal API behind it, so the API is answered in the browser.
// JC_SHOTS=<dir> saves a desktop 1440 and a phone 400 shot of each state.

const IDENTITY = { subject: "b7c1e0f4", username: "ana", name: "Ana Admin", email: "ana@hel.fi", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const ACCOUNT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ServiceAccount",
  metadata: { name: "vendor-ci", namespace: "helsinki", title: { en: "Vendor CI" } },
  spec: {
    owner: { user: "jana@hel.fi" },
    purpose: "Proposes pipelines from the vendor's CI",
    roles: [
      { role: "pipeline-admin", scope: { project: "helsinki" }, validity: { notAfter: "2026-12-31T23:59:59Z" } },
      { role: "endpoint-editor", scope: { project: "helsinki" }, validity: { notAfter: "2026-10-01T00:00:00Z" } },
    ],
    credentials: [{ kind: "oauth-client", name: "main" }],
  },
};
const ACCESS = (mayRevoke: boolean) => ({
  account: "vendor-ci",
  project: "helsinki",
  delegates: false,
  mayPropose: mayRevoke,
  mayRevoke,
  grants: [
    { index: 0, role: "pipeline-admin", scope: "project:helsinki", state: "active", notAfter: "2026-12-31T23:59:59Z",
      rules: [{ kinds: ["Pipeline", "DataSource"], verbs: ["propose", "delete"] }] },
    { index: 1, role: "endpoint-editor", scope: "project:helsinki", state: "expired", notAfter: "2026-10-01T00:00:00Z",
      rules: [{ kinds: ["Endpoint"], verbs: ["propose"] }] },
  ],
});

const AGENT = {
  kind: "ServiceAccount", name: "vendor-ci", project: "helsinki", title: "Vendor CI",
  owner: { type: "person", name: "jana@hel.fi" }, actsAs: "self", spaces: ["air"], endpoints: [],
  lastActivity: null, spend: { used: null, cap: null, per: null }, state: "active", runId: null, profile: null,
};

async function stubApi(page: Page, access: { status: number; body: unknown }, admin: boolean): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/permissions/me")) {
      const verbs = admin ? ["read", "propose", "approve", "delete"] : ["read"];
      return json({ project: "helsinki", bootstrap: false, grants: [
        { role: "r", binding: "b", scope: "project:helsinki", rule: { kinds: ["ServiceAccount", "Role"], verbs } },
      ] });
    }
    if (path === "/api/v1/projects/helsinki/serviceaccounts") return json(list([ACCOUNT]));
    if (path.endsWith("/serviceaccounts/vendor-ci/access")) return json(access.body, access.status);
    if (path === "/api/v1/organization/agents") return json({ items: [AGENT] });
    if (path.endsWith("/serviceaccounts/vendor-ci/revoke")) {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Change",
        metadata: { name: "chg-00000042", namespace: "helsinki" }, status: { phase: "Deploying", lane: "red" } }, 202);
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_SHOTS;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  for (const [width, label] of [[1440, "desktop"], [400, "phone"]] as const) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: join(dir, `agent-access-${name}-${label}.png`), fullPage: true });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

async function open(page: Page): Promise<void> {
  await page.goto("/projects/helsinki/settings/service-accounts?lang=en");
  await page.getByText("What this account may do").click();
}

test.describe("an agent account's Access view", () => {
  test("an administrator reads the matrix and revokes an ended grant", async ({ page }) => {
    await stubApi(page, { status: 200, body: ACCESS(true) }, true);
    await open(page);
    const matrix = page.getByRole("group", { name: "What vendor-ci may do, by kind" });
    await expect(matrix.getByRole("row", { name: /Pipeline/ })).toContainText("pipeline-admin · project:helsinki");
    await expect(matrix.getByRole("row", { name: /Endpoint/ })).toContainText("ended");
    await shot(page, "filled");
    await page.getByRole("button", { name: "Revoke endpoint-editor in project:helsinki now" }).click();
    await expect(page.getByRole("alertdialog")).toContainText("Revoke endpoint-editor from vendor-ci now?");
    await shot(page, "confirm");
    await page.getByRole("alertdialog").getByRole("button", { name: "Revoke now" }).click();
    await expect(page.getByRole("alertdialog")).toBeHidden();
    await shot(page, "revoked");
  });

  test("the agent's own page shows the same matrix on its Access tab", async ({ page }) => {
    await stubApi(page, { status: 200, body: ACCESS(true) }, true);
    await page.goto("/organization/agents/helsinki/ServiceAccount/vendor-ci?lang=en");
    await page.getByRole("tab", { name: "Access" }).click();
    const matrix = page.getByRole("group", { name: "What vendor-ci may do, by kind" });
    await expect(matrix.getByRole("row", { name: /Pipeline/ })).toContainText("pipeline-admin · project:helsinki");
    await shot(page, "agent-tab");
  });

  test("a reader sees the same matrix and no Revoke", async ({ page }) => {
    await stubApi(page, { status: 200, body: ACCESS(false) }, false);
    await open(page);
    await expect(page.getByRole("group", { name: "What vendor-ci may do, by kind" })).toBeVisible();
    await expect(page.getByRole("button", { name: /Revoke/ })).toHaveCount(0);
    await shot(page, "reader");
  });

  test("a refusal reads as the sentence the server sent", async ({ page }) => {
    const refused = { type: "https://joinedcontext.com/errors/forbidden", title: "Forbidden", status: 403,
      detail: "no role grants read on ServiceAccount in project helsinki (PF-50)" };
    await stubApi(page, { status: 403, body: refused }, false);
    await open(page);
    await expect(page.getByText(/no role grants read on ServiceAccount/)).toBeVisible();
    await shot(page, "refused");
  });
});

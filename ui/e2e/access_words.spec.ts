import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// Access tables in plain words (T-4990): Policies, Settings > Your access, Organization > Roles,
// the domain card and the organization's service accounts, read as a steward reads them, at 1440
// and 400 px. `vite preview` has no Portal API behind it, so the API is answered in the browser.

const IDENTITY = { subject: "s", username: "demo.steward", email: "demo.steward@hel.fi", roles: ["portal-admin"] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const ALL_KINDS = [
  "Organization", "Project", "ContextSpace", "DataModel", "Mapping", "Policy", "ScopeDefinition", "Endpoint",
  "ModelProjection", "ServiceAccount", "Pipeline", "DataSource", "MqttBroker", "App", "CkanInstance",
  "Subscription", "Entity", "Dashboard", "Layer", "Role", "RoleBinding", "Group", "Person",
];
const DATA_KINDS = ALL_KINDS.filter((kind) => !["Organization", "Person", "Project", "Role", "RoleBinding", "Group", "ServiceAccount"].includes(kind));

const GRANTS = [
  { role: "org-admin", rule: { verbs: ["propose", "approve", "delete"], kinds: ALL_KINDS }, binding: "admins", scope: "organization" },
  { role: "steward", rule: { verbs: ["propose", "approve"], kinds: DATA_KINDS }, binding: "stewards", scope: "organization" },
  { role: "viewer", rule: { verbs: ["read"], kinds: ALL_KINDS }, binding: "viewers", scope: "organization" },
  { role: "endpoint:helsinki/app-air-quality", rule: { verbs: ["read"], kinds: ["Entity"] }, binding: "app-air-quality", scope: "project:helsinki" },
];

const policy = (name: string, id: string, operations: string[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Policy",
  metadata: { name, namespace: "helsinki" },
  spec: {
    contextSpaceRef: "helsinki",
    assigner: "did:web:hel.fi",
    assignee: { kind: "role", id },
    operations,
    information: [{ entities: [{ type: "AirQualityObserved" }] }],
  },
  status: { phase: "Live" },
});

const POLICIES = [
  policy("app-air-quality-1", "endpoint:helsinki/app-air-quality", ["queryEntity", "retrieveEntity"]),
  policy("app-air-quality-1-history", "endpoint:helsinki/app-air-quality", ["retrieveTemporal"]),
  policy("app-air-quality-2-steward", "endpoint:helsinki/app-air-quality/steward", ["createEntity", "updateAttrs"]),
  policy("air-team-reads", "air-team", ["retrieveOps"]),
];

const role = (name: string, rules: unknown[], title?: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Role",
  metadata: { name, namespace: "org", ...(title ? { title } : {}) },
  spec: { rules },
});

const ROLES = [
  role("approver", [{ verbs: ["approve"], kinds: DATA_KINDS }]),
  role("app-editor", [{ verbs: ["propose"], kinds: ["App"] }]),
  role("reader", [{ verbs: ["read"], kinds: ALL_KINDS }], "Reads everything"),
  role("steward", [{ verbs: ["read"], kinds: ALL_KINDS }, { verbs: ["propose", "approve"], kinds: DATA_KINDS }]),
];

async function stub(page: Page): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ items: [{ name: "helsinki" }] });
    if (path.endsWith("/permissions/me")) return json({ project: "helsinki", grants: GRANTS });
    if (path === "/api/v1/projects/helsinki/policies") return json(list(POLICIES));
    if (path === "/api/v1/projects/org/roles") return json(list(ROLES));
    if (path.endsWith("/organizations")) {
      return json(list([{
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Organization",
        metadata: { name: "hel" },
        spec: { domain: "hel.fi", locales: ["en"], defaultLocale: "en" },
        status: {
          domainVerification: {
            state: "failed",
            checkedAt: "2026-10-21T09:48:00Z",
            reason: "no TXT record at _joinedcontext.hel.fi carries this instance's challenge",
            challenge: "YOYO55R4XJGn3VXRoPr7s9RKFX3V7Ljk1hwmWEpJ71g",
            record: '_joinedcontext.hel.fi TXT "jc-verify=YOYO55R4XJGn3VXRoPr7s9RKFX3V7Ljk1hwmWEpJ71g"',
          },
        },
      }]));
    }
    return json(list([]));
  });
}

/** Screenshots for the task, where `JC_EVIDENCE_DIR` asks for them. */
async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

test.describe("access tables in plain words (T-4990)", () => {
  for (const width of [1440, 400]) {
    test(`a steward reads who may do what without identifiers at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stub(page);

      await page.goto("/projects/helsinki/policies?lang=en");
      const row = page.getByRole("row", { name: /app-air-quality-1 / }).first();
      await expect(row.getByText("Every caller of endpoint app-air-quality").first()).toBeVisible();
      await expect(page.getByText("role:endpoint:helsinki/app-air-quality")).toHaveCount(0);
      await row.getByText("Read entities").click();
      await expect(row.getByText("queryEntity, retrieveEntity")).toBeVisible();
      await shot(page, `policies-${width}`);

      await page.goto("/projects/helsinki/settings/access?lang=en");
      const steward = page.getByRole("row", { name: /^steward/ });
      await expect(steward.getByText(/Propose and approve: \d+ kinds/)).toBeVisible();
      await expect(page.getByText("Every caller of endpoint app-air-quality")).toBeVisible();
      await shot(page, `settings_access-${width}`);

      await page.goto("/organization/roles?lang=en");
      await expect(page.getByRole("row", { name: /steward/ }).getByText("Steward", { exact: true })).toBeVisible();
      await expect(page.getByText("Reads everything")).toBeVisible();
      await shot(page, `organization_roles-${width}`);

      await page.goto("/organization/settings?lang=en");
      await expect(page.getByText("Verify the domain in three steps")).toBeVisible();
      await expect(page.getByRole("alert").filter({ hasText: "TXT" })).toHaveCount(0);
      await shot(page, `organization_settings-${width}`);

      await page.goto("/organization/service-accounts?lang=en");
      await expect(page.getByText(/across its projects/)).toBeVisible();
      await shot(page, `organization_service-accounts-${width}`);
    });
  }
});

import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// A ServiceAccount's client secret (T-5626, PF-37, API/01 §52): an approver issues a new one on
// the account's card behind a confirmation, sees it once with its client id and token URL, and
// Hide forgets it; a reader is told who issues it; an account with a key pair says why it has none.
// `vite preview` has no Portal API behind it, so the API is answered in the browser.
// JC_SHOTS=<dir> saves a desktop 1440 and a phone 400 shot of each state.

const IDENTITY = { subject: "b7c1e0f4", username: "ana", name: "Ana Admin", email: "ana@hel.fi", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const ACCOUNT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ServiceAccount",
  metadata: { name: "vendor", namespace: "helsinki", title: { en: "Vendor feed" } },
  spec: {
    owner: { user: "jana@hel.fi" },
    purpose: "The vendor's air-quality feed writes observations",
    roles: [{ role: "space-writer", scope: { contextSpace: "air" } }],
    credentials: [{ kind: "oauth-client", name: "main" }],
  },
};
const ACCESS = (noSecret: string | null, mayIssueSecret: boolean) => ({
  account: "vendor",
  project: "helsinki",
  delegates: false,
  mayPropose: mayIssueSecret,
  mayRevoke: mayIssueSecret,
  noSecret,
  mayIssueSecret,
  grants: [{ index: 0, role: "space-writer", scope: "contextSpace:air", state: "active", rules: [] }],
});
const ISSUED = {
  clientId: "helsinki-vendor",
  clientSecret: "Zk3vQ9pX2mR7tL4wN8cB1yH6sD5fG0aJ",
  tokenUrl: "https://idm.dev.joinedcontext.com/realms/dev/protocol/openid-connect/token",
};

async function stubApi(page: Page, access: unknown, approver: boolean): Promise<string[]> {
  const issued: string[] = [];
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/permissions/me")) {
      const verbs = approver ? ["read", "propose", "approve", "delete"] : ["read"];
      return json({ project: "helsinki", bootstrap: false, grants: [
        { role: "r", binding: "b", scope: "project:helsinki", rule: { kinds: ["ServiceAccount", "Role"], verbs } },
      ] });
    }
    if (path === "/api/v1/projects/helsinki/serviceaccounts") return json(list([ACCOUNT]));
    if (path.endsWith("/serviceaccounts/vendor/access")) return json(access);
    if (path.endsWith("/serviceaccounts/vendor/secret") && request.method() === "POST") {
      issued.push(path);
      return json(ISSUED);
    }
    return json(list([]));
  });
  return issued;
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_SHOTS;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  for (const [width, label] of [[1440, "desktop"], [400, "phone"]] as const) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: join(dir, `account-secret-${name}-${label}.png`), fullPage: true });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

async function open(page: Page): Promise<void> {
  await page.goto("/projects/helsinki/settings/service-accounts?lang=en");
  await page.getByText("What this account may do").click();
}

test.describe("a ServiceAccount's client secret", () => {
  test("an approver issues a secret, copies it once and Hide forgets it", async ({ page }) => {
    const issued = await stubApi(page, ACCESS(null, true), true);
    await open(page);
    await expect(page.getByText(/signs in as client helsinki-vendor/)).toBeVisible();
    await shot(page, "ready");
    await page.getByRole("button", { name: "Issue new secret" }).click();
    await expect(page.getByRole("alertdialog")).toContainText("Issue a new secret for vendor?");
    await shot(page, "confirm");
    expect(issued).toHaveLength(0);
    await page.getByRole("alertdialog").getByRole("button", { name: "Issue new secret" }).click();
    await expect(page.getByRole("textbox", { name: "Client secret" })).toHaveValue(ISSUED.clientSecret);
    await expect(page.getByRole("textbox", { name: "Client ID" })).toHaveValue("helsinki-vendor");
    await expect(page.getByRole("textbox", { name: "Token URL" })).toHaveValue(ISSUED.tokenUrl);
    expect(issued).toEqual(["/api/v1/projects/helsinki/serviceaccounts/vendor/secret"]);
    await shot(page, "issued");
    await page.getByRole("button", { name: "Hide the secret" }).click();
    await expect(page.getByRole("textbox", { name: "Client secret" })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Issue new secret" })).toBeVisible();
  });

  test("a reader is told who issues the secret", async ({ page }) => {
    await stubApi(page, ACCESS(null, false), false);
    await open(page);
    await expect(page.getByText("A person who may approve service accounts issues its secret.")).toBeVisible();
    await expect(page.getByRole("button", { name: "Issue new secret" })).toHaveCount(0);
    await shot(page, "reader");
  });

  test("a device account says it has a key pair and no secret", async ({ page }) => {
    await stubApi(page, ACCESS("key-pair", true), true);
    await open(page);
    await expect(page.getByText(/signs in with its own key pair/)).toBeVisible();
    await expect(page.getByRole("button", { name: "Issue new secret" })).toHaveCount(0);
    await shot(page, "key-pair");
  });
});

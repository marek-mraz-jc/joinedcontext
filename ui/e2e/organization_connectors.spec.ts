import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// Administration → Connectors (T-5975): every person's connectors with scope, clients and the last
// day's usage, a flagged one revoked behind a confirmation, the empty and refused states.
// `vite preview` has no Portal API behind it, so the API is answered in the browser.
// JC_SHOTS=<dir> saves a desktop 1440 and a phone 400 shot of each state.

const IDENTITY = { subject: "b7c1e0f4", username: "ada", name: "Ada Admin", email: "ada@hel.fi", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const connector = (id: string, name: string, owner: string, extra: Record<string, unknown> = {}) => ({
  id: id.repeat(26),
  name,
  owner,
  segments: ["data-read"],
  scope: [{ project: "helsinki", endpoint: "bike-counters", access: "read" }],
  writes: false,
  expiresAt: "2026-11-27T00:00:00Z",
  allowedClients: ["mcp-local"],
  clientIsBoundary: false,
  state: "active",
  createdAt: "2026-10-27T09:00:00Z",
  lastUsedAt: "2026-10-28T10:00:00Z",
  url: `https://portal.dev.joinedcontext.com/c/${id.repeat(26)}`,
  limits: { resultBytes: 1048576 },
  callsPerDay: 4,
  denialsPerDay: 0,
  flagged: false,
  ...extra,
});

async function stubApi(page: Page, admin: boolean, empty = false): Promise<void> {
  let items = empty
    ? []
    : [
        connector("a", "jana-claude-code", "jana@hel.fi", { callsPerDay: 31, denialsPerDay: 12, flagged: true }),
        connector("b", "pia-chatgpt", "pia@hel.fi", {
          allowedClients: ["https://chatgpt.com/oauth/client-metadata"],
          clientIsBoundary: true,
          segments: ["data-read", "pipeline"],
          writes: true,
        }),
        connector("c", "ola-old", "ola@hel.fi", { state: "revoked", lastUsedAt: null, callsPerDay: 0 }),
      ];
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/permissions/me")) {
      const rule = admin
        ? { kinds: ["RoleBinding"], verbs: ["read", "propose", "approve", "delete"] }
        : { kinds: ["*"], verbs: ["read"] };
      return json({ project: "org", bootstrap: false, grants: [{ role: "r", binding: "b", rule }] });
    }
    if (path === "/api/v1/organization/connectors") return json({ items, denialsFlag: 10 });
    if (request.method() === "DELETE" && path.startsWith("/api/v1/organization/connectors/")) {
      const id = path.split("/").pop();
      items = items.map((item) => (item.id === id ? { ...item, state: "revoked" } : item));
      return route.fulfill({ status: 204 });
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
    await page.screenshot({ path: join(dir, `org-connectors-${name}-${label}.png`), fullPage: true });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

test.describe("Administration → Connectors", () => {
  test("an administrator finds the flagged connector and revokes it", async ({ page }) => {
    await stubApi(page, true);
    await page.goto("/organization/connectors?lang=en");
    const table = page.getByRole("table", { name: "Every person's connectors" });
    await expect(table.getByRole("row", { name: /jana-claude-code/ })).toContainText("12 refused");
    await expect(page.getByText(/1 active connector is flagged/)).toBeVisible();
    await shot(page, "filled");
    await table.getByRole("button", { name: "Revoke connector jana-claude-code" }).click();
    await expect(page.getByRole("alertdialog")).toContainText("Revoke connector jana-claude-code?");
    await shot(page, "confirm");
    await page.getByRole("alertdialog").getByRole("button", { name: "Revoke" }).click();
    await expect(page.getByRole("alertdialog")).toBeHidden();
    await expect(table.getByRole("row", { name: /jana-claude-code/ })).toHaveCount(0);
    await page.getByLabel("State").selectOption("revoked");
    await expect(table.getByRole("row", { name: /jana-claude-code/ })).toContainText("Revoked");
    await shot(page, "revoked");
  });

  test("an organization nobody gave a connector says how one comes", async ({ page }) => {
    await stubApi(page, true, true);
    await page.goto("/organization/connectors?lang=en");
    await expect(page.getByText("Nobody holds a connector yet")).toBeVisible();
    await shot(page, "empty");
  });

  test("anybody else is told the page is the administrator's", async ({ page }) => {
    await stubApi(page, false);
    await page.goto("/organization/connectors?lang=en");
    await expect(page.getByText(/This page is for organization administrators/)).toBeVisible();
    await shot(page, "refused");
  });
});

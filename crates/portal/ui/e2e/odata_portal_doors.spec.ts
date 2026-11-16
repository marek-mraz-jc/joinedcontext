import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test, type Locator, type Page, type Route } from "@playwright/test";
import { axeViolations } from "./axe";
import { odataBody } from "../tests/odata";

// T-7100: the person-only doors (API/13 §2b, Portal doors) through the Organization's OData
// service, `…/JoinedContext.Portal.portal_*`. Each journey runs the real build against the
// service's answers (empty, filled, refused, done) at 1440 and 400 px; the network capture proves
// the old REST doors are never asked and that a secret's value only goes out.

const SHOTS = process.env.SHOTS_DIR ?? "test-results/T-7100";
mkdirSync(SHOTS, { recursive: true });

const API = "joinedcontext.com/v1alpha1";
const ADMIN = {
  role: "org-admin",
  binding: "admins",
  scope: "organization",
  rule: { kinds: ["Organization", "Project", "RoleBinding", "Person", "*"], verbs: ["read", "propose", "approve", "delete"] },
};

/** The REST doors these pages used to call; none may be asked any more. */
const OLD_DOORS = /\/projects\/[^/]+\/secrets$|\/organization\/connectors(\/|$)/;

type Answer = { status: number; value?: unknown; message?: string; target?: string; field?: string };
type Asked = { path: string; method: string; body: string };

/** The page's API, each Portal door answered by `doors[name]`; returns every `/api/` request. */
async function stub(page: Page, doors: Record<string, Answer>): Promise<Asked[]> {
  const asked: Asked[] = [];
  await page.route("**/api/**", (route: Route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = decodeURIComponent(url.pathname);
    asked.push({ path, method: request.method(), body: request.postData() ?? "" });
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const door = /JoinedContext\.Portal\.(portal_\w+)/.exec(path)?.[1];
    if (door) {
      expect(request.headers()["odata-version"], "an OData request").toBe("4.01");
      const answer = doors[door];
      if (!answer) return json({ error: { code: "not-implemented", message: `no ${door} here`, status: 501 } }, 501);
      if (answer.status !== 200) {
        const extra = answer.target ? { target: answer.target, details: [{ code: "unprocessable", target: answer.target, message: answer.field }] } : {};
        return json(
          { error: { code: "unprocessable", message: answer.message, type: "https://joinedcontext.com/errors/unprocessable", title: "Error", status: answer.status, ...extra } },
          answer.status,
        );
      }
      return route.fulfill({ status: 200, contentType: "application/json", body: odataBody(answer.value ?? null) });
    }
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "anna", name: "Anna Admin", email: "anna@hel.fi", roles: [] });
    if (path.endsWith("/permissions/me")) return json({ project: "org", bootstrap: false, grants: [ADMIN] });
    if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: "helsinki" }] });
    return json({ apiVersion: API, kind: "List", items: [] });
  });
  return asked;
}

async function shoot(page: Page, name: string, width: number, at: Locator): Promise<void> {
  expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth), "nothing scrolls sideways").toBe(0);
  await at.scrollIntoViewIfNeeded();
  await page.screenshot({ path: join(SHOTS, `${name}-${width}.png`) });
}

const TYPED = "abc123-typed-in-the-form";
const CHANGE = {
  apiVersion: API,
  kind: "Change",
  metadata: { name: "pr-7", project: "helsinki" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1, update: 0, delete: 0 } },
};

const connector = (id: string, name: string, extra: Record<string, unknown> = {}) => ({
  id,
  name,
  owner: "jana-id",
  segments: ["data-read"],
  scope: [{ project: "doprava", endpoint: "counters", access: "read" }],
  writes: false,
  expiresAt: "2026-11-27T00:00:00Z",
  allowedClients: ["mcp-local"],
  clientIsBoundary: false,
  state: "active",
  createdAt: "2026-10-27T09:00:00Z",
  lastUsedAt: "2026-10-28T10:00:00Z",
  url: `https://portal.test/c/${id}`,
  limits: { resultBytes: 1048576 },
  callsPerDay: 4,
  denialsPerDay: 0,
  flagged: false,
  ...extra,
});

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.use(width < 640 ? { hasTouch: true, isMobile: true } : {});
    test.beforeEach(async ({ page }) => page.setViewportSize({ width, height: 900 }));

    test("the secret form lists and proposes through the secret doors: empty, refused, filled, done", async ({ page }) => {
      const doors: Record<string, Answer> = { portal_secret_list: { status: 200, value: { items: [] } } };
      const asked = await stub(page, doors);
      await page.goto("/projects/helsinki/secrets/new?name=counters-api&key=apiKey&lang=en");
      const empty = page.getByText("This project has no secret yet: enter the first one below.");
      await expect(empty).toBeVisible();
      await shoot(page, "secret-empty", width, empty);
      expect(await axeViolations(page)).toEqual([]);

      doors.portal_secret_set = {
        status: 422,
        message: "the secret cannot be written as sent; nothing was kept",
        target: "values.apiKey",
        field: "1 to 16384 bytes",
      };
      await page.getByLabel(/^Value/).fill(TYPED);
      await page.getByRole("button", { name: "Encrypt and propose" }).click();
      const refused = page.getByRole("alert").filter({ hasText: "values.apiKey: 1 to 16384 bytes" });
      await expect(refused).toBeVisible();
      await shoot(page, "secret-refused", width, refused);

      doors.portal_secret_list = { status: 200, value: { items: [{ name: "counters-api", keys: ["apiKey", "username"], path: "projects/helsinki/secrets/counters-api.enc.yaml" }] } };
      doors.portal_secret_set = { status: 200, value: CHANGE };
      await page.reload();
      const kept = page.getByText(/apiKey, username/);
      await expect(kept).toBeVisible();
      await shoot(page, "secret-filled", width, kept);
      await page.getByLabel(/^Value/).first().fill(TYPED);
      await page.getByRole("button", { name: "Encrypt and propose" }).click();
      const done = page.getByText("pr-7").first();
      await expect(done).toBeVisible();
      await shoot(page, "secret-done", width, done);
      expect(await page.content()).not.toContain(TYPED);

      const sets = asked.filter((a) => a.path.endsWith("portal_secret_set"));
      expect(sets.length).toBe(2);
      expect(sets.every((a) => a.method === "POST" && a.path.startsWith("/api/v1/projects('helsinki')/"))).toBe(true);
      expect(JSON.parse(sets[0].body)).toEqual({ body: { name: "counters-api", values: { apiKey: TYPED } } });
      expect(asked.filter((a) => OLD_DOORS.test(a.path))).toEqual([]);
    });

    test("Connectors lists and revokes through the connector doors: filled, confirm, empty", async ({ page }) => {
      const doors: Record<string, Answer> = {
        portal_connector_list: {
          status: 200,
          value: { items: [connector("a".repeat(26), "jana-claude-code", { callsPerDay: 31, denialsPerDay: 12, flagged: true })], denialsFlag: 10 },
        },
        portal_connector_revoke: { status: 200, value: null },
        portal_person_names: { status: 200, value: { items: [{ id: "jana-id", name: "Jana Kováčová" }] } },
      };
      const asked = await stub(page, doors);
      await page.goto("/organization/connectors?lang=en");
      const table = page.getByRole("table", { name: "Every person's connectors" });
      await expect(table.getByText("jana-claude-code")).toBeVisible();
      await shoot(page, "connectors-filled", width, table);
      expect(await axeViolations(page)).toEqual([]);

      await table.getByRole("button", { name: "Revoke connector jana-claude-code" }).click();
      const dialog = page.getByRole("alertdialog", { name: "Revoke connector jana-claude-code?" });
      await expect(dialog).toBeVisible();
      await shoot(page, "connectors-confirm", width, dialog);
      doors.portal_connector_list = { status: 200, value: { items: [], denialsFlag: 10 } };
      await dialog.getByRole("button", { name: "Revoke" }).click();
      const empty = page.getByText("Nobody holds a connector yet");
      await expect(empty).toBeVisible();
      await shoot(page, "connectors-empty", width, empty);

      const revoke = asked.find((a) => a.path.endsWith("portal_connector_revoke"));
      expect(revoke?.path).toBe("/api/v1/Organization/JoinedContext.Portal.portal_connector_revoke");
      expect(JSON.parse(revoke?.body ?? "{}")).toEqual({ connector: "a".repeat(26) });
      expect(asked.filter((a) => OLD_DOORS.test(a.path))).toEqual([]);
    });
  });
}

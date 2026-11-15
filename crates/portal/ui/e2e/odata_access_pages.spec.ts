import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test, type Page, type Route } from "@playwright/test";
import { axeViolations } from "./axe";
import { odataBody, odataCall } from "../tests/odata";

// T-6812: the access and policy pages read through the Organization's OData service. Each journey
// runs the real build against the service's answers (filled, empty, refused, failed) at 1440 and
// 400 px, and the network capture proves the old REST doors of these reads are never asked.

const SHOTS = process.env.SHOTS_DIR ?? "test-results/T-6812";
mkdirSync(SHOTS, { recursive: true });

const API = "joinedcontext.com/v1alpha1";
const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
/** An organization administrator, the person both panels are for (EP-103, T-4947). */
const ADMIN = { role: "org-admin", binding: "admins", scope: "organization", rule: { kinds: ["RoleBinding"], verbs: ["approve", "delete"] } };
const ENDPOINT = { apiVersion: API, kind: "Endpoint", metadata: { name: "air", namespace: "helsinki" }, spec: { slug: "air-quality" } };
const MODEL = { name: "air-quality", level: "project", project: "helsinki", version: "1.0.0", lifecycle: "published", classes: ["AirQualityObserved"] };
const JANA = { id: "jana-id", email: "jana@hel.fi", firstName: "Jana", lastName: "K", enabled: true, emailVerified: true };
const ACCOUNT = { apiVersion: API, kind: "ServiceAccount", metadata: { name: "etl", namespace: "helsinki" }, spec: {} };
const REACHED = {
  subject: { serviceAccount: "helsinki-etl", groups: [], roles: [] },
  endpoints: ["city-air", "noise"].map((name) => ({
    name,
    slug: `${name}-slug`,
    space: "air",
    representations: ["ngsi-ld"],
    policies: [`${name}-read`],
    permissions: [{ resource: { type: "AirQualityObserved" }, actions: ["queryEntity"], attributes: ["pm10"], constraints: {} }],
    prohibitions: [],
  })),
  mcpServers: [],
  consents: [],
};
const CLOSED = {
  apiVersion: API,
  kind: "Change",
  metadata: { name: "chg-0000002a", namespace: "helsinki" },
  author: { name: "Anna Steward" },
  createdAt: "2026-11-14T08:00:00Z",
  decision: { at: "2026-11-14T09:30:00Z", by: "Pekka Approver", reason: "Writes stay with the stewards" },
  status: { phase: "Rejected", lane: "yellow", plan: {} },
  summary: { key: "change.summary.update", params: { kind: "Policy", name: "air-write", fields: 1 } },
};

/** The REST doors these pages used to read; none may be asked any more. */
const OLD_DOORS = /\/access\/explore$|\/access\/simulate$|\/changes\/history$|\/serviceaccounts\/[^/]+\/access$|\/endpoints\/[^/]+\/preview$/;

type Answer = { status: number; value?: unknown; message?: string };

/** The page's API, the OData functions answered by `functions`; returns every `/api/` path asked. */
async function stub(page: Page, functions: Record<string, Answer>): Promise<string[]> {
  const asked: string[] = [];
  await page.route("**/api/**", (route: Route) => {
    const request = route.request();
    const url = new URL(request.url());
    asked.push(url.pathname);
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const called = odataCall(url);
    if (called) {
      expect(request.headers()["odata-version"], `${called.name} is an OData request`).toBe("4.01");
      const answer = functions[called.name];
      if (!answer) return json({ error: { code: "not-implemented", message: `no ${called.name} here`, status: 501 } }, 501);
      if (answer.status !== 200) {
        const code = answer.status === 403 ? "forbidden" : "internal-error";
        return json({ error: { code, message: answer.message, type: `https://joinedcontext.com/errors/${code}`, title: "Error", status: answer.status } }, answer.status);
      }
      return route.fulfill({ status: 200, contentType: "application/json", body: odataBody(answer.value) });
    }
    const path = url.pathname;
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "anna", name: "Anna Admin", email: "anna@hel.fi", roles: [] });
    if (path.endsWith("/permissions/me")) return json({ project: "org", bootstrap: false, grants: [ADMIN] });
    if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path === "/api/v1/projects/helsinki/endpoints") return json(list([ENDPOINT]));
    if (path === "/api/v1/projects/helsinki/serviceaccounts") return json(list([ACCOUNT]));
    if (path === "/api/v1/organization/people") return json({ items: [JANA], total: 1 });
    if (path === "/api/v1/organization/datamodels") return json({ items: [MODEL] });
    return json(list([]));
  });
  return asked;
}

async function shoot(page: Page, name: string, width: number): Promise<void> {
  expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth), "nothing scrolls sideways").toBe(0);
  await page.screenshot({ path: join(SHOTS, `${name}-${width}.png`), fullPage: true });
}

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.beforeEach(async ({ page }) => page.setViewportSize({ width, height: 900 }));

    test("Try a person asks the gateway through jc_access_simulate: allowed, refused, failed", async ({ page }) => {
      const answers: Record<string, Answer> = {
        jc_access_simulate: { status: 200, value: { decision: true, reason: "policy_grant_matched", policy: "air-read", subject: { user: "jana@hel.fi", groups: [], roles: [] } } },
      };
      const asked = await stub(page, answers);
      await page.goto("/projects/helsinki/policies?lang=en");
      const panel = page.locator("div.rounded-lg", { has: page.getByRole("heading", { name: "Try a person" }) });
      await expect(panel.getByRole("option", { name: "jana@hel.fi" })).toBeAttached();
      await panel.getByRole("combobox", { name: "Entity type" }).click();
      await page.getByRole("option", { name: /AirQualityObserved/ }).click();
      const ask = panel.getByRole("button", { name: "Ask the gateway" });
      await ask.click();
      const verdict = page.getByTestId("try-verdict");
      await expect(verdict).toContainText("Allowed");
      await verdict.scrollIntoViewIfNeeded();
      await shoot(page, "try-a-person-allowed", width);

      answers.jc_access_simulate = { status: 200, value: { decision: false, reason: "no_grant", subject: { groups: [], roles: [] } } };
      await ask.click();
      await expect(verdict).toContainText("Refused");
      await shoot(page, "try-a-person-refused", width);

      answers.jc_access_simulate = { status: 503, message: "The gateway did not answer; try again shortly." };
      await ask.click();
      await expect(panel.getByText("The gateway did not answer; try again shortly.")).toBeVisible();
      await shoot(page, "try-a-person-error", width);
      expect(asked.filter((path) => OLD_DOORS.test(path))).toEqual([]);
      expect(await axeViolations(page)).toEqual([]);
    });

    test("the access explorer reads what a service account reaches through jc_access_explore: filled, empty, refused", async ({ page }) => {
      const answers: Record<string, Answer> = { jc_access_explore: { status: 200, value: REACHED } };
      const asked = await stub(page, answers);
      await page.goto("/projects/helsinki/settings/explorer?kind=serviceAccount&name=etl&lang=en");
      const show = page.getByRole("button", { name: "Show access" });
      await show.click();
      await expect(page.getByText("2 Endpoints reached")).toBeVisible();
      await expect(page.getByText("city-air", { exact: true })).toBeVisible();
      await shoot(page, "access-explorer-filled", width);

      answers.jc_access_explore = { status: 200, value: { ...REACHED, endpoints: [] } };
      await show.click();
      await expect(page.getByText("2 Endpoints reached")).toBeHidden();
      await shoot(page, "access-explorer-empty", width);

      answers.jc_access_explore = { status: 403, message: "Only an organization administrator explores another's access." };
      await show.click();
      await expect(page.getByText("Only an organization administrator explores another's access.")).toBeVisible();
      await shoot(page, "access-explorer-refused", width);
      expect(asked.filter((path) => OLD_DOORS.test(path))).toEqual([]);
      expect(await axeViolations(page)).toEqual([]);
    });

    test("the approvals history pages through jc_change_history: filled, empty, failed", async ({ page }) => {
      const answers: Record<string, Answer> = { jc_change_history: { status: 200, value: { ...list([CLOSED]), next: null } } };
      const asked = await stub(page, answers);
      await page.goto("/projects/helsinki/approvals?lang=en");
      await page.getByRole("tab", { name: "History" }).click();
      await expect(page.getByText("Reason: Writes stay with the stewards")).toBeVisible();
      await shoot(page, "approvals-history-filled", width);

      answers.jc_change_history = { status: 200, value: { ...list([]), next: null } };
      await page.getByLabel("Name contains").fill("nothing");
      await page.getByRole("button", { name: "Filter", exact: true }).click();
      await expect(page.getByText("Reason: Writes stay with the stewards")).toBeHidden();
      await shoot(page, "approvals-history-empty", width);

      answers.jc_change_history = { status: 503, message: "The forge does not answer." };
      await page.getByLabel("Name contains").fill("again");
      await page.getByRole("button", { name: "Filter", exact: true }).click();
      await expect(page.getByText(/The forge does not answer/)).toBeVisible();
      await shoot(page, "approvals-history-error", width);
      expect(asked.filter((path) => OLD_DOORS.test(path))).toEqual([]);
    });
  });
}

import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test, type Locator, type Page, type Route } from "@playwright/test";
import { axeViolations } from "./axe";
import { odataBody } from "../tests/odata";
import { doorsAsRest } from "./manifestReads";

// T-7176: the stub answers permissions/me by its REST address; the page asks its Portal door.
test.beforeEach(({ page }) => doorsAsRest(page, ["portal_permissions_me", "portal_org_permissions_me", "portal_project_list"]));

// T-7101: People's disable, enable, sign-out and delete and Agent activity's filters through the
// Organization's OData service (API/13 §2b): `Organization/JoinedContext.Portal.jc_person_*`,
// `portal_person_delete` and `jc_organization_read(aspect='agentActivity', …)`. Each journey runs
// the real build against the service's answers at 1440 and 400 px; the network capture proves the
// old REST doors are never asked.

const SHOTS = process.env.SHOTS_DIR ?? "test-results/T-7101";
mkdirSync(SHOTS, { recursive: true });

const API = "joinedcontext.com/v1alpha1";
const ADMIN = {
  role: "org-admin",
  binding: "admins",
  scope: "organization",
  rule: { kinds: ["Organization", "Project", "RoleBinding", "Person", "AgentProfile", "*"], verbs: ["read", "create", "update", "disable", "delete", "approve"] },
};

/** The REST doors these pages used to call; none may be asked any more. */
const OLD_DOORS = /\/organization\/people\/[^/]+\/(disable|enable|sign-out)$|\/organization\/agents\/activity/;

type Answer = { status: number; value?: unknown; message?: string };
type Asked = { path: string; query: URLSearchParams; method: string; body: string };

/** The page's API: each OData operation answered by `ops(name, params)`; returns every `/api/` request. */
async function stub(
  page: Page,
  ops: (name: string, params: Record<string, string>, body: string) => Answer | undefined,
  rest: Record<string, unknown> = {},
): Promise<Asked[]> {
  const asked: Asked[] = [];
  await page.route("**/api/**", (route: Route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = decodeURIComponent(url.pathname);
    const body = request.postData() ?? "";
    asked.push({ path, query: url.searchParams, method: request.method(), body });
    const json = (value: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(value) });
    const op = /JoinedContext\.Portal\.(\w+)(?:\(([^)]*)\))?$/.exec(path);
    if (op) {
      expect(request.headers()["odata-version"], "an OData request").toBe("4.01");
      const params: Record<string, string> = {};
      for (const argument of (op[2] ?? "").split(",").filter(Boolean)) {
        const [name, alias] = argument.split("=");
        const value = url.searchParams.get(alias);
        if (value !== null) params[name] = value.replace(/^'|'$/g, "");
      }
      const answer = ops(op[1], params, body);
      if (!answer) return json({ error: { code: "not-implemented", message: `no ${op[1]} here`, status: 501 } }, 501);
      if (answer.status !== 200) {
        return json(
          { error: { code: "forbidden", message: answer.message, type: "https://joinedcontext.com/errors/forbidden", title: "Forbidden", status: answer.status } },
          answer.status,
        );
      }
      return route.fulfill({ status: 200, contentType: "application/json", body: odataBody(answer.value ?? null) });
    }
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "anna", name: "Anna Admin", email: "anna@hel.fi", roles: [] });
    if (path.endsWith("/permissions/me")) return json({ project: "org", bootstrap: false, grants: [ADMIN] });
    if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path in rest) return json(rest[path]);
    return json({ apiVersion: API, kind: "List", items: [] });
  });
  return asked;
}

async function shoot(page: Page, name: string, width: number, at: Locator): Promise<void> {
  expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth), "nothing scrolls sideways").toBe(0);
  await at.scrollIntoViewIfNeeded();
  await page.screenshot({ path: join(SHOTS, `${name}-${width}.png`) });
}

const person = (id: string, first: string, last: string, extra: Record<string, unknown> = {}) => ({
  id,
  email: `${first.toLowerCase()}@hel.fi`,
  firstName: first,
  lastName: last,
  enabled: true,
  emailVerified: true,
  requiredActions: [],
  createdAt: "2026-09-01T08:00:00Z",
  lastSeen: "2026-11-14T10:30:00Z",
  locale: "sk",
  pendingDeletion: null,
  ...extra,
});

/** The server's sentence for a person holding a right the caller lacks (api/people.rs, PF-93). */
const REFUSED =
  "eva@hel.fi holds a right you do not, through the binding 'org-admins': only an organization administrator grants approve on Organization (PF-93)";

const PROFILE = {
  kind: "AgentProfile",
  name: "builder",
  project: "helsinki",
  title: "App builder",
  owner: { type: "project", name: "helsinki" },
  actsAs: "caller",
  spaces: ["air"],
  endpoints: ["air-quality"],
  lastActivity: "2026-11-15T02:10:00Z",
  spend: { used: 41000, cap: 400000, per: "day" },
  state: "active",
  runId: null,
  profile: null,
};

const RUN = {
  id: "run-7f3a",
  project: "helsinki",
  profile: "builder",
  title: "Air quality dashboard",
  startedBy: "ana@hel.fi",
  status: "succeeded",
  createdAt: "2026-11-15T02:10:00Z",
  spaces: ["air"],
  tokens: 41000,
  costUsd: 0.0125,
  refusals: 1,
  writes: 1,
  mayStop: false,
  steps: [
    { at: "2026-11-15T02:10:04Z", tool: "jc_resource_list", target: "Endpoint", verdict: "ok", write: false, durationMs: 9 },
    { at: "2026-11-15T02:10:09Z", tool: "jc_endpoint_propose", target: "air", verdict: "refused", write: true, reason: "forbidden: missing role steward" },
  ],
};

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.use(width < 640 ? { hasTouch: true, isMobile: true } : {});
    test.beforeEach(async ({ page }) => page.setViewportSize({ width, height: 900 }));

    test("People disables the ticked people through jc_person_disable and names whom the server refused", async ({ page }) => {
      const people = { items: [person("jana-id", "Jana", "Nováková"), person("eva-id", "Eva", "Kováčová")] };
      const asked = await stub(
        page,
        (name, _params, body) => {
          if (name !== "jc_person_disable") return undefined;
          return (JSON.parse(body) as { id: string }).id === "eva-id"
            ? { status: 403, message: REFUSED }
            : { status: 200, value: null };
        },
        { "/api/v1/organization/people": people },
      );
      await page.goto("/organization/people?lang=en");
      const table = page.getByRole("checkbox", { name: "Select Jana Nováková" });
      await expect(table).toBeVisible();
      await shoot(page, "people-filled", width, table);
      expect(await axeViolations(page)).toEqual([]);

      await table.check();
      await page.getByRole("checkbox", { name: "Select Eva Kováčová" }).check();
      await page.getByRole("button", { name: "Disable", exact: true }).click();
      const dialog = page.getByRole("alertdialog", { name: "Disable 2 people?" });
      await expect(dialog).toBeVisible();
      await shoot(page, "people-confirm", width, dialog);
      await dialog.getByTestId("confirm-accept").click();
      const status = page.getByText(/1 of 2 disabled/);
      await expect(status).toBeVisible();
      await expect(status).toContainText("through the binding 'org-admins'");
      await shoot(page, "people-refused", width, status);

      const disables = asked.filter((a) => a.path.endsWith("jc_person_disable"));
      expect(disables.map((a) => [a.method, a.path, JSON.parse(a.body)])).toEqual([
        ["POST", "/api/v1/Organization/JoinedContext.Portal.jc_person_disable", { id: "jana-id" }],
        ["POST", "/api/v1/Organization/JoinedContext.Portal.jc_person_disable", { id: "eva-id" }],
      ]);
      expect(asked.filter((a) => OLD_DOORS.test(a.path))).toEqual([]);
    });

    test("Agent activity filters its runs through jc_organization_read: filled, filtered, none match", async ({ page }) => {
      const readings: Record<string, string>[] = [];
      const asked = await stub(page, (name, params) => {
        if (name !== "jc_organization_read") return undefined;
        if (params.aspect === "agents") return { status: 200, value: { items: [PROFILE] } };
        if (params.aspect !== "agentActivity") return { status: 200, value: {} };
        readings.push(params);
        const filtered = params.verdict === "ok";
        return {
          status: 200,
          value: { runs: filtered ? [] : [RUN], alerts: [], choices: { people: ["ana@hel.fi"], spaces: ["air"], alertProjects: [] } },
        };
      });
      await page.goto("/organization/agents/helsinki/AgentProfile/builder?lang=en#activity");
      const runs = page.getByRole("list", { name: "Runs" });
      await expect(runs.getByText("Air quality dashboard")).toBeVisible();
      await shoot(page, "activity-filled", width, runs);
      expect(await axeViolations(page)).toEqual([]);

      await page.getByLabel("Started by").selectOption("ana@hel.fi");
      await page.getByLabel("Verdict").selectOption("ok");
      await page.getByRole("button", { name: "Apply" }).click();
      const none = page.getByText("No run matches these filters");
      await expect(none).toBeVisible();
      await shoot(page, "activity-none", width, none);

      expect(readings[0]).toMatchObject({ aspect: "agentActivity", profile: "builder" });
      expect(readings[0]).not.toHaveProperty("inProject");
      expect(readings.at(-1)).toMatchObject({ aspect: "agentActivity", profile: "builder", person: "ana@hel.fi", verdict: "ok" });
      expect(asked.filter((a) => OLD_DOORS.test(a.path))).toEqual([]);
    });
  });
}

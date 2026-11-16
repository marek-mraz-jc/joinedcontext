import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test, type Locator, type Page, type Route } from "@playwright/test";
import { axeViolations } from "./axe";
import { odataBody } from "../tests/odata";

// T-7099: the Administration pages read the Organization's readings through the OData service,
// `Organization/JoinedContext.Portal.jc_organization_read(aspect=…)`. Each journey runs the real
// build against the service's answers (filled, empty, refused, failed) at 1440 and 400 px, and the
// network capture proves the old REST doors of these reads are never asked.

const SHOTS = process.env.SHOTS_DIR ?? "test-results/T-7099";
mkdirSync(SHOTS, { recursive: true });

const API = "joinedcontext.com/v1alpha1";
const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
/** An administrator of the organization, the person these pages are for (PF-03). */
const ADMIN = {
  role: "org-admin",
  binding: "admins",
  scope: "organization",
  rule: { kinds: ["Organization", "Project", "RoleBinding", "Person"], verbs: ["read", "propose", "approve", "delete"] },
};

const MAIN = {
  name: "main",
  declared: true,
  default: true,
  size: "small",
  features: ["ogc"],
  health: "up",
  version: "0.14.2",
  reason: null,
  spaces: ["ovzdusie/air", "doprava/stops"],
  entities: 14232,
  observedAt: "2026-10-12T08:00:00Z",
};

function costs(month: string, projects = true) {
  return {
    month,
    currency: "EUR",
    measuredAt: "2026-10-22T01:00:00Z",
    prices: { currency: "EUR", perUsd: 0.9, nodes: [{ name: "cpx42", monthly: 30 }], volumes: [], fixed: [] },
    projects: projects
      ? [
          {
            project: "doprava",
            node: 10,
            storage: 0.5,
            ai: 3.78,
            total: 14.28,
            share: 0.25,
            spaces: [{ space: "cars", bytes: 1024 ** 3, storage: 0.5 }],
            assistants: [{ assistant: "help", questions: 120, uncostedQuestions: 0, costUsd: 4.2, cost: 3.78 }],
          },
        ]
      : [],
    platform: { node: 30, storage: 4.5, total: 34.5 },
    infrastructure: 45,
    ai: projects ? 3.78 : 0,
    total: projects ? 48.78 : 45,
  };
}

const cell = (policy: string | null) => ({
  granted: policy !== null,
  rules: policy ? [{ policy, rule: 1, types: ["TrafficFlowObserved"] }] : [],
  throughOrganization: false,
  unanswered: false,
});
const MATRIX = {
  columns: [{ kind: "organization" }, { kind: "group", group: "traffic-analysts" }],
  rows: [
    { project: "helsinki", name: "counts", slug: "countszzzzzzzzzzzzzzzzzz", title: "Traffic counts", audience: "organization", organization: "helsinki", cells: [cell("staff-traffic"), cell(null)] },
  ],
  unanswered: 0,
};

const SETUP = {
  complete: false,
  steps: ["organization", "domain", "people", "project", "publishers", "policies"].map((id, at) => ({ id, done: at < 2 })),
  operator: ["branding", "loginTheme", "smtp", "backups"].map((id) => ({ id, done: id !== "smtp" })),
};

/** The REST doors these pages used to read; none may be asked any more. */
const OLD_DOORS = /\/organization\/(brokers|costs|data-access|setup|health|addresses|agents|limits|widgets\/options)$|\/readable$/;

type Answer = { status: number; value?: unknown; message?: string };

/** The page's API, each reading answered by `readings[aspect]`; returns every `/api/` path asked. */
async function stub(page: Page, readings: Record<string, Answer>): Promise<string[]> {
  const asked: string[] = [];
  await page.route("**/api/**", (route: Route) => {
    const request = route.request();
    const url = new URL(request.url());
    asked.push(url.pathname);
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (decodeURIComponent(url.pathname).startsWith("/api/v1/Organization/JoinedContext.Portal.jc_organization_read(")) {
      expect(request.headers()["odata-version"], "an OData request").toBe("4.01");
      const aspect = (url.searchParams.get("@aspect") ?? "").replaceAll("'", "");
      const answer = readings[aspect];
      if (!answer) return json({ error: { code: "not-implemented", message: `no ${aspect} here`, status: 501 } }, 501);
      if (answer.status !== 200) {
        const code = answer.status === 403 ? "forbidden" : "unavailable";
        return route.fulfill({
          status: answer.status,
          contentType: "application/json",
          headers: { "x-request-id": "req-7099" },
          body: JSON.stringify({ error: { code, message: answer.message, type: `https://joinedcontext.com/errors/${code}`, title: "Error", status: answer.status } }),
        });
      }
      return route.fulfill({ status: 200, contentType: "application/json", body: odataBody(answer.value) });
    }
    const path = url.pathname;
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "anna", name: "Anna Admin", email: "anna@hel.fi", roles: [] });
    if (path.endsWith("/permissions/me")) return json({ project: "org", bootstrap: false, grants: [ADMIN] });
    if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: "helsinki" }] });
    return json(list([]));
  });
  return asked;
}

/** The state in view, as a person sees it. */
async function shoot(page: Page, name: string, width: number, at: Locator): Promise<void> {
  expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth), "nothing scrolls sideways").toBe(0);
  await at.scrollIntoViewIfNeeded();
  await page.screenshot({ path: join(SHOTS, `${name}-${width}.png`) });
}

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.use(width < 640 ? { hasTouch: true, isMobile: true } : {});
    test.beforeEach(async ({ page }) => page.setViewportSize({ width, height: 900 }));

    test("Brokers reads the brokers aspect: filled, failed", async ({ page }) => {
      const readings: Record<string, Answer> = { brokers: { status: 200, value: { items: [MAIN] } } };
      const asked = await stub(page, readings);
      await page.goto("/organization/brokers?lang=en");
      const version = page.getByText("0.14.2").first();
      await expect(version).toBeVisible();
      await shoot(page, "brokers-filled", width, version);
      expect(await axeViolations(page)).toEqual([]);

      readings.brokers = { status: 503, message: "The broker registry did not answer; try again shortly." };
      await page.reload();
      const failed = page.getByText("The broker registry did not answer; try again shortly.");
      await expect(failed).toBeVisible();
      await expect(page.getByText(/req-7099/)).toBeVisible();
      await shoot(page, "brokers-error", width, failed);
      expect(asked.filter((path) => OLD_DOORS.test(path))).toEqual([]);
    });

    test("Costs reads the costs aspect of the month picked: filled, empty", async ({ page }) => {
      const asked = await stub(page, { costs: { status: 200, value: costs("2026-10") } });
      await page.goto("/organization/costs?lang=en");
      const row = page.getByText("doprava").first();
      await expect(row).toBeVisible();
      await shoot(page, "costs-filled", width, row);
      expect(await axeViolations(page)).toEqual([]);

      await page.unroute("**/api/**");
      const later = await stub(page, { costs: { status: 200, value: costs("2026-10", false) } });
      await page.reload();
      const empty = page.getByText(/No project has a space or an assistant yet/);
      await expect(empty).toBeVisible();
      await expect(page.getByRole("link", { name: "Open the projects" })).toBeVisible();
      await shoot(page, "costs-empty", width, empty);
      expect([...asked, ...later].filter((path) => OLD_DOORS.test(path))).toEqual([]);
    });

    test("Data access reads the dataAccess aspect: filled, empty, refused", async ({ page }) => {
      const readings: Record<string, Answer> = { dataAccess: { status: 200, value: MATRIX } };
      const asked = await stub(page, readings);
      await page.goto("/organization/data-access?lang=en");
      const row = page.getByText("Traffic counts").first();
      await expect(row).toBeVisible();
      await shoot(page, "data-access-filled", width, row);
      expect(await axeViolations(page)).toEqual([]);

      readings.dataAccess = { status: 200, value: { ...MATRIX, rows: [] } };
      await page.reload();
      const empty = page.getByText("No internal datasets yet");
      await expect(empty).toBeVisible();
      await shoot(page, "data-access-empty", width, empty);

      readings.dataAccess = { status: 403, message: "read on Person is an organization administrator's" };
      await page.reload();
      const refused = page.getByText(/Only organization administrators and people administrators see who reads/);
      await expect(refused).toBeVisible();
      await shoot(page, "data-access-refused", width, refused);
      expect(asked.filter((path) => OLD_DOORS.test(path))).toEqual([]);
    });

    test("Setup reads the setup aspect: what is done and what is left", async ({ page }) => {
      const asked = await stub(page, { setup: { status: 200, value: SETUP } });
      await page.goto("/organization/setup?lang=en");
      const checklist = page.getByRole("main");
      await expect(checklist.getByRole("listitem").first()).toBeVisible();
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.screenshot({ path: join(SHOTS, `setup-filled-${width}.png`) });
      expect(await axeViolations(page)).toEqual([]);
      expect(asked.filter((path) => OLD_DOORS.test(path))).toEqual([]);
    });
  });
}

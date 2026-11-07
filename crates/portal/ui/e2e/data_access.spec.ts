import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";

// Organization → Data access (ADR-N-067 §2.5, T-6412): the matrix of non-public Endpoints against
// everyone and each group, a group's "Datasets this group reads" and a person's "What this person
// can read", each filled, empty and refused. `vite preview` has no Portal API behind it, so the API
// is answered in the browser. JC_SHOTS=<dir> saves a desktop 1440 and a phone 400 shot of each.
// T-6423: the Endpoint's "Who can read this", loading and failing states, names instead of ids, and a
// refusal that names whom to ask, as somebody other than the administrator.

const IDENTITY = { subject: "b7c1e0f4", username: "ada", name: "Ada Admin", email: "ada@example.org", roles: [] };
const MEMBER = { subject: "c2d4e6f8", username: "mia", name: "Mia Member", email: "mia@example.org", roles: [] };
const JANA_ID = "3f2a9c1e-7b4d-4e8a-9f61-2c5d8e0b7a14";
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const columns = [{ kind: "organization" }, { kind: "group", group: "traffic-analysts" }];
const cell = (rules: { policy: string; rule: number; types: string[] }[], throughOrganization = false) => ({
  granted: rules.length > 0 || throughOrganization,
  rules,
  throughOrganization,
  unanswered: false,
});
const row = (name: string, title: string, cells: ReturnType<typeof cell>[]) => ({
  project: "helsinki",
  name,
  slug: `${name.slice(0, 4)}2m7q4tv6xh3n5jb2ryd3wcf`,
  title,
  audience: "organization",
  organization: "helsinki",
  cells,
});
const MATRIX = {
  columns,
  rows: [
    row("staff-traffic-counts", "Traffic counts (staff)", [
      cell([{ policy: "staff-traffic", rule: 1, types: ["TrafficFlowObserved"] }]),
      cell([], true),
    ]),
    row("vehicles", "Vehicles on duty", [cell([]), cell([{ policy: "staff-traffic", rule: 2, types: ["Vehicle"] }])]),
    row("internal-incidents", "Incident log", [cell([]), cell([])]),
  ],
  unanswered: 0,
};
const JANA = {
  id: JANA_ID,
  email: "jana@hel.fi",
  firstName: "Jana",
  lastName: "Nováková",
  enabled: true,
  emailVerified: true,
  requiredActions: [],
  createdAt: "2026-09-01T08:00:00Z",
  lastSeen: "2026-10-30T10:30:00Z",
  locale: "sk",
  pendingDeletion: null,
};
const GROUP = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Group",
  metadata: { name: "traffic-analysts", namespace: "org" },
  spec: { members: [{ user: JANA_ID }] },
};
const dataset = (name: string, title: string, access: string, group?: string) => ({
  project: "helsinki",
  name,
  slug: `${name.slice(0, 4)}2m7q4tv6xh3n5jb2ryd3wcf`,
  title,
  description: null,
  representations: ["ngsi-ld"],
  access,
  ...(group ? { group } : {}),
});

const ORGANIZATION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Organization",
  metadata: { name: "helsinki", namespace: "org" },
  spec: { installation: true, contacts: [{ role: "administrative", name: "Ada Admin", email: "ada@example.org" }] },
};
const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "vehicles", namespace: "helsinki", title: { en: "Vehicles on duty" } },
  spec: { contextSpaceRef: "helsinki", slug: "vehi2m7q4tv6xh3n5jb2ryd3wcf", audience: "organization", enabledRepresentations: ["ngsi-ld"] },
  status: { phase: "Live" },
};

type State = "filled" | "empty" | "refused" | "loading" | "error";

async function stubApi(page: Page, state: State): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    const refused = () => json({ title: "Forbidden", status: 403, detail: "you lack read on Person" }, 403);
    const failed = () => json({ title: "Service Unavailable", status: 503, detail: "The gateway did not answer in time. Try again in a minute." }, 503);
    // A read that never answers keeps its panel loading for the shot.
    const pending = () => new Promise<void>(() => undefined);
    const datasets = (body: () => unknown) =>
      state === "refused" ? refused() : state === "error" ? failed() : state === "loading" ? pending() : json(body());
    if (path.endsWith("/auth/me")) return json(state === "refused" ? MEMBER : IDENTITY);
    if (path === "/api/v1/preferences") return json({ firstRunDismissed: true });
    if (path === "/api/v1/projects/org/organizations") return json(list([ORGANIZATION]));
    if (path === "/api/v1/organization/people/names") return json({ items: [{ id: JANA_ID, name: "Jana Nováková" }] });
    if (path === "/api/v1/projects/helsinki/endpoints/vehicles") return json(ENDPOINT);
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/permissions/me")) {
      // A refused person administers nothing; the page tells them whose it is.
      const rule =
        state === "refused"
          ? { kinds: ["*"], verbs: ["read"] }
          : { kinds: ["RoleBinding", "Person", "Group"], verbs: ["read", "create", "update", "disable", "propose", "approve", "delete"] };
      return json({ project: "org", bootstrap: false, grants: [{ role: "r", binding: "b", rule }] });
    }
    if (path === "/api/v1/organization/data-access") {
      const endpoint = url.searchParams.get("endpoint");
      if (endpoint) {
        const only = MATRIX.rows[1];
        if (state === "empty") return json({ columns, rows: [{ ...only, cells: [cell([]), cell([])] }], unanswered: 0 });
        return datasets(() => ({ columns, rows: [only], unanswered: 0 }));
      }
      if (state === "empty") return json({ columns: columns.slice(0, 1), rows: [], unanswered: 0 });
      return datasets(() => MATRIX);
    }
    if (path === `/api/v1/organization/people/${JANA_ID}/readable`) {
      if (state !== "filled" && state !== "empty") return datasets(() => null);
      const readable =
        state === "empty"
          ? [dataset("bikes", "Bike counters", "public")]
          : [
              dataset("bikes", "Bike counters", "public"),
              dataset("staff-traffic-counts", "Traffic counts (staff)", "organization"),
              dataset("vehicles", "Vehicles on duty", "group", "traffic-analysts"),
            ];
      return json({ datasets: readable, unanswered: 0 });
    }
    if (path === `/api/v1/organization/people/${JANA_ID}`) {
      // The person page is people-admin only on the server: a refused viewer reads no name.
      if (state === "refused") return refused();
      return json({ person: JANA, groups: [{ name: "traffic-analysts" }], platformRoles: [], appRoles: [] });
    }
    if (path === "/api/v1/projects/org/groups/traffic-analysts") return json(GROUP);
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_SHOTS;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  for (const [width, label] of [[1440, "1440"], [400, "400"]] as const) {
    await page.setViewportSize({ width, height: 900 });
    const height = await page.evaluate(() => document.documentElement.scrollHeight);
    await page.setViewportSize({ width, height: Math.max(900, height) });
    await page.screenshot({ path: join(dir, `${name}-${label}.png`) });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

async function panelShot(panel: Locator, name: string): Promise<void> {
  const dir = process.env.JC_SHOTS;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  for (const [width, label] of [[1440, "1440"], [400, "400"]] as const) {
    await panel.page().setViewportSize({ width, height: 900 });
    await panel.screenshot({ path: join(dir, `${name}-${label}.png`) });
  }
  await panel.page().setViewportSize({ width: 1440, height: 900 });
}

test.describe("Organization → Data access", () => {
  test("an administrator reads the matrix and opens the rule behind a cell", async ({ page }) => {
    await stubApi(page, "filled");
    await page.goto("/organization/data-access?lang=en");
    const table = page.getByRole("table", { name: "Data access" });
    const counts = table.getByRole("row", { name: /Traffic counts/ });
    await expect(counts).toContainText("TrafficFlowObserved");
    await expect(counts).toContainText("Through everyone");
    await expect(table.getByRole("row", { name: /Vehicles on duty/ })).toContainText("staff-traffic, rule 2");
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, "matrix-filled");
    await counts.getByRole("link", { name: "Propose a change of staff-traffic, rule 1" }).click();
    await expect(page).toHaveURL(/\/projects\/helsinki\/policies\/staff-traffic\/edit/);
  });

  test("an organization with only public Endpoints says what to do", async ({ page }) => {
    await stubApi(page, "empty");
    await page.goto("/organization/data-access?lang=en");
    await expect(page.getByText("No internal datasets yet")).toBeVisible();
    await shot(page, "matrix-empty");
  });

  test("anybody else is told whom to ask and the way back", async ({ page }) => {
    await stubApi(page, "refused");
    await page.goto("/organization/data-access?lang=en");
    const refusal = page.getByRole("status").filter({ hasText: "Ask an organization administrator" });
    await expect(refusal).toContainText(/This page is for organization administrators/);
    await expect(refusal).toContainText("Ada Admin, ada@example.org");
    await expect(page.getByText("Administration is for organization administrators.")).toBeVisible();
    await shot(page, "matrix-refused");
    await refusal.getByRole("link", { name: "Back to your projects" }).click();
    await expect(page).toHaveURL(/\/projects\/helsinki\/home/);
  });

  test("the matrix says it is asking, then offers a retry when the gateway fails", async ({ page }) => {
    await stubApi(page, "loading");
    await page.goto("/organization/data-access?lang=en");
    await expect(page.getByText("Asking the gateway who reads what…").first()).toBeVisible();
    await shot(page, "matrix-loading");
    await page.unrouteAll({ behavior: "ignoreErrors" });
    await stubApi(page, "error");
    await page.goto("/organization/data-access?lang=en");
    await expect(page.getByRole("button", { name: /Retry|Try again/ }).first()).toBeVisible();
    await shot(page, "matrix-error");
  });
});

test.describe("a group's and a person's datasets", () => {
  test("the group names its member, never the id", async ({ page }) => {
    await stubApi(page, "filled");
    await page.goto("/organization/groups/traffic-analysts?lang=en");
    const members = page.locator("section", { has: page.getByRole("heading", { name: "Members" }) });
    await expect(members.getByRole("link", { name: "Jana Nováková" })).toBeVisible();
    await expect(members).not.toContainText(JANA_ID);
  });

  for (const state of ["loading", "error"] as const) {
    test(`the group and person panels while ${state}`, async ({ page }) => {
      await stubApi(page, state);
      await page.goto("/organization/groups/traffic-analysts?lang=en");
      const group = page.getByRole("region", { name: "Datasets this group reads" });
      await expect(state === "loading" ? group.getByText("Asking the gateway who reads what…").first() : group.getByRole("button", { name: /Retry|Try again/ })).toBeVisible();
      await shot(page, `group-${state}`);
      await page.goto(`/organization/people/${JANA_ID}?lang=en`);
      const person = page.getByRole("region", { name: "What this person can read" });
      await expect(state === "loading" ? person.getByText("Asking the gateway who reads what…").first() : person.getByRole("button", { name: /Retry|Try again/ })).toBeVisible();
      await shot(page, `person-${state}`);
    });
  }

  for (const state of ["filled", "empty", "refused", "loading", "error"] as const) {
    test(`the Endpoint page says who can read it (${state})`, async ({ page }) => {
      await stubApi(page, state);
      await page.route("**/api/endpoint/**", (route) => route.fulfill({ status: 404, contentType: "application/json", body: "{}" }));
      await page.goto("/projects/helsinki/endpoints/vehicles?lang=en");
      const panel = page.locator("section", { has: page.getByRole("heading", { name: "Who can read this" }) });
      const expected = {
        filled: "Members of the group traffic-analysts read it:",
        empty: "No grant to the whole organization or to a group",
        refused: "Ask an organization administrator",
        loading: "Asking the gateway who reads what…",
        error: "The gateway did not answer in time.",
      }[state];
      await expect(panel.getByText(expected).first()).toBeVisible();
      // The panel alone: the rest of the page reads stubs this spec does not answer.
      await panelShot(panel, `endpoint-${state}`);
    });
  }

  for (const state of ["filled", "empty", "refused"] as const) {

    test(`the group page lists what the group reads (${state})`, async ({ page }) => {
      await stubApi(page, state);
      await page.goto("/organization/groups/traffic-analysts?lang=en");
      const panel = page.getByRole("region", { name: "Datasets this group reads" });
      const expected = {
        filled: "Vehicles on duty",
        empty: "traffic-analysts reads no internal dataset",
        refused: "Only organization administrators and people administrators",
      }[state];
      await expect(panel.or(page.locator("main")).getByText(expected).first()).toBeVisible();
      await shot(page, `group-${state}`);
    });

    test(`the person page lists what the person can read (${state})`, async ({ page }) => {
      await stubApi(page, state);
      await page.goto(`/organization/people/${JANA_ID}?lang=en`);
      const expected = {
        filled: "Vehicles on duty",
        empty: "reads no internal dataset",
        refused: "This page is for organization administrators",
      }[state];
      await expect(page.getByText(expected).first()).toBeVisible();
      await shot(page, `person-${state}`);
    });
  }
});

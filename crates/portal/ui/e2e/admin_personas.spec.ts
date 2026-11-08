import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { greenVerdict, isCheck } from "./verdict";

// Owner order 2026-11-08 (jc-admin-ui, crates/portal/MCP-ADMIN.md): what a platform
// administrator, an organization administrator and a project administrator do through the MCP
// they also do in the Portal, with the same rights. Each persona walks organization -> project ->
// space -> endpoint -> pipeline through the UI, and meets a refusal where its role ends.
// `vite preview` has no Portal API behind it, so the API is answered in the browser as the server
// answers each person: a project outside their reach is 404 (PF-59), a write the server would
// refuse is 403 with its reason, and the UI shows that refusal; hiding a control is never the guard.

type Persona = "platform" | "organization" | "project";

const LIST = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const manifest = (kind: string, name: string, namespace: string, spec: object) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace },
  spec,
});
const CHANGE = (namespace: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0a1b2c3d", namespace },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
});

const IDENTITIES: Record<Persona, object> = {
  platform: { subject: "a1", username: "ida", name: "Ida Admin", email: "ida@hel.fi", roles: [] },
  organization: { subject: "e5", username: "eva", name: "Eva Nováková", email: "eva@zilina.sk", roles: [] },
  project: { subject: "p7", username: "petr", name: "Petr Steward", email: "petr@hel.fi", roles: [] },
};

/** The projects each person reads, and the Organization each belongs to (ADR-N-058). */
const PROJECTS = [
  { name: "helsinki", sample: false, organization: "hel", title: "Helsinki" },
  { name: "ovzdusie", sample: false, organization: "zilina", title: "Ovzdušie Žilina" },
];
const ORGANIZATIONS = [
  { name: "hel", title: "City of Helsinki" },
  { name: "zilina", title: "City of Žilina" },
];
const READS: Record<Persona, string[]> = {
  platform: ["helsinki", "ovzdusie"],
  organization: ["ovzdusie"],
  project: ["helsinki"],
};
/** The project each persona walks. */
export const HOME: Record<Persona, string> = { platform: "helsinki", organization: "ovzdusie", project: "helsinki" };

const NO_NEW_PROJECT = "A new project belongs to the installation's Organization; ask its administrators.";

/** One request to read a space of the persona's project (PL-90), as the owners' list shows it. */
const ACCESS_REQUEST = {
  id: "ar-0a1b2c3d4e5f",
  pipeline: { project: "city-kpis", name: "air-index" },
  account: "city-kpis/pl-air-index",
  endpoint: { project: "helsinki", name: "air-public" },
  space: "air",
  operations: "read",
  types: [{ type: "AirQualityObserved", attrs: ["pm10", "pm25"] }],
  until: "2027-01-31T23:59:59Z",
  reason: "Air index for the city dashboard",
  state: "pending",
  requestedBy: "jana",
  requestedAt: "2026-11-07T19:40:00Z",
  decidedBy: null,
  decidedAt: null,
  decisionReason: null,
  change: null,
};

const PROJECT_KINDS = ["ContextSpace", "Endpoint", "Pipeline", "Policy", "DataModel", "DataSource", "RoleBinding", "Role", "ServiceAccount"];
const VERBS = ["propose", "approve", "delete"];

/** `permissions/me` per project as the server answers each persona; `undefined` is 404. */
function permissions(persona: Persona, project: string): object | undefined {
  if (project === "org") {
    if (persona === "platform") {
      return {
        project: "org",
        bootstrap: false,
        grants: [{ role: "org-admin", binding: "admins", scope: "organization", rule: { kinds: ["RoleBinding", "Role", "Group", "Organization", "Project", "ServiceAccount", "Person"], verbs: VERBS } }],
      };
    }
    if (persona === "organization") {
      return {
        project: "org",
        bootstrap: false,
        grants: ["RoleBinding", "Organization", "Group"].map((kind) => ({
          role: "org-admin",
          binding: "zilina-admins",
          scope: "organization",
          organization: "zilina",
          rule: { kinds: [kind], verbs: ["propose", "approve"] },
        })),
        projects: { creation: { allowed: false, reason: NO_NEW_PROJECT } },
      };
    }
    return { project: "org", bootstrap: false, grants: [] };
  }
  if (!READS[persona].includes(project)) return undefined;
  const role = persona === "project" ? "steward" : "org-admin";
  return {
    project,
    bootstrap: false,
    ...(persona === "platform" ? {} : { projects: { creation: { allowed: false, reason: NO_NEW_PROJECT } } }),
    grants: [{ role, binding: `${project}-${role}`, scope: persona === "project" ? `project:${project}` : "organization", rule: { kinds: PROJECT_KINDS, verbs: VERBS } }],
  };
}

const MODEL = (project: string) =>
  manifest("DataModel", "air-quality", project, { source: { linkml: "id: air\nname: air\n" } });

/** One person's Portal, answered in the browser; every write is recorded. */
interface World {
  /** The access requests into the persona's spaces, or the server's refusal of that list. */
  accessRequests?: unknown[] | "refused";
}

async function stubPortal(page: Page, persona: Persona, world: World = {}): Promise<{ writes: string[] }> {
  const writes: string[] = [];
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const method = request.method();
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const refuse = (status: number, detail: string) =>
      route.fulfill({ status, contentType: "application/problem+json", body: JSON.stringify({
          // The Portal's own problem types (src/error.rs), so the page shows the hint it would live.
          type: `https://joinedcontext.com/errors/${status === 404 ? "resource-not-found" : "forbidden"}`,
          title: status === 404 ? "Resource Not Found" : "Forbidden",
          status,
          detail,
        }) });

    if (path.endsWith("/auth/me")) return json(IDENTITIES[persona]);
    if (path === "/api/v1/projects" && method === "GET") {
      return json({ ...LIST(PROJECTS.filter((p) => READS[persona].includes(p.name))), organizations: ORGANIZATIONS });
    }
    if (path === "/api/v1/projects" && method === "POST") {
      writes.push(`POST ${path}`);
      // Opening a project is the installation administrator's (ADR-N-058 section 2.2).
      if (persona !== "platform") return refuse(403, NO_NEW_PROJECT);
      return json(CHANGE("org"), 202);
    }
    const project = path.match(/^\/api\/v1\/projects\/([^/]+)/)?.[1];
    if (!project) return json(LIST([]));
    const effective = permissions(persona, project);
    if (!effective) return refuse(404, `project '${project}' not found`);
    if (isCheck(method, request.url())) return json(greenVerdict(request.postData()));
    const decision = /\/access-requests\/([^/]+)\/(approve|decline|revoke)$/.exec(path);
    if (decision && method === "POST" && Array.isArray(world.accessRequests)) {
      writes.push(`POST ${path}`);
      const state = { approve: "approved", decline: "declined", revoke: "revoked" }[decision[2] as "approve" | "decline" | "revoke"];
      world.accessRequests = world.accessRequests.map((one) =>
        (one as { id: string }).id === decision[1] ? { ...(one as object), state, decidedBy: "petr", change: "chg-0a1b2c3d" } : one,
      );
      return json(CHANGE(project), 202);
    }
    const draft = /^\/api\/v1\/projects\/([^/]+)\/drafts\/([^/]+)\/([^/]+)$/.exec(path);
    if (draft) {
      // The form saves its draft before it proposes (T-0769); the draft is not the change.
      if (method === "GET") return refuse(404, "no draft");
      if (method === "DELETE") {
        writes.push(`DELETE ${path}`);
        // A draft the server will not drop: its refusal is said beside the button.
        if (draft[3] === "locked") return refuse(403, "The draft “locked” belongs to Eva Nováková's open change chg-0a1b2c3d.");
        return json({ dropped: true });
      }
      if (method !== "PUT") return route.fulfill({ status: 204 });
      const { manifest: saved } = request.postDataJSON() as { manifest: unknown };
      return json({ project, kind: draft[2], name: draft[3], manifest: saved, touchedBy: "me", touchedKind: "person", version: 1, updatedAt: new Date().toISOString() });
    }
    if (path.endsWith("/drafts")) return json(LIST([]));
    if (method !== "GET") {
      writes.push(`${method} ${path}`);
      // The server re-checks: a project administrator writing organization settings is refused.
      if (project === "org" && persona === "project") return refuse(403, "Organization settings are the organization administrators'.");
      return json(CHANGE(project), 202);
    }
    if (path.endsWith("/permissions/me")) return json(effective);
    if (path.endsWith("/access-requests")) {
      if (world.accessRequests === "refused") return refuse(403, "Only the owners of a space of this project decide its access requests.");
      return json({ items: world.accessRequests ?? [] });
    }
    if (project === "org") {
      if (path.endsWith("/organizations")) {
        const own = persona === "organization" ? ["zilina"] : ["hel", "zilina"];
        return json(LIST(own.map((name) => manifest("Organization", name, "org", { domain: name === "hel" ? "hel.fi" : "zilina.sk", locales: ["en"], defaultLocale: "en" }))));
      }
      if (path.endsWith("/projects")) {
        return json(LIST(PROJECTS.filter((p) => READS[persona].includes(p.name)).map((p) => manifest("Project", p.name, "org", { organizationRef: p.organization }))));
      }
      return json(LIST([]));
    }
    if (path.endsWith("/datamodels")) return json(LIST([MODEL(project)]));
    if (path.endsWith("/spaces")) return json(LIST([manifest("ContextSpace", "air", project, { dataModelRef: { name: "air-quality" } })]));
    if (path.endsWith("/endpoints")) {
      return json(LIST([manifest("Endpoint", "air-public", project, { contextSpaceRef: "air", slug: "knimpgi7bnmz6khs7azlgfus7p", audience: "public" })]));
    }
    return json(LIST([]));
  });
  return { writes };
}

/** Screenshots for the report, where `JC_EVIDENCE_DIR` asks for them. */
async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  // Full height by the viewport, not `fullPage`, which cuts the sticky sidebar (T-6423).
  const size = page.viewportSize() ?? { width: 1440, height: 900 };
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  await page.setViewportSize({ width: size.width, height: Math.max(size.height, Math.min(height, 4000)) });
  await page.screenshot({ path: join(dir, `${name}.png`) });
  await page.setViewportSize(size);
}

/** Administration as a person opens it: the side link, or the account menu on a phone (T-3319). */
async function openAdministration(page: Page, project: string, width: number): Promise<void> {
  await page.goto(`/projects/${project}/spaces?lang=en`);
  if (width < 640) {
    await page.getByRole("button", { name: /Signed in as/ }).click();
    await page.getByRole("menuitem", { name: "Administration" }).click();
  } else {
    await page.getByRole("link", { name: "Administration" }).click();
  }
  await expect(page).toHaveURL(/\/organization\/settings/);
}

/** Fill a resource form's name, check it, propose it: the write the walk counts. */
async function propose(
  page: Page,
  address: string,
  name: string,
  persona: string,
  width: number,
  pick?: { field: string; option: string },
): Promise<void> {
  await page.goto(`${address}?lang=en`);
  const form = page.getByTestId("form-page");
  await expect(form).toBeVisible();
  await form.getByRole("textbox", { name: "Name", exact: true }).fill(name);
  // A field naming an existing thing is a dropdown of what the server lists for this person.
  if (pick) {
    const select = form.getByRole("combobox", { name: pick.field, exact: true });
    const option = select.locator("option", { hasText: pick.option }).first();
    await expect(option, `${pick.field}: ${(await select.locator("option").allTextContents()).join(" | ")}`).toBeAttached();
    await select.selectOption((await option.getAttribute("value")) ?? "");
  }
  await form.getByRole("button", { name: "Check" }).click();
  const submit = form.getByRole("button", { name: "Propose change" });
  await expect(submit).not.toHaveAttribute("aria-disabled", "true", { timeout: 15_000 });
  await submit.click();
  await shot(page, `${persona}-${address.split("/")[3]}-proposed-${width}`);
}

/** Space, endpoint and pipeline in one project, each a proposed change of that project. */
async function walkProject(page: Page, project: string, persona: string, width: number, writes: string[]): Promise<void> {
  await propose(page, `/projects/${project}/spaces/new`, "traffic", persona, width);
  await expect.poll(() => writes).toContain(`POST /api/v1/projects/${project}/spaces`);

  await propose(page, `/projects/${project}/endpoints/new`, "traffic-public", persona, width, { field: "Context Space", option: "air" });
  await expect.poll(() => writes).toContain(`POST /api/v1/projects/${project}/endpoints`);

  await propose(page, `/projects/${project}/pipelines/new`, "traffic-ingest", persona, width, { field: "Target endpoint", option: "air-public" });
  await expect.poll(() => writes).toContain(`POST /api/v1/projects/${project}/pipelines`);
}

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
    });

    test("a platform administrator sets up the organization, opens a project and builds space, endpoint and pipeline", async ({ page }) => {
      const { writes } = await stubPortal(page, "platform");
      await openAdministration(page, "helsinki", width);
      await expect(page.getByText("hel.fi").first()).toBeVisible();
      await shot(page, `platform-organization-${width}`);

      await page.goto("/organization/projects?lang=en");
      await page.getByRole("button", { name: "New project" }).first().click();
      const dialog = page.getByRole("dialog");
      await dialog.getByRole("textbox", { name: /Name/ }).first().fill("traffic");
      await dialog.getByRole("button", { name: "Open project" }).click();
      await expect.poll(() => writes).toContain("POST /api/v1/projects");
      await shot(page, `platform-project-opened-${width}`);
      await dialog.getByRole("button", { name: "Close" }).first().click();

      await walkProject(page, HOME.platform, "platform", width, writes);
    });

    test("an organization administrator runs its own Organization's project and is refused the installation's", async ({ page }) => {
      const { writes } = await stubPortal(page, "organization");
      await openAdministration(page, "ovzdusie", width);
      await expect(page.getByText("zilina.sk").first()).toBeVisible();
      await shot(page, `organization-own-settings-${width}`);

      await walkProject(page, HOME.organization, "organization", width, writes);

      // Opening a project is the installation's: the control says why, and the API refuses anyway.
      if (width >= 1024) {
        await page.goto("/projects/ovzdusie/spaces?lang=en");
        const create = page.getByRole("button", { name: "New project" });
        await expect(create).toHaveAttribute("aria-disabled", "true");
        await expect(create).toHaveAccessibleDescription(NO_NEW_PROJECT);
      }

      // Refusals: an installation section, and a project of another Organization (404, PF-59).
      await page.goto("/organization/brokers?lang=en");
      await expect(page.getByText("This page is for organization administrators.")).toBeVisible();
      await shot(page, `organization-refused-installation-${width}`);
      await page.goto("/projects/helsinki/spaces?lang=en");
      await expect(page.getByRole("alert")).toContainText("project 'helsinki' not found");
      await expect(page.getByRole("heading", { level: 1, name: "Context Spaces" })).toBeVisible();
      // No create control is offered in a project out of reach; the API's 404 stays the guard.
      await expect(page.getByRole("button", { name: "New Context Space" })).toHaveAttribute("aria-disabled", "true");
      await shot(page, `organization-refused-other-project-${width}`);
      expect(writes.filter((w) => w.includes("/projects/helsinki/"))).toEqual([]);
    });

    test("a project administrator builds in its project and cannot change organization settings", async ({ page }) => {
      const { writes } = await stubPortal(page, "project");
      await walkProject(page, HOME.project, "project", width, writes);

      await page.goto("/organization/settings?lang=en");
      await expect(page.getByText("This page is for organization administrators.")).toBeVisible();
      await shot(page, `project-refused-organization-${width}`);
      await page.goto("/projects/ovzdusie/spaces?lang=en");
      await expect(page.getByRole("alert")).toContainText("project 'ovzdusie' not found");
      await expect(page.getByRole("heading", { level: 1, name: "Context Spaces" })).toBeVisible();
      // No create control is offered in a project out of reach; the API's 404 stays the guard.
      await expect(page.getByRole("button", { name: "New Context Space" })).toHaveAttribute("aria-disabled", "true");
      await shot(page, `project-refused-other-project-${width}`);
      expect(writes.filter((w) => w.includes("/projects/org/") || w.includes("/projects/ovzdusie/"))).toEqual([]);
    });

    test("a project administrator decides an access request from the Inbox (jc_access_request_decide)", async ({ page }) => {
      const { writes } = await stubPortal(page, "project", { accessRequests: [ACCESS_REQUEST] });
      await page.goto("/projects/helsinki/inbox?lang=en");
      const section = page.locator("section", { has: page.getByRole("heading", { name: "Access requests" }) });
      const row = section.getByRole("listitem").filter({ hasText: "city-kpis/pl-air-index" });
      await expect(row.getByText("· in space air")).toBeVisible();
      await expect(row.getByRole("textbox", { name: "Reason for the requester" })).toHaveAccessibleDescription(/for example/);
      await shot(page, `inbox-access-filled-${width}`);
      await row.getByRole("button", { name: "Approve access for city-kpis/pl-air-index" }).click();
      await expect.poll(() => writes).toContain("POST /api/v1/projects/helsinki/access-requests/ar-0a1b2c3d4e5f/approve");
      await expect(section.getByText("chg-0a1b2c3d", { exact: true })).toBeVisible();
      // The list reads the request again: approved, so it offers Revoke and no second Approve.
      await expect(row.getByText("Approved", { exact: true })).toBeVisible();
      await expect(row.getByRole("button", { name: /^Approve access/ })).toHaveCount(0);
      await expect(row.getByText("Its grant is change chg-0a1b2c3d; access starts once another person approves it.")).toBeVisible();
      await shot(page, `inbox-access-approved-${width}`);
    });

    test("the Inbox says when nobody waits, and says the server's refusal of the list", async ({ page }) => {
      await stubPortal(page, "platform", { accessRequests: [] });
      await page.goto("/projects/helsinki/inbox?lang=en");
      await expect(page.getByText("Nobody waits for your decision on access to a space of this project.")).toBeVisible();
      await shot(page, `inbox-access-empty-${width}`);

      await page.unrouteAll({ behavior: "ignoreErrors" });
      await stubPortal(page, "organization", { accessRequests: "refused" });
      await page.goto("/projects/ovzdusie/inbox?lang=en");
      await expect(page.getByTestId("access-inbox-failed")).toContainText(
        "No access request of this project is yours to decide: Only the owners of a space of this project decide its access requests.",
      );
      await shot(page, `inbox-access-refused-${width}`);
    });

    test("a saved draft is thrown away after a second confirmation, and a refusal is said beside it (jc_draft_drop)", async ({ page }) => {
      const { writes } = await stubPortal(page, "project");
      for (const name of ["locked", "traffic"]) {
        await page.goto("/projects/helsinki/spaces/new?lang=en");
        const form = page.getByTestId("form-page");
        await form.getByRole("textbox", { name: "Name", exact: true }).fill(name);
        // The draft is saved 600 ms after typing; only a saved draft can be thrown away.
        await form.getByRole("button", { name: "Discard the draft" }).click();
        const ask = form.getByRole("group", { name: "Discard the draft" });
        await expect(ask).toContainText("Throw away the saved draft?");
        if (name === "traffic") await shot(page, `draft-discard-confirm-${width}`);
        await ask.getByRole("button", { name: "Discard", exact: true }).click();
        if (name === "locked") {
          await expect(ask.getByRole("alert")).toContainText("The draft was not discarded: The draft “locked” belongs to Eva Nováková's open change chg-0a1b2c3d.");
          await shot(page, `draft-discard-refused-${width}`);
        }
      }
      await expect.poll(() => writes.filter((w) => w.startsWith("DELETE"))).toEqual([
        "DELETE /api/v1/projects/helsinki/drafts/ContextSpace/locked",
        "DELETE /api/v1/projects/helsinki/drafts/ContextSpace/traffic",
      ]);
      // The form closes on the saved state: back on the list.
      await expect(page).toHaveURL(/\/projects\/helsinki\/spaces(\?|$)/);
    });
  });
}

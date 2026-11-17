import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test, type Locator, type Page, type Route } from "@playwright/test";
import { axeViolations } from "./axe";
import { odataBody } from "../tests/odata";
import { doorsAsRest } from "./manifestReads";

// T-7176: the stub answers permissions/me by its REST address; the page asks its Portal door.
test.beforeEach(({ page }) => doorsAsRest(page, ["portal_permissions_me", "portal_org_permissions_me", "portal_project_list"]));

// T-7103: the Group and Endpoint pages read their manifests through the Organization's OData
// service, a kind's set with $select of spec, status and metadata (api/odata.ts listManifests,
// readManifest). The journeys answer the service's own addresses, not the REST stubs
// (manifestReads.ts), at 1440 and 400 px: filled, missing and refused, and the network capture
// proves the REST manifest doors these pages read are never asked.

const SHOTS = process.env.SHOTS_DIR ?? "test-results/T-7103";
mkdirSync(SHOTS, { recursive: true });

const API = "joinedcontext.com/v1alpha1";
const JANA = "a1b2c3d4-0000-4000-8000-000000000001";
const KINDS = ["ContextSpace", "Endpoint", "Group", "Policy", "Role", "RoleBinding", "ServiceAccount"];

/** A manifest as the service's row of it with `$select=name,kind,metadata,spec,status`. */
const row = (kind: string, name: string, namespace: string, spec: unknown, status: unknown = null) => ({
  "@odata.etag": `W/"${name}-1"`,
  name,
  kind,
  metadata: { name, namespace },
  spec,
  status,
});
const STEWARDS = row("Group", "stewards", "org", { members: [{ user: JANA }] });
const BINDING = row("RoleBinding", "stewards-steward", "org", { roleRef: { kind: "Role", name: "steward" }, subjects: [{ kind: "Group", name: "stewards" }] });
const STEWARD = row("Role", "steward", "org", { rules: [{ kinds: ["Endpoint"], verbs: ["read", "propose"] }] });
const ENDPOINT = row("Endpoint", "air", "helsinki", { slug: "air-quality", contextSpaceRef: "air", representations: ["ngsi-ld"] }, { phase: "Live" });
const SPACE = row("ContextSpace", "air", "helsinki", { description: "Air sensors" }, { phase: "Live" });
const POLICY = row("Policy", "air-read", "helsinki", { contextSpaceRef: { kind: "ContextSpace", name: "air" }, assignee: { kind: "public" }, operations: ["read"] });

/** The REST manifest doors each page read itself before T-7103; none may be asked any more. */
const GROUP_DOORS = /^\/api\/v1\/projects\/(?:org\/(?:groups\/stewards|rolebindings|roles)|helsinki\/(?:apps|roles|ckaninstances))$/;
const ENDPOINT_DOORS = /^\/api\/v1\/projects\/helsinki\/(?:endpoints\/air|policies|spaces|projections|datamodels)$/;

type Answer = { status: number; body?: unknown };
const NONE: Answer = { status: 200, body: { value: [] } };

/** The service's manifest reads answered by `reads` (by the decoded path), the rest of the shell as REST; every path asked. */
async function stub(page: Page, reads: Record<string, Answer>): Promise<string[]> {
  const asked: string[] = [];
  await page.route("**/api/**", (route: Route) => {
    const url = new URL(route.request().url());
    const path = decodeURIComponent(url.pathname);
    asked.push(path);
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (url.searchParams.get("$select")?.includes("spec")) {
      const answer = reads[path] ?? NONE;
      return json(answer.body, answer.status);
    }
    if (path.includes("JoinedContext.Portal.portal_person_names")) {
      return route.fulfill({ status: 200, contentType: "application/json", body: odataBody({ items: [{ id: JANA, name: "Jana Novák" }] }) });
    }
    // The reads beside the moved ones, so a filled page shows no failed section: the group's
    // datasets (jc_organization_read dataAccess) and the gateway's query card.
    if (path.includes("JoinedContext.Portal.jc_organization_read")) {
      return route.fulfill({ status: 200, contentType: "application/json", body: odataBody({ rows: [] }) });
    }
    if (path === "/api/endpoint/air-quality/mcp") {
      const text = "Query AirQualityObserved: GET /ngsi-ld/v1/entities?type=AirQualityObserved";
      return json({ jsonrpc: "2.0", id: 1, result: { contents: [{ uri: "card://air-quality", mimeType: "text/markdown", text }] } });
    }
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "admin", name: "Ada Admin", email: "admin@example.org", roles: [] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: "org", grants: [{ role: "org-admin", binding: "admins", scope: "organization", rule: { kinds: KINDS, verbs: ["read", "propose", "approve"] } }] });
    }
    if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path === "/api/v1/preferences") return json({ firstRunDismissed: true });
    if (path.endsWith("/endpoints/air/admission")) return json({ admitted: true });
    return json({ apiVersion: API, kind: "List", items: [] });
  });
  return asked;
}

const refusal = (status: number, code: string, message: string): Answer => ({
  status,
  body: { error: { code, message, type: `https://joinedcontext.com/errors/${code}`, title: "Error", status } },
});

async function shoot(page: Page, name: string, width: number, at: Locator): Promise<void> {
  expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth), "nothing scrolls sideways").toBe(0);
  await at.scrollIntoViewIfNeeded();
  await page.screenshot({ path: join(SHOTS, `${name}-${width}.png`) });
}

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.use(width < 640 ? { hasTouch: true, isMobile: true } : {});
    test.beforeEach(async ({ page }) => page.setViewportSize({ width, height: 900 }));

    test("a group's page reads the group, its bindings and the roles from the service", async ({ page }) => {
      const asked = await stub(page, {
        "/api/v1/groups('stewards')": { status: 200, body: STEWARDS },
        "/api/v1/rolebindings": { status: 200, body: { value: [BINDING] } },
        "/api/v1/organizationRoles": { status: 200, body: { value: [STEWARD] } },
      });
      await page.goto("/organization/groups/stewards?lang=en");
      const member = page.getByRole("link", { name: "Jana Novák", exact: true }).first();
      await expect(member).toBeVisible();
      // The Organization's own Roles (organizationRoles) are what a binding may grant.
      await expect(page.locator('option[value="org/steward"]').first()).toBeAttached();
      expect(await axeViolations(page)).toEqual([]);
      await shoot(page, "group-filled", width, member);
      expect(asked.filter((path) => GROUP_DOORS.test(path))).toEqual([]);
      expect(asked).toContain("/api/v1/groups('stewards')");
    });

    test("a group the service does not hold says so, with a way back", async ({ page }) => {
      await stub(page, { "/api/v1/groups('stewards')": refusal(404, "resource-not-found", "no Group stewards you may read") });
      await page.goto("/organization/groups/stewards?lang=en");
      const said = page.getByText(/no Group stewards you may read|not found|does not exist/i).first();
      await expect(said).toBeVisible();
      await shoot(page, "group-missing", width, said);
    });

    test("an endpoint's page reads the endpoint and the project's policies and spaces from the service", async ({ page }) => {
      const asked = await stub(page, {
        "/api/v1/endpoints(project='helsinki',name='air')": { status: 200, body: ENDPOINT },
        "/api/v1/projects('helsinki')/policies": { status: 200, body: { value: [POLICY] } },
        "/api/v1/projects('helsinki')/spaces": { status: 200, body: { value: [SPACE] } },
      });
      await page.goto("/projects/helsinki/endpoints/air?lang=en");
      const heading = page.getByRole("heading", { level: 1 });
      await expect(heading).toContainText("air");
      await expect(page.getByText("air-quality").first()).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shoot(page, "endpoint-filled", width, heading);
      expect(asked.filter((path) => ENDPOINT_DOORS.test(path))).toEqual([]);
      expect(asked).toContain("/api/v1/projects('helsinki')/policies");
    });

    test("an endpoint the person may not read shows the service's refusal", async ({ page }) => {
      await stub(page, {
        "/api/v1/endpoints(project='helsinki',name='air')": refusal(403, "forbidden", "Your roles in helsinki do not let you read Endpoint air."),
      });
      await page.goto("/projects/helsinki/endpoints/air?lang=en");
      const said = page.getByText("Your roles in helsinki do not let you read Endpoint air.").first();
      await expect(said).toBeVisible();
      await shoot(page, "endpoint-refused", width, said);
    });
  });
}

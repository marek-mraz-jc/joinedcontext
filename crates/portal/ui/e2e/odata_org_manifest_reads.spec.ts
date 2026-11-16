import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test, type Locator, type Page, type Route } from "@playwright/test";
import { axeViolations } from "./axe";

// T-7104: a project's settings and the Organization's read the Project manifest from the service's
// `projects` set and the Organization's own from its `Organization` singleton, each with $select
// of spec, status and metadata (api/odata.ts readManifest, listOrganizations). The journeys answer
// the service's own addresses at 1440 and 400 px: filled, a project whose manifest the person may
// not read, and an address that serves several Organizations, where the installation's
// administrators still choose one (`?org=`) from the REST list.

const SHOTS = process.env.SHOTS_DIR ?? "test-results/T-7104";
mkdirSync(SHOTS, { recursive: true });

const API = "joinedcontext.com/v1alpha1";
const KINDS = ["Project", "Organization", "ContextSpace", "Endpoint", "Group", "Role", "RoleBinding"];

const HELSINKI = {
  "@odata.etag": 'W/"helsinki-1"',
  name: "helsinki",
  metadata: { name: "helsinki", namespace: "org", title: "Helsinki open data", description: "The city's sensors and registers" },
  spec: { organizationRef: "hel" },
  status: null,
};
const HIDDEN = { name: "helsinki", metadata: null, spec: null, status: null };
const organization = (name: string, domain: string) => ({
  apiVersion: API,
  kind: "Organization",
  metadata: { name, namespace: "org", title: name },
  spec: { domain, projects: { visibility: "organization" } },
});
const HEL = { "@odata.etag": 'W/"hel-1"', name: "hel", ...organization("hel", "hel.fi") };

/** The REST manifest doors these pages read before T-7104; none may be asked on one Organization's address. */
const DOORS = /^\/api\/v1\/projects\/org\/(?:organizations|projects(?:\/helsinki)?)$/;

type Answer = { status: number; body?: unknown };

/** The service's reads answered by `reads` (by the decoded path), the rest of the shell as REST; every path asked. */
async function stub(page: Page, reads: Record<string, Answer>, organizations: unknown[] = []): Promise<string[]> {
  const asked: string[] = [];
  await page.route("**/api/**", (route: Route) => {
    const url = new URL(route.request().url());
    const path = decodeURIComponent(url.pathname);
    asked.push(path);
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (url.searchParams.get("$select")?.includes("spec")) {
      const answer = reads[path] ?? { status: 200, body: { value: [] } };
      return json(answer.body, answer.status);
    }
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "admin", name: "Ada Admin", email: "admin@example.org", roles: [] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: "org", grants: [{ role: "org-admin", binding: "admins", scope: "organization", rule: { kinds: KINDS, verbs: ["read", "propose", "approve", "delete"] } }] });
    }
    if (path === "/api/v1/projects") {
      const owners = organizations.length > 1 ? { organizations: [{ name: "hel" }, { name: "zilina" }] } : {};
      return json({ apiVersion: API, kind: "ProjectList", items: [{ name: "helsinki", organization: "hel" }], ...owners });
    }
    if (path === "/api/v1/projects/org/organizations") return json({ apiVersion: API, kind: "List", items: organizations });
    if (path === "/api/v1/preferences") return json({ firstRunDismissed: true });
    return json({ apiVersion: API, kind: "List", items: [] });
  });
  return asked;
}

const several: Answer = {
  status: 404,
  body: { error: { code: "several-organizations", message: "this address serves several Organizations; read one at its own host" } },
};

async function shoot(page: Page, name: string, width: number, at: Locator): Promise<void> {
  expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth), "nothing scrolls sideways").toBe(0);
  await at.scrollIntoViewIfNeeded();
  await page.screenshot({ path: join(SHOTS, `${name}-${width}.png`), fullPage: true });
}

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.use(width < 640 ? { hasTouch: true, isMobile: true } : {});
    test.beforeEach(async ({ page }) => page.setViewportSize({ width, height: 900 }));

    test("a project's settings read the Project and its Organization from the service", async ({ page }) => {
      const asked = await stub(page, {
        "/api/v1/projects('helsinki')": { status: 200, body: HELSINKI },
        "/api/v1/Organization": { status: 200, body: HEL },
      });
      await page.goto("/projects/helsinki/settings?lang=en");
      const title = page.getByText("Helsinki open data").first();
      await expect(title).toBeVisible();
      await expect(page.getByText("The city's sensors and registers").first()).toBeVisible();
      await expect(page.getByText("hel.fi").first()).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shoot(page, "project-settings-filled", width, title);
      expect(asked.filter((path) => DOORS.test(path))).toEqual([]);
      expect(asked).toEqual(expect.arrayContaining(["/api/v1/projects('helsinki')", "/api/v1/Organization"]));
    });

    test("a project whose manifest the person may not read says it is not found", async ({ page }) => {
      await stub(page, {
        "/api/v1/projects('helsinki')": { status: 200, body: HIDDEN },
        "/api/v1/Organization": { status: 200, body: HEL },
      });
      await page.goto("/projects/helsinki/settings?lang=en");
      const said = page.getByText("Your roles do not let you read the settings of helsinki, or it no longer exists.").first();
      await expect(said).toBeVisible();
      await shoot(page, "project-settings-refused", width, said);
    });

    test("the Organization's settings read its manifest from the singleton", async ({ page }) => {
      const asked = await stub(page, { "/api/v1/Organization": { status: 200, body: HEL } });
      await page.goto("/organization/settings?lang=en");
      const domain = page.getByText("hel.fi").first();
      await expect(domain).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shoot(page, "organization-settings-filled", width, domain);
      expect(asked.filter((path) => DOORS.test(path))).toEqual([]);
    });

    test("an address of several Organizations still lets the administrator choose one", async ({ page }) => {
      const asked = await stub(page, { "/api/v1/Organization": several }, [organization("hel", "hel.fi"), organization("zilina", "zilina.sk")]);
      await page.goto("/organization/settings?org=zilina&lang=en");
      const domain = page.getByText("zilina.sk").first();
      await expect(domain).toBeVisible();
      await expect(page.getByText("hel.fi")).toHaveCount(0);
      await shoot(page, "organization-settings-several", width, domain);
      expect(asked).toContain("/api/v1/projects/org/organizations");
    });
  });
}

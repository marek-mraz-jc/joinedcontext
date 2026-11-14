import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// An Organization's administrator asks every member for a second factor (T-6895): Settings shows
// the rule in force in the Sign-in section, Edit → Policies → Sign-in offers Required or Optional,
// and Required goes out as a proposed Change of `spec.policies.signIn.secondFactor`. Under a
// deployment floor of `required` the row says the rule is inherited. A proposal the server
// refuses shows its reason. `vite preview` has no Portal API behind it, so the API is answered in
// the browser as the server answers Žilina's administrator.

const API = "joinedcontext.com/v1alpha1";
const IDENTITY = { subject: "e5a2", username: "eva", name: "Eva Nováková", email: "eva@zilina.sk", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
const ZILINA = {
  apiVersion: API,
  kind: "Organization",
  metadata: { name: "zilina", namespace: "org" },
  spec: { domain: "zilina.sk", locales: ["sk", "en"], defaultLocale: "sk" },
};
const PERMISSIONS = {
  project: "org",
  bootstrap: false,
  grants: ["RoleBinding", "Organization", "Group"].map((kind) => ({
    role: "org-admin",
    binding: "zilina-admins",
    scope: "organization",
    organization: "zilina",
    rule: { kinds: [kind], verbs: ["propose", "approve"] },
  })),
};
const REFUSAL = {
  type: "https://joinedcontext.com/problems/forbidden",
  title: "Forbidden",
  status: 403,
  detail:
    "eva may not propose a change to Organization zilina: the organization-admin binding ran out on 2026-11-14. Ask another administrator of zilina to renew it.",
};

async function stubApi(page: Page, floor: string, proposed: unknown[], refuse = false): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") {
      return json({
        ...list([{ name: "ovzdusie", sample: false, organization: "zilina", title: "Ovzdušie Žilina" }]),
        organizations: [{ name: "zilina", title: { sk: "Mesto Žilina", en: "City of Žilina" } }],
      });
    }
    if (path === "/api/v1/organization/limits") {
      return json({
        entries: [
          {
            path: "spec.limits.signIn.sessionIdleMinutes",
            section: "signIn",
            security: true,
            default: 60,
            min: 5,
            max: 600,
            value: null,
            origin: "default",
          },
        ],
        projects: [],
        secondFactorFloor: floor,
      });
    }
    if (path === "/api/v1/projects/org/permissions/me") return json(PERMISSIONS);
    if (path === "/api/v1/projects/org/organizations") return json(list([ZILINA]));
    if (/\/drafts\//.test(path)) {
      if (request.method() !== "PUT") return route.fulfill({ status: 204 });
      const { manifest } = request.postDataJSON() as { manifest: unknown };
      return json({ project: "org", kind: "Organization", name: "zilina", manifest, version: 1, updatedAt: new Date().toISOString() });
    }
    if (path === "/api/v1/projects/org/organizations/zilina") {
      if (request.method() === "GET") return json(ZILINA);
      if (url.searchParams.get("dryRun")) return json({ valid: true });
      if (refuse) return route.fulfill({ status: 403, contentType: "application/problem+json", body: JSON.stringify(REFUSAL) });
      proposed.push(request.postDataJSON());
      return json({ apiVersion: API, kind: "Change", metadata: { name: "chg-2fa0c1", namespace: "org" }, spec: {}, status: { phase: "Proposed" } }, 201);
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), animations: "disabled" });
}

const row = (page: Page) => page.getByRole("row", { name: /Second factor at sign-in/ });

/** Opens Edit → Policies → Sign-in and chooses `value` for the second factor. */
async function choose(page: Page, value: string): Promise<void> {
  await page.getByRole("button", { name: "Edit zilina.sk" }).click();
  const policies = page.getByRole("group", { name: "Policies" });
  await policies.locator("summary").first().click();
  const signIn = policies.getByRole("group", { name: "Sign-in" });
  if ((await signIn.locator("summary").count()) > 0) await signIn.locator("summary").first().click();
  const field = page.getByRole("combobox", { name: /Second factor at sign-in/ });
  await field.selectOption({ label: value });
  await expect(page.getByText(/are asked to set up an authenticator app at their next sign-in/).locator("visible=true").first()).toBeVisible();
  await field.evaluate((element) => element.scrollIntoView({ block: "center" }));
}

test.describe("an Organization's second factor", () => {
  for (const width of [1440, 400]) {
    test(`shows the deployment's floor as inherited at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, "required", []);
      await page.goto("/organization/settings?lang=en");
      await expect(row(page)).toContainText("Required (inherited from the deployment)");
      await expect(row(page)).toContainText("Optional here has no effect");
      await row(page).evaluate((element) => element.scrollIntoView({ block: "center" }));
      await shot(page, `second-factor-inherited-${width}`);
    });

    test(`is proposed as required from Settings at ${width} px`, async ({ page }) => {
      const proposed: unknown[] = [];
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, "optional", proposed);
      await page.goto("/organization/settings?lang=en");
      await expect(row(page)).toContainText("Optional (deployment default)");
      await choose(page, "Required");
      await shot(page, `second-factor-form-${width}`);
      await page.getByRole("button", { name: "Propose change" }).click();
      await expect.poll(() => proposed.length).toBe(1);
      const spec = (proposed[0] as { spec: { policies?: { signIn?: { secondFactor?: string } } } }).spec;
      expect(spec.policies?.signIn?.secondFactor).toBe("required");
      await page.getByText("chg-2fa0c1").first().evaluate((element) => element.scrollIntoView({ block: "center" }));
      await shot(page, `second-factor-required-${width}`);
    });

    test(`says why the server refused the proposal at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, "optional", [], true);
      await page.goto("/organization/settings?lang=en");
      await choose(page, "Required");
      await page.getByRole("button", { name: "Propose change" }).click();
      const reason = page.getByText(/organization-admin binding ran out/).first();
      await expect(reason).toBeVisible();
      await reason.evaluate((element) => element.scrollIntoView({ block: "center" }));
      await shot(page, `second-factor-refused-${width}`);
    });
  }
});

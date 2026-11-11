import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// An Organization's administrator sets how its Portal looks (UIX-16, T-6772): Settings → Edit →
// "How it looks", a colour that is CSS is refused beside its field, a hex colour goes out as a
// proposed Change of `spec.appearance`, and the Portal on that Organization's host draws the
// branding the server answers for it. `vite preview` has no Portal API behind it, so the API is
// answered in the browser as the server answers Žilina's administrator.

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
/** What `GET /api/v1/branding` answers on Žilina's host once its appearance is merged. */
const BRANDING = {
  instanceName: "Žilina dáta",
  shortName: "Žilina",
  colours: { primary: "#0b5394", secondary: "#0f766e", accent: "#f59e0b", background: "#ffffff", text: "#0f172a" },
  languages: { default: "en", offered: ["sk", "en"] },
  primaryForeground: "#ffffff",
  primaryDark: "#7aa5d6",
  primaryForegroundDark: "#0f172a",
  hiddenSections: [],
};

async function stubApi(page: Page, proposed: unknown[]): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/branding") return json(BRANDING);
    if (path === "/api/v1/projects") {
      return json({
        ...list([{ name: "ovzdusie", sample: false, organization: "zilina", title: "Ovzdušie Žilina" }]),
        organizations: [{ name: "zilina", title: { sk: "Mesto Žilina", en: "City of Žilina" } }],
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
      proposed.push(request.postDataJSON());
      return json({ apiVersion: API, kind: "Change", metadata: { name: "chg-0b5394aa", namespace: "org" }, spec: {}, status: { phase: "Proposed" } }, 201);
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

test.describe("an Organization's appearance", () => {
  for (const width of [1440, 400]) {
    test(`is proposed from Settings and drawn on its Portal at ${width} px`, async ({ page }) => {
      const proposed: unknown[] = [];
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, proposed);
      await page.goto("/organization/settings?lang=en");
      await expect(page.getByText("zilina.sk").first()).toBeVisible();
      // The Portal on Žilina's host is drawn in Žilina's colour.
      const brand = await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--portal-color-primary"));
      expect(brand.trim().toLowerCase()).toContain("#0b5394");
      await shot(page, `appearance-portal-${width}`);

      await page.getByRole("button", { name: "Edit zilina.sk" }).click();
      await page.getByRole("group", { name: "How it looks" }).locator("summary").click();
      await page.getByRole("textbox", { name: /^Name/ }).fill("Žilina dáta");
      const primary = page.getByRole("textbox", { name: /^Primary/ });
      await primary.fill("red;}body{background:url(//x)}");
      await page.getByRole("button", { name: "Propose change" }).click();
      await expect(page.getByText("Value does not match pattern").first()).toBeVisible();
      expect(proposed).toHaveLength(0);
      await shot(page, `appearance-refused-${width}`);

      await primary.fill("#0b5394");
      await shot(page, `appearance-form-${width}`);
      await page.getByRole("button", { name: "Propose change" }).click();
      await expect.poll(() => proposed.length).toBe(1);
      const spec = (proposed[0] as { spec: { appearance?: { instanceName?: string; colours?: { primary?: string } } } }).spec;
      expect(spec.appearance?.instanceName).toBe("Žilina dáta");
      expect(spec.appearance?.colours?.primary).toBe("#0b5394");
      // The form never sends the empty models list it renders for an Organization naming none.
      expect((spec as { policies?: { agents?: { models?: unknown } } }).policies?.agents?.models).toBeUndefined();
      await shot(page, `appearance-proposed-${width}`);
    });
  }
});

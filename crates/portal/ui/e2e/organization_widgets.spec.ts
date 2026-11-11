import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

// An Organization's home page Dashboards (UIX-16, T-6779): its administrator picks them on
// Settings → Edit → "How it looks" from the Dashboards the server offers them, the server refuses
// one the administrator may not read beside the field, and a project home shows each Dashboard
// the server lets the viewer read, one click to it. `vite preview` has no Portal API behind it,
// so the API is answered in the browser as the server answers Žilina's administrator.

const API = "joinedcontext.com/v1alpha1";
const IDENTITY = { subject: "e5a2", username: "eva", name: "Eva Nováková", email: "eva@zilina.sk", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
const ZILINA = {
  apiVersion: API,
  kind: "Organization",
  metadata: { name: "zilina", namespace: "org" },
  spec: { domain: "zilina.sk", locales: ["sk", "en"], defaultLocale: "sk", appearance: { shortName: "Žilina" } },
};
const ADMIN = {
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
const VIEWER = {
  project: "ovzdusie",
  bootstrap: false,
  grants: [{ role: "viewer", binding: "readers", scope: "project", rule: { kinds: ["ContextSpace", "Endpoint", "Dashboard"], verbs: ["read"] } }],
};
const OPTIONS = [
  { reference: "ovzdusie/prehlad", project: "ovzdusie", name: "prehlad", title: { sk: "Prehľad ovzdušia", en: "Air quality overview" }, pages: 2 },
  { reference: "doprava/nehody", project: "doprava", name: "nehody", title: "Traffic accidents", pages: 1 },
];

interface Stub {
  proposed: unknown[];
  /** The Dashboards the server offers; the default two. */
  options?: unknown[];
  /** The 422 the server answers a proposal, when it refuses one. */
  refuse?: string;
}

async function stubApi(page: Page, stub: Stub): Promise<void> {
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
    if (path === "/api/v1/organization/widgets") return json({ widgets: OPTIONS });
    if (path === "/api/v1/organization/widgets/options") {
      return json({ options: url.searchParams.get("organization") === "zilina" ? (stub.options ?? OPTIONS) : [] });
    }
    if (path === "/api/v1/projects/org/permissions/me") return json(ADMIN);
    if (path === "/api/v1/projects/ovzdusie/permissions/me") return json(VIEWER);
    if (path === "/api/v1/projects/org/organizations" && request.method() === "GET") return json(list([ZILINA]));
    if (/\/drafts\//.test(path)) {
      if (request.method() !== "PUT") return route.fulfill({ status: 204 });
      const { manifest } = request.postDataJSON() as { manifest: unknown };
      return json({ project: "org", kind: "Organization", name: "zilina", manifest, version: 1, updatedAt: new Date().toISOString() });
    }
    // The form's own check posts to the kind's list, the proposal goes to the resource.
    if (path === "/api/v1/projects/org/organizations/zilina" || (path === "/api/v1/projects/org/organizations" && url.searchParams.get("dryRun"))) {
      if (request.method() === "GET") return json(ZILINA);
      if (stub.refuse) {
        // The server's dry run: a red verdict whose finding names the member (dry_run.rs led_by_its_member).
        const [path, message] = stub.refuse.split(": ");
        if (url.searchParams.get("dryRun")) {
          return json({ valid: false, verdict: { ok: false, inputDigest: "d", checkedAt: new Date().toISOString(), findings: [{ level: "error", path, message }] } });
        }
        return json({ type: "about:blank", title: "Unprocessable Entity", status: 422, detail: stub.refuse, errors: [stub.refuse] }, 422);
      }
      if (url.searchParams.get("dryRun")) return json({ valid: true });
      stub.proposed.push(request.postDataJSON());
      return json({ apiVersion: API, kind: "Change", metadata: { name: "chg-6779aa", namespace: "org" }, spec: {}, status: { phase: "Proposed" } }, 201);
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

async function openWidgets(page: Page): Promise<void> {
  await page.goto("/organization/settings?lang=en");
  await page.getByRole("button", { name: "Edit zilina.sk" }).click();
  await page.getByRole("group", { name: "How it looks" }).locator("summary").click();
}

test.describe("an Organization's home page dashboards", () => {
  for (const width of [1440, 400]) {
    test(`are shown on a project home and open their dashboard at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, { proposed: [] });
      await page.goto("/projects/ovzdusie/home?lang=en");
      const section = page.getByRole("region", { name: "Dashboards on the home page" });
      await expect(section.getByRole("link", { name: /Air quality overview/ })).toBeVisible();
      await expect(section.getByRole("link", { name: /Traffic accidents/ })).toContainText("doprava · 1 page");
      await shot(page, `widgets-home-${width}`);
      await section.getByRole("link", { name: /Air quality overview/ }).click();
      await expect(page).toHaveURL(/\/projects\/ovzdusie\/dashboards\?dashboard=prehlad/);
    });

    test(`are chosen from the server's list and proposed at ${width} px`, async ({ page }) => {
      const stub: Stub = { proposed: [] };
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, stub);
      await openWidgets(page);
      // One box per Dashboard the server offers this person.
      const field = page.locator("#root_appearance_homeWidgets");
      await expect(field.getByRole("checkbox")).toHaveCount(2);
      await field.getByRole("checkbox", { name: "Air quality overview (ovzdusie/prehlad)" }).check();
      await expect(page.getByText(/each person sees the ones they may open/)).toBeVisible();
      await expect(field).toContainText("1 of 6 chosen");
      await field.scrollIntoViewIfNeeded();
      await page.screenshot({ path: process.env.JC_EVIDENCE_DIR ? join(process.env.JC_EVIDENCE_DIR, `widgets-form-${width}.png`) : undefined });
      await page.getByRole("button", { name: "Propose change" }).click();
      await expect.poll(() => stub.proposed.length).toBe(1);
      const spec = (stub.proposed[0] as { spec: { appearance?: { homeWidgets?: string[] } } }).spec;
      expect(spec.appearance?.homeWidgets).toEqual(["ovzdusie/prehlad"]);
    });

    test(`refused by the server names the field at ${width} px`, async ({ page }) => {
      const stub: Stub = {
        proposed: [],
        refuse: "spec.appearance.homeWidgets[0]: doprava/nehody is not a Dashboard of this Organization's projects that you may read; choose one the list offers",
      };
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, stub);
      await openWidgets(page);
      const field = page.locator("#root_appearance_homeWidgets");
      await field.getByRole("checkbox", { name: "Traffic accidents (doprava/nehody)" }).check();
      await page.getByRole("button", { name: "Propose change" }).click();
      // The refusal stands beside the field it is about.
      await expect(page.locator("#root_appearance_homeWidgets__error")).toContainText("doprava/nehody is not a Dashboard of this Organization's projects that you may read");
      expect(stub.proposed).toHaveLength(0);
      await field.scrollIntoViewIfNeeded();
      await page.screenshot({ path: process.env.JC_EVIDENCE_DIR ? join(process.env.JC_EVIDENCE_DIR, `widgets-refused-${width}.png`) : undefined });
    });

    test(`says what to do first when no dashboard is offered at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, { proposed: [], options: [] });
      await openWidgets(page);
      await expect(page.getByText("No dashboard to offer yet. Open a project of this organization, go to Dashboards and make one; it is offered here next time.")).toBeVisible();
      // Nothing to add by hand: a typed name would only be refused.
      await expect(page.locator("#root_appearance_homeWidgets").getByRole("button", { name: /Add/ })).toHaveCount(0);
      await page.getByText(/No dashboard to offer yet/).scrollIntoViewIfNeeded();
      await page.screenshot({ path: process.env.JC_EVIDENCE_DIR ? join(process.env.JC_EVIDENCE_DIR, `widgets-empty-${width}.png`) : undefined });
    });
  }
});

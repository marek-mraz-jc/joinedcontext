import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { greenVerdict, isCheck } from "./verdict";

// PL-94, T-4893: the pipeline editor offers only Endpoints the person reads through, and a
// pipeline the server refuses shows "No access to …" with what to ask for. `vite preview` has no
// Portal behind it: the API is answered in the browser, everything above it is the real page.

const SHOTS = process.env.JC_SHOTS_DIR;

const IDENTITY = { subject: "d3v", username: "dev", name: "Dana Developer", email: "dev@hel.fi", roles: [] };
const BRANDING = {
  instanceName: "joinedcontext",
  shortName: "joinedcontext",
  city: "Helsinki",
  organisation: "City of Helsinki",
  orgDomain: "hel.fi",
  domain: "portal.hel.fi",
  contactEmail: "data@hel.fi",
  licenseDefault: "CC-BY-4.0",
  logo: "",
  favicon: "",
  colours: { primary: "#1d4ed8", secondary: "#0f766e", accent: "#f59e0b", background: "#ffffff", text: "#0f172a" },
  fonts: { heading: "system-ui, sans-serif", body: "system-ui, sans-serif" },
  languages: { default: "en", offered: ["en"] },
  primaryForeground: "#ffffff",
};
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const endpoint = (name: string, space: string, audience: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name, namespace: "helsinki" },
  spec: { contextSpaceRef: space, slug: `${name}-k7m2qz4tv6xh3n5jb2ry`, audience },
});
/** Dana proposes Pipelines in helsinki and reads the entities of space `air` only. */
const PERMISSIONS = {
  project: "helsinki",
  bootstrap: false,
  grants: [
    { role: "pipeline-editor", binding: "dana-pipelines", scope: "project:helsinki", rule: { kinds: ["Pipeline", "DataSource"], verbs: ["propose"] } },
    { role: "space-reader", binding: "dana-air", scope: "contextSpace:air", space: "air", rule: { kinds: ["Entity"], verbs: ["read"] } },
  ],
};
const REFUSAL =
  "No access to source Endpoint parking-internal: a pipeline reads and writes only what the person who may propose it reads and writes, ask for read on the source or write on the target first (PL-94)";

async function stubApi(page: Page, refuse: boolean): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path.endsWith("/branding")) return json(BRANDING);
    if (path.endsWith("/permissions/me")) return json(PERMISSIONS);
    if (isCheck(request.method(), request.url())) return json(greenVerdict(request.postData()));
    if (request.method() === "PUT" && path.includes("/drafts/")) {
      const { manifest } = JSON.parse(request.postData() ?? "{}") as { manifest: unknown };
      return json({ project: "helsinki", kind: "Pipeline", name: "copy", manifest, touchedBy: "dev", touchedKind: "person", version: 1, updatedAt: new Date().toISOString() });
    }
    if (/\/ops\/jc_pipeline_/.test(path)) return json({ records: [], fields: [], count: 0, truncated: false, errors: [] });
    if (request.method() === "POST" && path.endsWith("/pipelines")) {
      return refuse
        ? json({ type: "about:blank", title: "Forbidden", status: 403, detail: REFUSAL }, 403)
        : json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-1", namespace: "helsinki" }, status: { lane: "yellow", phase: "PendingApproval" } }, 202);
    }
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/endpoints")) {
      return json(list([endpoint("air-internal", "air", "internal"), endpoint("parking-internal", "parking", "internal"), endpoint("public-bikes", "bikes", "public")]));
    }
    return json(list([]));
  });
}

async function openForm(page: Page) {
  await page.goto("/projects/helsinki/pipelines?lang=en");
  await page.getByRole("button", { name: "New pipeline" }).first().click();
  const dialog = page.getByTestId("form-page");
  await dialog.getByLabel(/^Name/).fill("copy");
  return dialog;
}

for (const [width, height] of [[1440, 900], [400, 860]] as const) {
  test.describe(`pipeline reach at ${width}`, () => {
    test.use({ viewport: { width, height } });

    test("offers only the Endpoints the person reads through", async ({ page }) => {
      await stubApi(page, false);
      const dialog = await openForm(page);
      const target = dialog.getByLabel(/^Target endpoint/);
      await expect(target.locator("option", { hasText: "air-internal" })).toHaveCount(1);
      await expect(target.locator("option", { hasText: "public-bikes" })).toHaveCount(1);
      await expect(target.locator("option", { hasText: "parking-internal" })).toHaveCount(0);
      await target.selectOption({ label: "air-internal" });
      await target.scrollIntoViewIfNeeded();
      if (SHOTS) await page.screenshot({ path: `${SHOTS}/pipeline-reach-picker-${width}.png`, fullPage: false });
    });

    test("shows the server's refusal with what to ask for", async ({ page }) => {
      await stubApi(page, true);
      const dialog = await openForm(page);
      await dialog.getByLabel(/^Target endpoint/).selectOption({ label: "air-internal" });
      await expect(dialog.getByTestId("draft-verdict")).toContainText("Checked");
      await dialog.getByRole("button", { name: "Propose change" }).click();
      const refusal = page.getByText(/No access to source Endpoint parking-internal/);
      await expect(refusal).toBeVisible();
      await refusal.scrollIntoViewIfNeeded();
      if (SHOTS) await page.screenshot({ path: `${SHOTS}/pipeline-reach-refusal-${width}.png`, fullPage: false });
    });
  });
}

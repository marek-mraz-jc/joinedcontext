import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { digestOf } from "../src/api/digest";
import { greenVerdict, isCheck } from "./verdict";

// T-5040 (PL-111, ADR-N-057 decision 5): a Pipeline aimed at an Endpoint that binds only its
// policyRef gets a red verdict on its target field, saying what to change; aimed at the space's
// Endpoint without policyRef it goes green. The API is answered in the browser as in
// pipeline_editor.spec.ts; the red answer is the shape the Portal's dry run sends (refused_check).
// Shots at 1440 and 400 go to test-results/unmade-writes.

const IDENTITY = { subject: "b7c1e0f4", username: "jana", name: "Jana Kováčová", email: "jana@example.org", roles: ["portal-editor"] };
const BRANDING = {
  instanceName: "joinedcontext", shortName: "joinedcontext", city: "Banská Bystrica", organisation: "Mesto Banská Bystrica",
  orgDomain: "banskabystrica.sk", domain: "portal.banskabystrica.sk", contactEmail: "data@banskabystrica.sk",
  licenseDefault: "CC-BY-4.0", logo: "", favicon: "",
  colours: { primary: "#1d4ed8", secondary: "#0f766e", accent: "#f59e0b", background: "#ffffff", text: "#0f172a" },
  fonts: { heading: "system-ui, sans-serif", body: "system-ui, sans-serif" },
  languages: { default: "en", offered: ["en"] },
  primaryForeground: "#ffffff",
};
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const endpoint = (name: string, slug: string, policyRef?: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name, namespace: "ovzdusie" },
  spec: { contextSpaceRef: "ovzdusie", slug, audience: "public", ...(policyRef ? { policyRef } : {}) },
});
const ENDPOINTS = list([
  endpoint("public-air", "k7m2qz4tv6xh3n5jb2ryd3wcfa", "urn:ngsi-ld:Policy:banskabystrica.sk:ovzdusie-ovzdusie:public-read"),
  endpoint("air-write", "bq3kz7xw2mdv6ksb3ncf5arw2y"),
]);
const REFUSAL =
  "Pipeline stations output 1 writes as pl-stations, but its targetEndpoint grants that account no write (public-air binds only its policyRef urn:ngsi-ld:Policy:banskabystrica.sk:ovzdusie-ovzdusie:public-read), so every write answers 404: aim it at an Endpoint of the space without policyRef (PL-20, ADR-N-057)";

function redVerdict(postData: string | null): unknown {
  const manifest = { ...(JSON.parse(postData ?? "{}") as Record<string, unknown>) };
  delete manifest.draft;
  return {
    valid: false,
    lane: "red",
    verdict: {
      ok: false,
      findings: [{ level: "error", path: "spec.outputs.0.targetEndpoint", message: REFUSAL }],
      checkedAt: new Date().toISOString(),
      inputDigest: digestOf(manifest),
    },
  };
}

async function stubApi(page: Page): Promise<string[]> {
  const checked: string[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path.endsWith("/branding")) return json(BRANDING);
    if (isCheck(request.method(), request.url())) {
      const body = request.postData() ?? "";
      checked.push(body);
      return json(body.includes(":public-air") ? redVerdict(body) : greenVerdict(body));
    }
    const draft = /^\/api\/v1\/projects\/([^/]+)\/drafts\/([^/]+)\/([^/]+)$/.exec(path);
    if (draft && request.method() === "PUT") {
      const { manifest } = JSON.parse(request.postData() ?? "{}") as { manifest: unknown };
      const [, project, kind, name] = draft;
      return json({ project, kind, name, manifest, touchedBy: "jana", touchedKind: "person", version: 1, updatedAt: new Date().toISOString() });
    }
    if (/\/ops\/jc_pipeline_(sample_source|try_mapping|validate)$/.test(path)) {
      return json({ records: [], fields: [], count: 0, truncated: false, errors: [] });
    }
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "ovzdusie" }] });
    if (path.endsWith("/endpoints")) return json(ENDPOINTS);
    if (path.endsWith("/spaces"))
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "ovzdusie", namespace: "ovzdusie" }, spec: {} }]));
    return json(list([]));
  });
  return checked;
}

const SHOTS = join(process.cwd(), "test-results", "unmade-writes");

async function shot(page: Page, name: string): Promise<void> {
  mkdirSync(SHOTS, { recursive: true });
  await page.screenshot({ path: join(SHOTS, `${name}-${page.viewportSize()?.width ?? 0}.png`), fullPage: true });
}

for (const width of [1440, 400]) {
  test.describe(`a write nobody can make at ${width}`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("a pipeline aimed at a policyRef Endpoint is red on its target, the space's write Endpoint green", async ({ page }) => {
      const checked = await stubApi(page);
      await page.goto("/projects/ovzdusie/pipelines?lang=en");
      await page.getByRole("button", { name: "New pipeline" }).first().click();
      const dialog = page.getByTestId("form-page");
      await dialog.getByLabel(/^Name/).fill("stations");
      await dialog.getByLabel(/^Target endpoint/).selectOption({ label: "public-air" });

      const verdict = dialog.getByTestId("draft-verdict");
      await expect(dialog.getByText("aim it at an Endpoint of the space without policyRef").first()).toBeVisible();
      await expect(dialog.getByText("spec.outputs.0.targetEndpoint:").first()).toBeVisible();
      await shot(page, "pipeline-refused");

      await dialog.getByLabel(/^Target endpoint/).selectOption({ label: "air-write" });
      await expect(verdict).toContainText("Checked");
      await expect(dialog.getByText("aim it at an Endpoint of the space without policyRef")).toHaveCount(0);
      await shot(page, "pipeline-fixed");
      expect(checked.some((body) => body.includes(":public-air"))).toBe(true);
      expect(checked.at(-1)).toContain(":air-write");
    });
  });
}

import { expect, test } from "@playwright/test";
import type { Page, TestInfo } from "@playwright/test";
import { axeViolations } from "./axe";

// T-4847: publishing a dataset to CKAN end to end in a real browser, the API answered in the
// browser. A steward drafts the publication, edits it, sees the CKAN page it becomes, types the
// endpoint's name for the red lane and proposes; an approver reads the same page beside the diff;
// the open-data page then links the published dataset. Each step is shot at 1440 and 400 px.

const V = "joinedcontext.com/v1alpha1";
const LIST = (items: unknown[]) => ({ apiVersion: V, kind: "List", items });
const STEWARD = { subject: "s1", username: "eva.steward", name: "Eva Steward", email: "eva@hel.fi", roles: ["portal-editor"] };
const APPROVER = { subject: "a1", username: "olli.admin", name: "Olli Admin", email: "olli@hel.fi", roles: ["portal-editor"] };

const ENDPOINT = {
  apiVersion: V,
  kind: "Endpoint",
  metadata: { name: "helsinki-kpi", namespace: "helsinki", title: { en: "City indicators" } },
  spec: { contextSpaceRef: "kpi", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "organization", enabledRepresentations: ["ngsi-ld", "csv", "geojson"] },
  status: { phase: "Live" },
};

const DRAFT = {
  endpoint: "helsinki-kpi",
  makesPublic: true,
  catalog: {
    license: "CC_BY_4_0",
    themes: ["ECON"],
    keywords: { en: ["indicator", "kpi value"] },
    publisher: { name: { en: "City of Helsinki" } },
    contactPoint: { name: "Open data desk", email: "opendata@hel.fi" },
  },
  publish: { ckan: { instanceRef: { kind: "CkanInstance", name: "hel-fi" } } },
  missing: ["spatial", "temporal", "frequency"],
  attributes: [
    { type: "KeyPerformanceIndicator", name: "kpiValue" },
    { type: "KeyPerformanceIndicator", name: "location" },
    { type: "KeyPerformanceIndicator", name: "calculationMethod" },
    { type: "KeyPerformanceIndicator", name: "reporterEmail", withheld: "personal" },
  ],
};

const CHANGE = {
  apiVersion: V,
  kind: "ChangeProposal",
  metadata: { name: "chg-ckan-1", namespace: "helsinki" },
  summary: { key: "change.summary.update", params: { kind: "Endpoint", name: "helsinki-kpi", fields: 5 } },
  author: { name: "Eva Steward", email: "eva@hel.fi" },
  createdAt: "2026-10-21T08:00:00Z",
  status: { lane: "red", phase: "PendingApproval", plan: { update: 1 } },
  files: [
    {
      path: "projects/helsinki/endpoints/helsinki-kpi.yaml",
      kind: "Endpoint",
      operation: "Update",
      lane: "red",
      fields: [
        { path: "spec.audience", from: "organization", to: "public" },
        { path: "metadata.title.en", from: "City indicators", to: "Helsinki city indicators" },
        { path: "spec.catalog.license", to: "CC_BY_4_0" },
        { path: "spec.catalog.themes[0]", to: "ECON" },
        { path: "spec.catalog.keywords.en[0]", to: "indicator" },
        { path: "spec.publish.ckan.instanceRef.name", to: "hel-fi" },
        { path: "spec.projection.hiddenAttributes[0]", to: "reporterEmail" },
      ],
    },
  ],
};

// The headline diff is the file's own, as the API sends it for a one-file change.
Object.assign(CHANGE, { planFields: CHANGE.files[0].fields });

const STATUS = {
  instances: [{ name: "hel-fi", url: "https://opendata.dev.joinedcontext.com", organizationDefault: "helsinki", apiTokenRef: "ckan-api-token" }],
  publications: [
    {
      endpoint: "helsinki-kpi",
      instance: "hel-fi",
      instanceMissing: false,
      dataset: "helsinki-kpi",
      datasetUrl: "https://opendata.dev.joinedcontext.com/dataset/helsinki-kpi",
      resources: [
        { name: "Table", format: "CSV", url: "https://api.dev.joinedcontext.com/api/endpoint/k7m2/file.csv" },
        { name: "DCAT-AP record", format: "JSON-LD", url: "https://api.dev.joinedcontext.com/api/endpoint/k7m2/" },
      ],
    },
  ],
};

async function stubApi(page: Page, who: typeof STEWARD): Promise<{ writes: { path: string; body: unknown }[] }> {
  const writes: { path: string; body: unknown }[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(who);
    if (path.endsWith("/permissions/me")) {
      const verbs = who === APPROVER ? ["read", "approve"] : ["read", "propose"];
      return json({ project: "helsinki", bootstrap: false, grants: [{ role: "r", binding: "b", scope: "project:helsinki", rule: { kinds: ["Endpoint", "CkanInstance"], verbs } }] });
    }
    if (path === "/api/v1/projects") return json(LIST([{ name: "helsinki" }]));
    if (path === "/api/v1/projects/helsinki/catalogue/drafts") return json(DRAFT);
    if (path === "/api/v1/projects/helsinki/endpoints" && request.method() === "GET") return json(LIST([ENDPOINT]));
    if (path === "/api/v1/projects/helsinki/endpoints/helsinki-kpi" && request.method() === "GET") return json(ENDPOINT);
    if (path === "/api/v1/projects/helsinki/changes/chg-ckan-1") return json(CHANGE);
    if (path === "/api/v1/projects/helsinki/ckan/status") return json(STATUS);
    if (request.method() !== "GET") {
      writes.push({ path: path + url.search, body: request.postDataJSON() as unknown });
      if (url.searchParams.get("dryRun") === "All") return json({ valid: true, verdict: { ok: true, findings: [] } });
      return json({ apiVersion: V, kind: "Change", metadata: { name: "chg-ckan-1", namespace: "helsinki" }, status: { lane: "red", phase: "PendingApproval", plan: { update: 1 } } }, 202);
    }
    return json(LIST([]));
  });
  return { writes };
}

async function shoot(page: Page, info: TestInfo, name: string): Promise<void> {
  await page.screenshot({ path: info.outputPath(`${name}.png`), fullPage: true });
}

/** A dialog is fixed to the viewport: a tall one shows the whole form in one shot. */
async function shootDialog(page: Page, info: TestInfo, name: string, width: number): Promise<void> {
  await page.setViewportSize({ width, height: 2600 });
  await page.screenshot({ path: info.outputPath(`${name}.png`) });
  await page.setViewportSize({ width, height: 900 });
}

for (const width of [1440, 400]) {
  test(`a steward publishes, an approver reads the same CKAN page, the dataset is linked (${width} px)`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 900 });
    const { writes } = await stubApi(page, STEWARD);
    await page.goto("/projects/helsinki/ckan?lang=en");
    await page.getByRole("button", { name: "Publish a dataset" }).first().click();
    const dialog = page.getByRole("dialog", { name: "Publish a dataset" });
    await dialog.getByRole("combobox", { name: "Endpoint" }).click();
    await page.getByRole("option", { name: /City indicators/ }).click();
    await dialog.getByRole("button", { name: "Draft the description" }).click();

    await expect(dialog.getByRole("textbox", { name: /Dataset title/ })).toHaveValue("City indicators");
    await dialog.getByRole("textbox", { name: /Dataset title/ }).fill("Helsinki city indicators");
    await dialog.getByRole("textbox", { name: /Description/ }).fill("Monthly indicators of the city's services.");
    await dialog.getByRole("combobox", { name: "Area on the map" }).selectOption("FI1B1");
    await expect(dialog.getByRole("checkbox", { name: /reporterEmail/ })).toBeDisabled();
    await dialog.getByRole("checkbox", { name: /calculationMethod/ }).uncheck();
    await shootDialog(page, info, `${width}-1-form`, width);
    expect(await axeViolations(page)).toEqual([]);

    await dialog.getByRole("button", { name: "Preview the entry" }).click();
    const preview = dialog.getByRole("region", { name: "3. Preview" });
    await expect(preview.getByRole("heading", { name: "Helsinki city indicators" })).toBeVisible();
    await expect(preview.getByText("Helsinki-Uusimaa")).toBeVisible();
    await expect(preview.getByRole("list", { name: "Columns" }).getByRole("listitem")).toHaveCount(2);
    const propose = dialog.getByRole("button", { name: "Make public and propose publication" });
    await expect(propose).toHaveAttribute("aria-disabled", "true");
    await shootDialog(page, info, `${width}-2-preview`, width);
    await dialog.getByRole("textbox", { name: /Type the endpoint's name/ }).fill("helsinki-kpi");
    await propose.click();
    await expect(dialog.getByText(/next reconcile/)).toBeVisible();
    const put = writes.filter((w) => !w.path.includes("dryRun") && w.path.endsWith("/endpoints/helsinki-kpi"));
    expect(put).toHaveLength(1);
    const body = put[0].body as { spec: { audience: string; projection: { hiddenAttributes: string[] } } };
    expect(body.spec.audience).toBe("public");
    expect(body.spec.projection.hiddenAttributes).toEqual(["calculationMethod", "reporterEmail"]);

    // A second person approves: the same CKAN page beside the diff, the name typed back.
    await page.unrouteAll({ behavior: "ignoreErrors" });
    await stubApi(page, APPROVER);
    await page.goto("/projects/helsinki/approvals/chg-ckan-1?lang=en");
    const onApproval = page.getByRole("region", { name: "The CKAN page of helsinki-kpi after the approval" });
    await expect(onApproval.getByRole("heading", { name: "Helsinki city indicators" })).toBeVisible();
    await expect(onApproval.getByText(/DCAT-AP record/)).toBeVisible();
    await expect(onApproval.getByText(/Table in CKAN/)).toHaveCount(0);
    await shoot(page, info, `${width}-3-approval`);
    expect(await axeViolations(page)).toEqual([]);

    // Published: the open-data page links the package, its table and the map on its page.
    await page.goto("/projects/helsinki/ckan?lang=en");
    const published = page.getByRole("region", { name: "Published endpoints" });
    await expect(published.getByRole("link", { name: "helsinki-kpi" })).toHaveAttribute("href", STATUS.publications[0].datasetUrl);
    await expect(published.getByRole("link", { name: "Table (CSV)" })).toBeVisible();
    await expect(published.getByRole("link", { name: "Map and table on the dataset page" })).toBeVisible();
    await shoot(page, info, `${width}-4-published`);
  });
}

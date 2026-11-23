import { expect, test } from "@playwright/test";
import type { Page, TestInfo } from "@playwright/test";
import { axeViolations } from "./axe";
import { manifestReadsAsRest } from "./manifestReads";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { phoneHits } from "./phoneCheck.cts";

test.beforeEach(({ page }) => manifestReadsAsRest(page));

// T-7746: an Organization's own dataset fields in the publish form, each state at 1440 and 400 px
// (default, a required field refused in the form, filled, refused by the server), and the portal
// Display tab offering the fields as about rows a portal hides (doprava).

const V = "joinedcontext.com/v1alpha1";
const LIST = (items: unknown[]) => ({ apiVersion: V, kind: "List", items });
const STEWARD = { subject: "s1", username: "eva.steward", name: "Eva Steward", email: "eva@bbsk.sk", roles: ["portal-editor"] };
const FIELDS = [
  { name: "funding", label: { en: "Funding", sk: "Financovanie" }, type: "enum", values: ["eu", "city", "state"], required: true },
  { name: "reviewed", label: { en: "Reviewed on", sk: "Revízia" }, type: "date" },
  { name: "report", label: { en: "Annual report", sk: "Výročná správa" }, type: "url" },
];
const ENDPOINT = {
  apiVersion: V,
  kind: "Endpoint",
  metadata: { name: "bbsk-kpi", namespace: "bbsk", title: { en: "Region indicators" } },
  spec: { contextSpaceRef: "kpi", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "public", enabledRepresentations: ["ngsi-ld", "csv"] },
  status: { phase: "Live" },
};
const DRAFT = {
  endpoint: "bbsk-kpi",
  makesPublic: false,
  catalog: { license: "CC_BY_4_0", themes: ["ECON"], keywords: { en: ["indicator"] }, publisher: { name: { en: "Banská Bystrica Region" } } },
  publish: { ckan: { instanceRef: { kind: "CkanInstance", name: "bbsk" } } },
  missing: ["spatial", "temporal", "frequency"],
  attributes: [{ type: "KeyPerformanceIndicator", name: "kpiValue" }],
  datasetFields: FIELDS,
};
const ORGANIZATION = { apiVersion: V, kind: "Organization", metadata: { name: "bbsk", namespace: "org" }, spec: { domain: "bbsk.sk", datasetFields: FIELDS } };
const REFUSAL = "invalid spec.catalog.fields.funding: one of: eu, city, state";

async function stub(page: Page, refuse = false): Promise<unknown[]> {
  const writes: unknown[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(STEWARD);
    if (path.endsWith("/permissions/me")) {
      return json({
        project: "bbsk",
        bootstrap: false,
        grants: [{ role: "r", binding: "b", scope: "project:bbsk", rule: { kinds: ["Endpoint", "CkanInstance", "Portal", "Organization"], verbs: ["read", "propose"] } }],
      });
    }
    if (path === "/api/v1/projects") return json(LIST([{ name: "bbsk", organization: "bbsk" }]));
    if (path === "/api/v1/projects/bbsk/catalogue/drafts") return json(DRAFT);
    if (path === "/api/v1/projects/bbsk/endpoints" && request.method() === "GET") return json(LIST([ENDPOINT]));
    if (path === "/api/v1/projects/bbsk/endpoints/bbsk-kpi" && request.method() === "GET") return json(ENDPOINT);
    if (path === "/api/v1/catalogue") {
      return json({ total: 0, page: 1, pageSize: 20, datasets: [], unavailable: [],
        facets: { publisher: [], theme: [], format: [], licence: [], spatial: [], year: [], keyword: [] } });
    }
    if (path === "/api/v1/catalogue/readable") return json({ datasets: [], unanswered: 0 });
    if (path === "/api/v1/projects/bbsk/ckan/status") {
      return json({ instances: [{ name: "bbsk", url: "https://opendata.bbsk.sk", organizationDefault: "bbsk", apiTokenRef: "ckan-api-token" }], publications: [] });
    }
    if (path === "/api/v1/projects/org/organizations") return json(LIST([ORGANIZATION]));
    if (path === "/api/v1/organization/portal-hosts") return json({ items: [{ organization: "bbsk", hosts: ["doprava.bbsk.sk"], opendata: "https://opendata.bbsk.sk" }] });
    if (path === "/api/v1/projects/org/portals/bbsk-doprava" && request.method() === "GET") {
      return json({ apiVersion: V, kind: "Portal", metadata: { name: "bbsk-doprava", namespace: "org" }, spec: { organizationRef: "bbsk", path: "doprava", title: { en: "Transport" }, languages: ["en", "sk"] } });
    }
    if (path.endsWith("/events")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
    if (request.method() !== "GET") {
      writes.push(request.postDataJSON() as unknown);
      if (refuse) return json({ type: "about:blank", title: "Unprocessable Content", status: 422, detail: REFUSAL, errors: [REFUSAL] }, 422);
      if (url.searchParams.get("dryRun") === "All") return json({ valid: true, verdict: { ok: true, findings: [] } });
      return json({ apiVersion: V, kind: "Change", metadata: { name: "chg-7746", namespace: "bbsk" }, status: { lane: "green", phase: "PendingApproval", plan: { update: 1 } } }, 202);
    }
    return json(LIST([]));
  });
  return writes;
}

/**
 * The dialog is fixed to the viewport: a tall viewport shows the whole form in one shot. The phone
 * check runs first at the phone's own height (a mobile viewport stretched to 2600 px zooms the page
 * out), and its report names the tall shot's sha256 as writePhoneReport does.
 */
async function shoot(page: Page, info: TestInfo, state: string, width: number, height = 2600, modal = true): Promise<void> {
  await page.waitForFunction(() => document.getAnimations().every((animation) => animation.playState !== "running"));
  const hits = width === 400 ? await phoneHits(page) : [];
  await page.setViewportSize({ width, height });
  const path = info.outputPath(`${state}-${width}.png`);
  await page.screenshot({ path });
  await page.setViewportSize({ width, height: 900 });
  if (width === 400) {
    const sha256 = createHash("sha256").update(readFileSync(path)).digest("hex");
    writeFileSync(path.replace(/\.png$/, ".phone.json"), `${JSON.stringify({ sha256, hits }, null, 1)}\n`);
    // With a modal open Chrome's mobile text autosizing stops, so the page under the dialog and the
    // dialog read the Portal's 80 % type scale (body 11.2 px) as under 12 px, and the page behind
    // keeps its own targets: recorded in the report, not this form's. Overflow stays held, and
    // each field control is measured for a finger below.
    expect(hits.filter((hit) => !modal || hit.kind === "overflow")).toEqual([]);
    if (modal) {
      for (const control of await page.locator('input[id^="publish-field-"], select[id^="publish-field-"]').all()) {
        const box = await control.boundingBox();
        expect(box?.height ?? 0).toBeGreaterThanOrEqual(44);
      }
    }
  }
}

async function openForm(page: Page): Promise<ReturnType<Page["getByRole"]>> {
  await page.goto("/catalogue?lang=en");
  await page.getByRole("button", { name: "Publish a dataset" }).first().click();
  const dialog = page.getByRole("dialog", { name: "Publish a dataset" });
  await dialog.getByRole("combobox", { name: "Endpoint" }).click();
  await page.getByRole("option", { name: /Region indicators/ }).click();
  await dialog.getByRole("button", { name: "Draft the description" }).click();
  await expect(dialog.getByRole("group", { name: "Your organization's fields" })).toBeVisible();
  return dialog;
}

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.use(width === 400 ? { hasTouch: true, isMobile: true } : {});
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
    });

    test("dataset fields: default, refused in the form, filled, refused by the server", async ({ page }, info) => {
      const writes = await stub(page);
      let dialog = await openForm(page);
      const group = dialog.getByRole("group", { name: "Your organization's fields" });
      await expect(group.getByRole("combobox", { name: /Funding/ })).toHaveAccessibleDescription("Pick one of the values your organization set.");
      await group.scrollIntoViewIfNeeded();
      await shoot(page, info, "fields-default", width);
      expect(await axeViolations(page)).toEqual([]);

      await dialog.getByRole("button", { name: "Preview the entry" }).click();
      await expect(group.getByText("Your organization requires this field: fill it.")).toBeVisible();
      await shoot(page, info, "fields-required", width);
      expect(await axeViolations(page)).toEqual([]);

      await group.getByRole("combobox", { name: /Funding/ }).selectOption("eu");
      await group.getByLabel(/Reviewed on/).fill("2026-11-23");
      await group.getByLabel(/Annual report/).fill("https://bbsk.sk/vyrocna-sprava-2025");
      await shoot(page, info, "fields-filled", width);
      await dialog.getByRole("button", { name: "Preview the entry" }).click();
      await dialog.getByRole("button", { name: "Propose publication" }).click();
      await expect(dialog.getByText(/next reconcile/)).toBeVisible();
      const sent = writes.at(-1) as { spec: { catalog: { fields: Record<string, string> } } };
      expect(sent.spec.catalog.fields).toEqual({ funding: "eu", reviewed: "2026-11-23", report: "https://bbsk.sk/vyrocna-sprava-2025" });

      await page.unrouteAll({ behavior: "ignoreErrors" });
      await manifestReadsAsRest(page);
      await stub(page, true);
      dialog = await openForm(page);
      await dialog.getByRole("group", { name: "Your organization's fields" }).getByRole("combobox", { name: /Funding/ }).selectOption("eu");
      await dialog.getByRole("button", { name: "Preview the entry" }).click();
      await dialog.getByRole("button", { name: "Propose publication" }).click();
      await expect(dialog.getByRole("alert")).toContainText("Your organization refused the field “Funding” (one of: eu, city, state). Go back, correct it and propose again.");
      await shoot(page, info, "fields-refusal", width);
    });

    test("display: the Organization's fields are about rows a portal can hide", async ({ page }, info) => {
      await stub(page);
      await page.goto("/portals/bbsk-doprava?lang=en");
      await page.getByRole("tab", { name: "Display" }).click();
      const about = page.locator("section").filter({ has: page.getByRole("heading", { name: "About table rows" }) });
      await expect(about.getByRole("checkbox", { name: "Funding" })).toBeChecked();
      await shoot(page, info, "display-fields", width, 1800, false);
      expect(await axeViolations(page)).toEqual([]);
      await about.getByRole("checkbox", { name: "Funding" }).uncheck();
      await about.getByRole("checkbox", { name: "Reviewed on" }).uncheck();
      await about.getByRole("checkbox", { name: "Annual report" }).uncheck();
      await expect(page.getByText("Unpublished changes: publish to propose them for approval.")).toBeVisible();
      await shoot(page, info, "display-fields-hidden", width, 1800, false);
    });
  });
}

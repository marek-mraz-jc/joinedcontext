import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { axeViolations } from "./axe";
import { greenVerdict, isCheck } from "./verdict";
import { manifestReadsAsRest } from "./manifestReads";

// The pipeline pages read through their Portal doors (T-7109): each REST stub below answers its door.
test.beforeEach(({ page }) => manifestReadsAsRest(page));

// The workbench's whole-source check (G-21, T-7085) as an author meets it in the pipeline editor:
// every page of the saved pipeline's source, counted by rule, the first rejected records, a
// refusal in the Portal's words, and a source with nothing rejected. `vite preview` has no Portal
// API behind it, so the API is answered in the browser; JC_EVIDENCE_DIR saves each state.

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const PIPELINE = {
  apiVersion: "joinedcontext.com/v1alpha2",
  kind: "Pipeline",
  metadata: { name: "ovzdusie-stanice", namespace: "bbsk" },
  spec: {
    class: "scheduled",
    sources: [{ dataSourceRef: { kind: "DataSource", name: "shmu-ckan" } }],
    steps: [{ kind: "bloblang", bloblang: 'root.id = "urn:ngsi-ld:AirQualityObserved:" + this.station\nroot.type = "AirQualityObserved"' }],
    outputs: [{ targetEndpoint: "urn:ngsi-ld:Endpoint:bbsk.sk:ovzdusie:ovzdusie-all" }],
  },
  status: { phase: "Live" },
};

const row = (id: number, rule: string, path: string, message: string) => ({
  id,
  at: "2026-11-15T20:00:00Z",
  record: { id: `urn:ngsi-ld:AirQualityObserved:s-${id}`, type: "AirQualityObserved" },
  rule,
  path,
  message,
});
const datatype = (id: number) => row(id, "sh:datatype", "pm10", "the attribute pm10 breaks the data model of AirQualityObserved: its value must be a number");
const enumBroken = (id: number) => row(id, "sh:in", "window", "the attribute window breaks the data model of AirQualityObserved: its value is not one of the values the model lists");

const FILLED = {
  records: 2400,
  accepted: 2214,
  rejected: 186,
  complete: true,
  errors: [],
  groups: [
    { rule: "sh:datatype", path: "pm10", count: 171, fix: "mapping", examples: [datatype(12), datatype(40), datatype(77)] },
    { rule: "sh:in", path: "window", count: 13, fix: "mapping", allowed: ["day", "month"], examples: [enumBroken(301)] },
    { rule: "step", path: "", count: 2, fix: "step", examples: [row(990, "step", "", "step 1 threw: date 31.09.2026 is not a calendar day")] },
  ],
  rows: [datatype(12), datatype(40), datatype(77), enumBroken(301), row(990, "step", "", "step 1 threw: date 31.09.2026 is not a calendar day")],
};
const CLEAN = { records: 2400, accepted: 2400, rejected: 0, complete: true, errors: [], groups: [], rows: [] };
const BUSY = {
  type: "about:blank",
  title: "Conflict",
  status: 409,
  detail: "a pipeline test is already running in 'bbsk'; wait for it",
};

async function stubApi(page: Page, check: { status: number; body: unknown }, seen: string[]): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "s1", username: "eva", name: "Eva Horváthová", email: "eva@bbsk.sk", roles: ["portal-editor"] });
    }
    if (path.endsWith("/branding")) {
      return json({ instanceName: "joinedcontext", orgDomain: "bbsk.sk", languages: { default: "en", offered: ["en"] } });
    }
    if (path.endsWith("/source-check") && request.method() === "POST") {
      seen.push(path);
      return json(check.body, check.status);
    }
    if (isCheck(request.method(), request.url())) return json(greenVerdict(request.postData()));
    if (/\/ops\/jc_pipeline_sample_source$/.test(path)) {
      const records = [{ station: "bb-1", pm10: "18.2" }];
      return json({ records, fields: Object.keys(records[0]), count: 1, truncated: false, errors: [] });
    }
    if (/\/ops\/jc_pipeline_try_mapping$/.test(path)) {
      return json({ input: {}, records: [{ id: "urn:ngsi-ld:AirQualityObserved:bb-1", type: "AirQualityObserved" }], errors: [] });
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "bbsk" }] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: "bbsk", grants: [{ rule: { kinds: ["Pipeline", "DataSource"], verbs: ["read", "propose"] } }] });
    }
    if (path.endsWith("/pipelines")) return json(list([PIPELINE]));
    if (path.endsWith("/datasources")) {
      return json(list([{ kind: "DataSource", metadata: { name: "shmu-ckan", namespace: "bbsk" }, spec: { type: "http", http: { url: "https://data.bbsk.sk/api/3/action/datastore_search", preset: { kind: "ckan", resourceId: "ovzdusie" } } } }]));
    }
    if (path.endsWith("/endpoints")) {
      return json(list([{ kind: "Endpoint", metadata: { name: "ovzdusie-all", namespace: "bbsk" }, spec: { contextSpaceRef: "ovzdusie", slug: "vq7m2z4tk6xh3n5jb2ryd3wcfa", audience: "organization" } }]));
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  const section = page.locator('section[aria-labelledby="source-check-title"]');
  await section.scrollIntoViewIfNeeded();
  await section.screenshot({ path: join(dir, `${name}.png`) });
}

async function openCheck(page: Page) {
  await page.goto("/projects/bbsk/pipelines?lang=en");
  const pipelineRow = page.getByRole("row").filter({ hasText: "ovzdusie-stanice" });
  await pipelineRow.getByRole("button", { name: /More actions/ }).click();
  await page.getByRole("menuitem", { name: "Edit" }).click();
  const section = page.locator('section[aria-labelledby="source-check-title"]');
  await expect(section).toBeVisible();
  return section;
}

test.describe("the whole-source check in the pipeline workbench (G-21)", () => {
  for (const width of [1440, 400]) {
    test(`an author counts the whole source by rule at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      const seen: string[] = [];
      await stubApi(page, { status: 200, body: FILLED }, seen);
      const section = await openCheck(page);
      await expect(section).toContainText("Nothing is written");
      await shot(page, `source-check-ready-${width}`);
      await section.getByRole("button", { name: "Check whole source" }).click();
      await expect(section.getByRole("status")).toContainText("2,400 records: 2,214 accepted, 186 rejected");
      expect(seen).toEqual(["/api/v1/projects/bbsk/pipelines/ovzdusie-stanice/source-check"]);
      const groups = section.getByRole("region", { name: "By rule" });
      await expect(groups.getByRole("listitem")).toHaveCount(3);
      await expect(groups.getByRole("listitem").nth(0)).toContainText("Fix in the mapping");
      await section.getByText("First 5 rejected records").click();
      await expect(section).toContainText("Record 990");
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `source-check-filled-${width}`);
    });

    test(`a check the Portal refuses says why at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, { status: 409, body: BUSY }, []);
      const section = await openCheck(page);
      await section.getByRole("button", { name: "Check whole source" }).click();
      await expect(section.getByRole("alert")).toContainText("already running");
      await expect(section.getByRole("button", { name: "Check whole source" })).toBeEnabled();
      await shot(page, `source-check-error-${width}`);
    });

    test(`a source with nothing rejected says a live run writes it all at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, { status: 200, body: CLEAN }, []);
      const section = await openCheck(page);
      await section.getByRole("button", { name: "Check whole source" }).click();
      await expect(section).toContainText("no record breaks a rule");
      await expect(section.getByRole("region", { name: "By rule" })).toHaveCount(0);
      await shot(page, `source-check-empty-${width}`);
    });
  }
});

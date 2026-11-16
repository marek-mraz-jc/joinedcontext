import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { greenVerdict, isCheck } from "./verdict";
import { manifestReadsAsRest } from "./manifestReads";

// The pages read their manifests through the OData service; this file stubs the REST reads (T-7073).
test.beforeEach(({ page }) => manifestReadsAsRest(page));

// T-7062 (G-18, G-23): a Barcelona tree register mapped with no Bloblang typed. The workbench
// samples the CSV, the mapper links each tree to its District by code, writes a bilingual name
// (ca + es), reprojects ETRS89 / UTM 31N coordinates and translates the status to the model's
// values; the "When it runs" step sets a daily 03:00 load; the palette offers its tiers. The API
// is answered in the browser, as in pipeline_editor.spec.ts; the dialog and its steps are real.

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const manifest = (kind: string, name: string, spec: unknown) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: "helsinki" },
  spec,
});

const RECORDS = [
  { ID: "T1", NOM_CA: "Plàtan", NOM_ES: "Plátano", CODI_DISTRICTE: "01", X: "430887.56", Y: "4581837.85", ESTAT: " Bo " },
  { ID: "T2", NOM_CA: "Pi", NOM_ES: "Pino", CODI_DISTRICTE: "02", X: "430900.00", Y: "4581800.00", ESTAT: "Dolent" },
];
const ATTRIBUTES = {
  model: "trees",
  version: "1.0.0",
  type: "Tree",
  description: "A street tree of the city register.",
  attributes: [
    { name: "name", kind: "LanguageProperty", required: false, description: "The tree's common name." },
    { name: "refDistrict", kind: "Relationship", required: false, relationship: { target: "District", many: false } },
    { name: "location", kind: "GeoProperty", required: false },
    { name: "status", kind: "Property", valueType: "string", required: false, values: ["working", "closed"] },
  ],
};
const LINKML = "id: https://hel.fi/models/trees\nname: trees\nclasses:\n  Tree:\n    slots: [status]\nslots:\n  status:\n    range: string\n";

async function stubApi(page: Page): Promise<{ ops: { name: string; body: string }[] }> {
  const ops: { name: string; body: string }[] = [];
  await page.route(/\/api\/v1\//, async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "b7c1e0f4", username: "jana", name: "Jana Kováčová", email: "jana@hel.fi", roles: ["portal-editor"] });
    if (isCheck(request.method(), request.url())) return json(greenVerdict(request.postData()));
    const op = /\/ops\/(jc_[a-z_]+)$/.exec(path)?.[1];
    if (op) {
      ops.push({ name: op, body: request.postData() ?? "" });
      if (op === "jc_pipeline_sample_source") return json({ records: RECORDS, fields: Object.keys(RECORDS[0]), count: 2, truncated: false, errors: [] });
      if (op === "jc_pipeline_try_mapping") return json({ records: [{ id: "urn:ngsi-ld:Tree:hel.fi:air:T1", type: "Tree" }], errors: [] });
      if (op === "jc_pipeline_validate") return json({ space: "air", model: "trees", version: "1.0.0", valid: 2, rejected: 0, verdicts: [] });
      return json({});
    }
    const draft = /^\/api\/v1\/projects\/([^/]+)\/drafts\/([^/]+)\/([^/]+)$/.exec(path);
    if (draft && request.method() === "PUT") {
      const { manifest: body } = JSON.parse(request.postData() ?? "{}") as { manifest: unknown };
      return json({ project: draft[1], kind: draft[2], name: draft[3], manifest: body, touchedBy: "jana", touchedKind: "person", version: 1, updatedAt: new Date().toISOString() });
    }
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path === "/api/v1/projects/helsinki") return json(manifest("Project", "helsinki", { quotas: { minPipelinePeriodSeconds: 3600 } }));
    if (path === "/api/v1/organization/datamodels") {
      return json({ ...list([{ name: "trees", level: "project", project: "helsinki", space: "air", version: "1.0.0", lifecycle: "published", classes: ["Tree"] }]), smartDataModels: [] });
    }
    if (path.endsWith("/datamodels/trees/source")) return route.fulfill({ status: 200, contentType: "text/yaml", body: LINKML });
    if (path.endsWith("/spaces/air/types/Tree/attributes")) return json(ATTRIBUTES);
    if (path.endsWith("/datasources")) return json(list([manifest("DataSource", "bcn-trees", { type: "http", http: { url: "https://opendata-ajuntament.barcelona.cat/arbrat.csv", format: "csv" } })]));
    if (path.endsWith("/endpoints")) return json(list([manifest("Endpoint", "public-air", { contextSpaceRef: "air", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "public" })]));
    if (path.endsWith("/spaces")) return json(list([manifest("ContextSpace", "air", { dataModelRef: { kind: "DataModel", name: "trees" } })]));
    if (path.endsWith("/datamodels")) return json(list([manifest("DataModel", "trees", { linkml: LINKML, classes: ["Tree"] })]));
    if (path.endsWith("/branding")) return json({ instanceName: "joinedcontext", orgDomain: "hel.fi", languages: { default: "en", offered: ["en"] } });
    return json(list([]));
  });
  return { ops };
}

const shot = async (page: Page, name: string) => {
  if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/${name}.png`, fullPage: false });
};

test.describe("G-18 mapper and G-23 schedule step", () => {
  test("maps a Barcelona CSV to a Relationship and a bilingual name, schedules it daily, no Bloblang typed", async ({ page }) => {
    const { ops } = await stubApi(page);
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto("/projects/helsinki/pipelines?lang=en");
    await page.getByRole("button", { name: "New pipeline" }).first().click();
    const dialog = page.getByTestId("form-page");
    await dialog.getByLabel(/^Name/).fill("bcn-trees");
    await dialog.locator("#workbench-source-pick").selectOption("datasource:bcn-trees");
    await expect(dialog.getByText("Plàtan").first()).toBeVisible();
    await dialog.locator("#workbench-target-pick").selectOption({ label: "public-air" });
    await dialog.getByRole("radio", { name: /Map fields/ }).check();
    await dialog.getByRole("combobox", { name: /Entity type/ }).click();
    // The picker's option carries a link to its model: the keyboard picks the type itself.
    await page.keyboard.type("Tree");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    // The workbench drafted a step from the first record; the mapper takes over on request.
    await dialog.getByRole("button", { name: "Map fields instead" }).click();

    const ref = dialog.getByTestId("mapper-attribute-refDistrict");
    await ref.getByRole("combobox", { name: /Source of refDistrict/ }).selectOption("ref");
    await ref.getByRole("combobox", { name: /Key field of the District/ }).selectOption("CODI_DISTRICTE");
    const name = dialog.getByTestId("mapper-attribute-name");
    await name.getByRole("combobox", { name: /Source of name/ }).selectOption("languages");
    await name.getByRole("combobox", { name: /^Language/ }).selectOption("ca");
    await name.getByRole("combobox", { name: /Field for ca/ }).selectOption("NOM_CA");
    await name.getByRole("button", { name: "Add a language" }).click();
    await name.getByRole("combobox", { name: /^Language/ }).nth(1).selectOption("es");
    await name.getByRole("combobox", { name: /Field for es/ }).selectOption("NOM_ES");
    const location = dialog.getByTestId("mapper-attribute-location");
    await location.getByRole("combobox", { name: /Source of location/ }).selectOption("point");
    await location.getByRole("combobox", { name: /Longitude field/ }).selectOption("X");
    await location.getByRole("combobox", { name: /Latitude field/ }).selectOption("Y");
    await location.getByRole("combobox", { name: /Coordinate system/ }).selectOption("EPSG:25831");
    const status = dialog.getByTestId("mapper-attribute-status");
    await status.getByRole("combobox", { name: /Source of status/ }).selectOption("field:ESTAT");
    for (const step of ["trim", "lower", "lookup"]) await status.getByRole("combobox", { name: /Change the value/ }).selectOption(step);
    await status.getByRole("combobox", { name: /“bo” becomes/ }).selectOption("working");
    await status.getByRole("combobox", { name: /“dolent” becomes/ }).selectOption("closed");
    await expect(dialog.getByTestId("mapper-preview")).toContainText("urn:ngsi-ld:District:hel.fi:air:01");
    await expect(dialog.getByTestId("mapper-preview")).toContainText('"es": "Plátano"');

    // The pipeline test (PL-43) runs the generated step on the runner: it is sent, and it passes.
    await expect(dialog.getByText(/All 2 records are valid/).first()).toBeVisible();
    const tried = ops.filter((one) => one.name === "jc_pipeline_try_mapping").at(-1)?.body ?? "";
    expect(tried).toContain('format(\\"District\\", $owner, \\"air\\", this.CODI_DISTRICTE');
    expect(tried).toContain('geo_transform(\\"EPSG:25831\\", \\"EPSG:4326\\")');

    await ref.scrollIntoViewIfNeeded();
    await shot(page, "g18-mapper-ref-lang-1440");
    await status.scrollIntoViewIfNeeded();
    await shot(page, "g18-mapper-transforms-1440");

    // When it runs: a daily 03:00 load, the project's minimum said at the field.
    const schedule = dialog.getByRole("region", { name: /When it runs$/ });
    await schedule.getByRole("button", { name: "Run on a schedule" }).click();
    await expect(schedule.getByRole("combobox", { name: "How often" })).toHaveValue("daily");
    await expect(schedule.getByText(/at most once an hour/)).toBeVisible();
    await schedule.scrollIntoViewIfNeeded();
    await shot(page, "g23-schedule-1440");
    await schedule.getByRole("combobox", { name: "How often" }).selectOption("minutes");
    await expect(schedule.getByText(/more often than this project allows/).first()).toBeVisible();
    await shot(page, "g23-schedule-refused-1440");
    await schedule.getByRole("combobox", { name: "How often" }).selectOption("daily");

    await page.setViewportSize({ width: 400, height: 900 });
    await ref.scrollIntoViewIfNeeded();
    await shot(page, "g18-mapper-ref-lang-400");
    await status.scrollIntoViewIfNeeded();
    await shot(page, "g18-mapper-transforms-400");
    await schedule.scrollIntoViewIfNeeded();
    await shot(page, "g23-schedule-400");
    await schedule.getByRole("combobox", { name: "How often" }).selectOption("minutes");
    await schedule.scrollIntoViewIfNeeded();
    await shot(page, "g23-schedule-refused-400");
  });

  test("offers the processors in tiers: forms first, advanced behind its warning", async ({ page }) => {
    await stubApi(page);
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto("/projects/helsinki/pipelines?lang=en");
      await page.getByRole("button", { name: "New pipeline" }).first().click();
      const dialog = page.getByTestId("form-page");
      await dialog.getByText("Advanced editor").click();
      const palette = dialog.getByTestId("palette-processors");
      await palette.locator("summary").first().click();
      await expect(palette.getByTestId("palette-tier-forms")).toBeVisible();
      await expect(palette.getByTestId("palette-processor-dedupe")).toBeVisible();
      await expect(palette.getByTestId("palette-processor-branch")).toBeHidden();
      await palette.getByTestId("palette-tier-advanced").locator("summary").click();
      await expect(palette.getByTestId("palette-tier-advanced")).toContainText("can drop or repeat records");
      await palette.getByTestId("palette-tier-advanced").scrollIntoViewIfNeeded();
      await shot(page, `g23-palette-tiers-${width}`);
    }
  });
});

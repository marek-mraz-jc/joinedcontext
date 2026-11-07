import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { generate } from "../src/pages/pipelines/outputMapper";
import type { Attribute } from "../src/pages/pipelines/outputMapper";
import { digestOf } from "../src/api/digest";
import { greenVerdict, isCheck } from "./verdict";

// T-4839, T-4842: the workbench's Lifetime step writes the pipeline's `spec.lifetime` (PL-80), and
// a T-4822 step that wrote its own `expiresAt` moves into it on open. The API is answered in
// the browser, as in pipeline_editor.spec.ts; JC_SHOTS=<dir> also saves each state at 1440 and
// 400 px for the review.

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const ATTRIBUTES = {
  model: "hsl-transit",
  version: "1.0.0",
  type: "Vehicle",
  description: "One HSL vehicle and its latest position.",
  attributes: [
    { name: "observedOn", kind: "Property", valueType: "string", format: "date-time", required: false },
    { name: "speed", kind: "Property", valueType: "number", required: false, unit: { code: "KMH", ucum: "km/h" } },
    { name: "route", kind: "Property", valueType: "string", required: false },
  ],
};

const RECORDS = [
  { veh: "1203", spd: "31.5", desi: "550", tst: "2026-10-20T19:58:00Z" },
  { veh: "1204", spd: "0", desi: "560", tst: "2026-10-20T19:58:04Z" },
];

/** The step T-4822's mapper wrote: the map, with its own `expiresAt` in the header. */
const LEGACY_STATE = {
  type: "Vehicle",
  idField: "veh",
  attributes: { observedOn: { field: "tst" }, speed: { field: "spd" }, route: { field: "desi" } },
  expires: { field: "tst", plus: "PT10M" },
};

function pipeline(bloblang: string, lifetime?: unknown) {
  return {
    apiVersion: "joinedcontext.com/v1alpha2",
    kind: "Pipeline",
    metadata: { name: "hfp-vehicles", namespace: "helsinki" },
    spec: {
      class: "resident",
      sources: [{ dataSourceRef: { kind: "DataSource", name: "hfp" } }],
      steps: [{ kind: "bloblang", bloblang }],
      outputs: [{ targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:mobility:vehicles-write" }],
      ...(lifetime ? { lifetime } : {}),
    },
    status: { phase: "Live" },
  };
}

interface Stub {
  checks: unknown[];
  refuse?: { errors: string[] };
}

async function stubApi(page: Page, pipelines: unknown[], stub: Stub): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "s1", username: "aino", name: "Aino Virtanen", email: "aino@hel.fi", roles: ["portal-editor"] });
    }
    if (path.endsWith("/branding")) {
      return json({ instanceName: "joinedcontext", orgDomain: "hel.fi", languages: { default: "en", offered: ["en"] } });
    }
    if (isCheck(request.method(), request.url())) {
      const manifest = JSON.parse(request.postData() ?? "{}") as { spec?: unknown };
      stub.checks.push(manifest.spec);
      if (stub.refuse) {
        // A dry run answers the door's refusal as a red verdict, one finding per reason (PF-57).
        const judged = { ...(JSON.parse(request.postData() ?? "{}") as Record<string, unknown>) };
        delete judged.draft;
        const findings = stub.refuse.errors.map((message) => ({ level: "error", path: "", message }));
        return json({ valid: false, lane: "red", verdict: { ok: false, findings, checkedAt: new Date().toISOString(), inputDigest: digestOf(judged) } });
      }
      return json(greenVerdict(request.postData()));
    }
    if (/\/ops\/jc_pipeline_sample_source$/.test(path)) {
      return json({ records: RECORDS, fields: Object.keys(RECORDS[0]), count: RECORDS.length, truncated: false, errors: [] });
    }
    if (/\/ops\/jc_pipeline_(try_mapping|validate)$/.test(path)) {
      return json({ records: [], fields: [], count: 0, truncated: false, errors: [] });
    }
    if (request.method() !== "GET") {
      return json({ kind: "Change", metadata: { name: "chg-1", namespace: "helsinki" }, status: { phase: "PendingApproval" } }, 202);
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/pipelines")) return json(list(pipelines));
    if (path.endsWith("/datasources")) {
      return json(list([{ kind: "DataSource", metadata: { name: "hfp", namespace: "helsinki" }, spec: { type: "http", http: { url: "https://hfp.hsl.fi/vehicles.json" } } }]));
    }
    if (path.endsWith("/endpoints")) {
      return json(list([{ kind: "Endpoint", metadata: { name: "vehicles-write", namespace: "helsinki" }, spec: { contextSpaceRef: "mobility", slug: "vq7m2z4tk6xh3n5jb2ryd3wcfa", audience: "organization" } }]));
    }
    if (path === "/api/v1/organization/datamodels") {
      return json({ ...list([{ name: "hsl-transit", level: "project", project: "helsinki", space: "mobility", version: "1.0.0", lifecycle: "published", classes: ["Vehicle"] }]), smartDataModels: [] });
    }
    if (path.endsWith("/spaces/mobility/types/Vehicle/attributes")) return json(ATTRIBUTES);
    return json(list([]));
  });
}

async function shot(page: Page, name: string, testId = "pipeline-lifetime"): Promise<void> {
  const dir = process.env.JC_SHOTS;
  if (!dir) return;
  for (const [width, label] of [
    [1440, "desktop"],
    [400, "phone"],
  ] as const) {
    await page.setViewportSize({ width, height: 900 });
    await page.getByTestId(testId).scrollIntoViewIfNeeded();
    await page.screenshot({ path: `${dir}/lifetime-${name}-${label}.png` });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

async function openEditor(page: Page) {
  await page.goto("/projects/helsinki/pipelines?lang=en");
  const row = page.getByRole("row").filter({ hasText: "hfp-vehicles" });
  await row.getByRole("button", { name: /More actions/ }).click();
  await page.getByRole("menuitem", { name: "Edit" }).click();
  return page.getByTestId("form-page");
}

test.describe("pipeline lifetime in the mapper", () => {
  test("a T-4822 header expiry moves into spec.lifetime and is checked as the manifest's", async ({ page }) => {
    // The step exactly as the T-4822 mapper generated it, its own expiresAt lines included.
    const body = generate(LEGACY_STATE, ATTRIBUTES.attributes as Attribute[], "mobility");
    expect(body).toContain("root.expiresAt");
    const stub: Stub = { checks: [] };
    await stubApi(page, [pipeline(body)], stub);
    const dialog = await openEditor(page);

    const expiry = dialog.getByTestId("pipeline-lifetime");
    await expect(dialog.getByText(/now the pipeline's lifetime, in the Lifetime step/)).toBeVisible();
    await expect(expiry.getByRole("radio", { name: /Whole entity expires/ })).toBeChecked();
    await expect(expiry.getByLabel("Time from")).toHaveValue("observedOn");
    await expect(dialog.getByTestId("mapper-preview")).toContainText('"expiresAt": "2026-10-20T20:08:00Z"');
    await shot(page, "migrated");

    // The check judges the manifest with the lifetime and a step that no longer writes expiresAt.
    await expect(dialog.getByTestId("draft-verdict")).toContainText("Checked");
    const spec = stub.checks.at(-1) as { lifetime?: unknown; steps?: { bloblang?: string }[] };
    expect(spec.lifetime).toEqual([{ type: "Vehicle", entity: { from: "observedOn", plus: "PT10M" } }]);
    expect(spec.steps?.[0]?.bloblang).not.toContain("expiresAt");

    // The write time plus a while, the other end the choice offers.
    await expiry.getByRole("radio", { name: /After each write/ }).check();
    await expect(expiry.getByTestId("pipeline-lifetime-sentence")).toContainText("10 minutes after its last write");
    await shot(page, "rolling");

    await expiry.getByRole("radio", { name: /Keep entities/ }).click();
    await expect(expiry.getByLabel("Time from")).toHaveCount(0);
    await shot(page, "never");
  });

  test("a time attribute that holds no time is said at the choice", async ({ page }) => {
    const stub: Stub = { checks: [] };
    await stubApi(page, [pipeline(`# jc-mapper ${JSON.stringify({ ...LEGACY_STATE, expires: undefined })}\n`)], stub);
    const dialog = await openEditor(page);
    // An unreadable header starts the mapper over from the sample; map observedOn to a text field.
    await dialog.getByRole("combobox", { name: /Source of observedOn/ }).selectOption("field:desi");
    const expiry = dialog.getByTestId("pipeline-lifetime");
    await expiry.getByRole("radio", { name: /Whole entity expires/ }).click();
    await expiry.getByLabel("Time from").selectOption("observedOn");
    // The mapper says it where the field is mapped.
    await expect(dialog.getByText(/desi holds 550, which is no time/)).toBeVisible();
    await shot(page, "error", "mapper-preview");
  });

  test("the door's refusal names the slot and the type's slots", async ({ page }) => {
    const reason =
      "spec.lifetime[0].attributes[0].names: `position` is not a slot of `Vehicle` in the space's model hsl-transit 1.0.0 (PL-81) in space mobility; the slots of `Vehicle` are observedOn, route, speed";
    const stub: Stub = { checks: [], refuse: { errors: [reason] } };
    const lifetime = [{ type: "Vehicle", attributes: [{ names: ["position"], rolling: "PT5M" }] }];
    const step = `# jc-mapper ${JSON.stringify({ ...LEGACY_STATE, expires: undefined })}\n`;
    await stubApi(page, [pipeline(step, lifetime)], stub);
    const dialog = await openEditor(page);
    const expiry = dialog.getByTestId("pipeline-lifetime");
    // An entry naming a slot the model lacks stays as the manifest holds it, shown in its row.
    await expect(expiry.getByRole("radio", { name: /Only some fields expire/ })).toBeChecked();
    await expect(expiry.getByRole("combobox", { name: "Field" })).toHaveValue("position");
    await expect(dialog.getByText(/is not a slot of `Vehicle`/).first()).toBeVisible();
    await shot(page, "refusal-kept");
    await shot(page, "refusal", "draft-verdict");
    expect((stub.checks.at(-1) as { lifetime?: unknown }).lifetime).toEqual(lifetime);
  });
});

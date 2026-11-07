import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { greenVerdict, isCheck } from "./verdict";

// T-4840: the workbench's validation judges each mapped record where its write is judged (the
// space's model, the target Endpoint's Policies and view, the lifetime) and shows the expiresAt it
// would get. The API is answered in the browser, as in pipeline_lifetime.spec.ts; JC_SHOTS=<dir>
// also saves each state at 1440 and 400 px for the review.

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const SAMPLE = [
  { veh: "1203", spd: "31.5", tst: "2026-10-20T19:58:00Z" },
  { veh: "1204", spd: "0", tst: "" },
];

const RECORDS = [
  {
    id: "urn:ngsi-ld:Vehicle:1203",
    type: "Vehicle",
    speed: { type: "Property", value: 31.5 },
    observedOn: { type: "Property", value: "2026-10-20T19:58:00Z" },
    expiresAt: "2026-10-20T20:08:00Z",
  },
  { id: "urn:ngsi-ld:Vehicle:1204", type: "Vehicle", speed: { type: "Property", value: 0 }, pm25: { type: "Property", value: 3 } },
];

const GREEN = {
  space: "mobility",
  model: "hsl-transit",
  version: "1.0.0",
  valid: 2,
  rejected: 0,
  verdicts: [
    { index: 0, ok: true, problems: [], endpoint: "helsinki-all", findings: [], expires: { entity: "2026-10-20T20:08:00Z", attributes: { speed: "2026-10-20T20:03:00Z" } } },
    { index: 1, ok: true, problems: [], endpoint: "helsinki-all", findings: [], expires: { entity: "2026-10-20T20:08:04Z", attributes: {} } },
  ],
};

const REFUSED = {
  ...GREEN,
  valid: 1,
  rejected: 1,
  verdicts: [
    GREEN.verdicts[0],
    {
      index: 1,
      ok: false,
      problems: [{ rule: "sh:closed", path: "pm25", message: "pm25 is not an attribute the class declares" }],
      endpoint: "helsinki-all",
      findings: [
        { source: "model", rule: "sh:closed", field: "pm25", reason: "pm25 is not an attribute the class declares" },
        {
          source: "endpoint",
          rule: "NOT_WRITABLE",
          field: "pm25",
          reason: "not writable through helsinki-all: its view lets Vehicle write 4 Attributes and not this one",
        },
        {
          source: "lifetime",
          rule: "from",
          field: "observedOn",
          reason: "the record gives no date-time in observedOn, so the entity would be written without expiresAt",
        },
      ],
    },
  ],
};

const PIPELINE = {
  apiVersion: "joinedcontext.com/v1alpha2",
  kind: "Pipeline",
  metadata: { name: "hsl-hfp-vehicles", namespace: "helsinki" },
  spec: {
    class: "resident",
    sources: [{ dataSourceRef: { kind: "DataSource", name: "hfp" } }],
    steps: [{ kind: "bloblang", bloblang: 'root.id = "urn:ngsi-ld:Vehicle:" + this.veh\nroot.type = "Vehicle"' }],
    outputs: [{ targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:mobility:helsinki-all" }],
    lifetime: [{ type: "Vehicle", entity: { from: "observedOn", plus: "PT10M" } }],
  },
  status: { phase: "Live" },
};

interface Stub {
  validate: unknown;
  /** The source's records, when not SAMPLE. */
  sample?: Record<string, unknown>[];
  status?: number;
  seen: Record<string, unknown>[];
  /** The bodies the workbench sent to jc_pipeline_try_mapping. */
  tried?: Record<string, unknown>[];
}

async function stubApi(page: Page, stub: Stub): Promise<void> {
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
    if (isCheck(request.method(), request.url())) return json(greenVerdict(request.postData()));
    if (/\/ops\/jc_pipeline_sample_source$/.test(path)) {
      const records = stub.sample ?? SAMPLE;
      return json({ records, fields: Object.keys(records[0]), count: records.length, truncated: false, errors: [] });
    }
    if (/\/ops\/jc_pipeline_try_mapping$/.test(path)) {
      stub.tried?.push(JSON.parse(request.postData() ?? "{}") as Record<string, unknown>);
      return json({ input: {}, records: RECORDS, errors: [] });
    }
    if (/\/ops\/jc_pipeline_validate$/.test(path)) {
      stub.seen.push(JSON.parse(request.postData() ?? "{}") as Record<string, unknown>);
      return json(stub.validate, stub.status ?? 200);
    }
    if (request.method() !== "GET") {
      return json({ kind: "Change", metadata: { name: "chg-1", namespace: "helsinki" }, status: { phase: "PendingApproval" } }, 202);
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/pipelines")) return json(list([PIPELINE]));
    if (path.endsWith("/datasources")) {
      return json(list([{ kind: "DataSource", metadata: { name: "hfp", namespace: "helsinki" }, spec: { type: "http", http: { url: "https://hfp.hsl.fi/vehicles.json" } } }]));
    }
    if (path.endsWith("/endpoints")) {
      return json(list([{ kind: "Endpoint", metadata: { name: "helsinki-all", namespace: "helsinki" }, spec: { contextSpaceRef: "mobility", slug: "vq7m2z4tk6xh3n5jb2ryd3wcfa", audience: "organization" } }]));
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string, steps = ["workbench-output", "workbench-validation"]): Promise<void> {
  const dir = process.env.JC_SHOTS;
  if (!dir) return;
  for (const [width, label] of [
    [1440, "desktop"],
    [400, "phone"],
  ] as const) {
    await page.setViewportSize({ width, height: 900 });
    for (const step of steps) {
      const section = page.locator(`section[aria-labelledby="${step}-title"]`);
      await section.scrollIntoViewIfNeeded();
      await section.screenshot({ path: `${dir}/record-check-${name}-${step.replace("workbench-", "")}-${label}.png` });
    }
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

async function openEditor(page: Page) {
  await page.goto("/projects/helsinki/pipelines?lang=en");
  const row = page.getByRole("row").filter({ hasText: "hsl-hfp-vehicles" });
  await row.getByRole("button", { name: /More actions/ }).click();
  await page.getByRole("menuitem", { name: "Edit" }).click();
  return page.getByTestId("form-page");
}

const output = (page: Page) => page.locator('section[aria-labelledby="workbench-output-title"]');
const validation = (page: Page) => page.locator('section[aria-labelledby="workbench-validation-title"]');

test.describe("the record check in the pipeline workbench", () => {
  test("a green check shows the expiry each record is written with", async ({ page }) => {
    const stub: Stub = { validate: GREEN, seen: [], tried: [] };
    await stubApi(page, stub);
    await openEditor(page);
    await expect(validation(page).getByRole("status")).toContainText("All 2 records are valid against hsl-transit 1.0.0.");
    // Each record is a table of its attributes (T-4842): the ends sit in its Expires column.
    const first = output(page).getByTestId("workbench-record-0");
    await expect(first.getByRole("row").filter({ hasText: "expiresAt" })).toContainText("2026-10-20T20:08:00Z");
    await expect(first.getByRole("row").filter({ hasText: "speed" })).toContainText("2026-10-20T20:03:00Z");
    // The run of the mapping keeps its record check on the draft the dialog edits, for the
    // proposal (PF-57); the validation of the records shown back keeps nothing (T-4840).
    expect(stub.tried?.at(-1)?.draft).toEqual({ kind: "Pipeline", name: "hsl-hfp-vehicles" });
    expect(stub.seen.at(-1)?.draft).toBeUndefined();
    await shot(page, "green");
  });

  test("a refused record names the model, the Endpoint and the lifetime", async ({ page }) => {
    await stubApi(page, { validate: REFUSED, seen: [] });
    await openEditor(page);
    await expect(validation(page).getByRole("status")).toContainText("1 record breaks hsl-transit 1.0.0 and would not be written; 1 valid.");
    const row = output(page).getByTestId("workbench-record-1");
    await expect(row).toContainText("not writable through helsinki-all");
    for (const source of ["Model", "Endpoint", "Lifetime"]) {
      await expect(row.getByText(source, { exact: true })).toBeVisible();
    }
    await shot(page, "refused");
  });

  test("a check that cannot run says why", async ({ page }) => {
    await stubApi(page, {
      validate: { type: "about:blank", title: "Service Unavailable", status: 503, detail: "The pipeline runner did not answer; try again in a minute." },
      status: 503,
      seen: [],
    });
    await openEditor(page);
    await expect(validation(page).getByRole("alert")).toBeVisible();
    await shot(page, "error");
  });

  test("an XML source's nested values read as words in the sample, never as JSON (T-4972)", async ({ page }) => {
    // Bento reads an XML root's namespace declarations as `-prefix` members of one object.
    const xml = [{ FeatureCollection: { "-BsWfs": "http://xml.fmi.fi/schema/wfs/2.0", "-gml": "http://www.opengis.net/gml/3.2" }, numberMatched: "24" }];
    await stubApi(page, { validate: GREEN, sample: xml, seen: [] });
    await openEditor(page);
    const sample = page.locator('section[aria-labelledby="workbench-sample-title"]').getByRole("table", { name: "Sample records" });
    await expect(sample.getByRole("cell").first()).toHaveText("-BsWfs: http://xml.fmi.fi/schema/wfs/2.0; -gml: http://www.…");
    await expect(sample).not.toContainText('{"');
    await shot(page, "xml", ["workbench-sample"]);
  });
});

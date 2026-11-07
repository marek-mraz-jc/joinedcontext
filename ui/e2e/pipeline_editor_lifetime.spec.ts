import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { greenVerdict, isCheck } from "./verdict";

// T-4842: the pipeline editor's Target card (the space's model and what the Endpoint lets the
// pipeline write), the Mapped output as each record's attributes with their verdicts, and the
// Lifetime step whose choice the proposal carries as spec.lifetime. The API is answered in the
// browser; JC_SHOTS=<dir> saves each state at 1440 and 400 px for the review.

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const LINKML = [
  "id: https://hel.fi/models/helsinki",
  "name: helsinki",
  "classes:",
  "  Vehicle:",
  "    slots: [location, route, speed, delay, transportMode, nextStop, observedOn]",
  "  Event:",
  "    slots: [name, startDate]",
  "slots:",
  "  location: { range: GeoJSONPoint }",
  "  route: { range: string }",
  "  speed: { range: float, unit: { ucum_code: m/s } }",
  "  delay: { range: integer }",
  "  transportMode: { range: TransportMode }",
  "  nextStop: { range: Stop }",
  "  observedOn: { range: datetime }",
  "  name: { range: string }",
  "  startDate: { range: datetime }",
  "enums:",
  "  TransportMode:",
  "    permissible_values: { bus: {}, tram: {}, metro: {}, ferry: {} }",
  "",
].join("\n");

const ATTRIBUTES = {
  model: "helsinki",
  version: "v1",
  type: "Vehicle",
  attributes: [
    { name: "location", kind: "GeoProperty", valueType: "object", required: false },
    { name: "route", kind: "Property", valueType: "string", required: false },
    { name: "speed", kind: "Property", valueType: "number", required: false, unit: { code: "MTS" }, minimum: 0 },
    { name: "delay", kind: "Property", valueType: "integer", required: false },
    { name: "transportMode", kind: "Property", valueType: "string", required: true, values: ["bus", "tram", "metro", "ferry"] },
    { name: "nextStop", kind: "Relationship", required: false, relationship: { target: "Stop", many: false } },
    { name: "observedOn", kind: "Property", valueType: "string", format: "date-time", required: false },
  ],
};

const WRITABLE = {
  writable: [
    { name: "location", writable: true },
    { name: "route", writable: true },
    { name: "speed", writable: true },
    { name: "delay", writable: true },
    { name: "transportMode", writable: true },
    { name: "nextStop", writable: false, reason: "no Policy bound to helsinki-all grants nextStop: not writable through helsinki-all" },
    { name: "observedOn", writable: true },
  ],
};

const SAMPLE = [
  { veh: "1806", desi: "550", spd: 8.4, dl: "+42", lat: 60.1699, long: 24.9384, tst: "2026-10-20T17:41:07Z" },
  { veh: "1807", desi: "4", spd: 5.1, dl: 12, lat: 60.171, long: 24.94, tst: "2026-10-20T17:41:09Z" },
];

const RECORDS = [
  {
    id: "urn:ngsi-ld:Vehicle:hel.fi:helsinki:1806",
    type: "Vehicle",
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.9384, 60.1699] } },
    route: { type: "Property", value: "550", observedAt: "2026-10-20T17:41:07Z" },
    speed: { type: "Property", value: 8.4, unitCode: "MTS", observedAt: "2026-10-20T17:41:07Z" },
    delay: { type: "Property", value: "+42", observedAt: "2026-10-20T17:41:07Z" },
    transportMode: { type: "Property", value: "bus" },
  },
  {
    id: "urn:ngsi-ld:Vehicle:hel.fi:helsinki:1807",
    type: "Vehicle",
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.94, 60.171] } },
    route: { type: "Property", value: "4", observedAt: "2026-10-20T17:41:09Z" },
    delay: { type: "Property", value: 12, observedAt: "2026-10-20T17:41:09Z" },
    transportMode: { type: "Property", value: "tram" },
  },
];

const verdicts = (refused: boolean) => ({
  space: "helsinki",
  model: "helsinki",
  version: "v1",
  valid: refused ? 1 : 2,
  rejected: refused ? 1 : 0,
  verdicts: [
    refused
      ? {
          index: 0,
          ok: false,
          problems: [],
          endpoint: "helsinki-all",
          findings: [{ source: "model", rule: "sh:datatype", field: "delay", reason: "the model wants a whole number of seconds; the feed sends text like \"+42\": map it with .number()" }],
          expires: { entity: "2026-10-20T17:51:07Z", attributes: {} },
        }
      : { index: 0, ok: true, problems: [], endpoint: "helsinki-all", findings: [], expires: { entity: "2026-10-20T17:51:07Z", attributes: {} } },
    { index: 1, ok: true, problems: [], endpoint: "helsinki-all", findings: [], expires: { entity: "2026-10-20T17:51:09Z", attributes: {} } },
  ],
});

const MAPPING = 'root.id = "urn:ngsi-ld:Vehicle:hel.fi:helsinki:" + this.veh\nroot.type = "Vehicle"\nroot.delay = { "type": "Property", "value": this.dl }';

const PIPELINE = {
  apiVersion: "joinedcontext.com/v1alpha2",
  kind: "Pipeline",
  metadata: { name: "hsl-hfp-vehicles", namespace: "helsinki" },
  spec: {
    class: "resident",
    sources: [{ dataSourceRef: { kind: "DataSource", name: "hfp" } }],
    steps: [{ kind: "bloblang", bloblang: MAPPING }],
    outputs: [{ targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all", type: "Vehicle" }],
  },
  status: { phase: "Live" },
};

interface Stub {
  refused: boolean;
  proposed: Record<string, unknown>[];
}

async function stubApi(page: Page, stub: Stub): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const body = () => JSON.parse(request.postData() ?? "{}") as Record<string, unknown>;
    if (path.endsWith("/auth/me")) {
      return json({ subject: "s1", username: "aino", name: "Aino Virtanen", email: "aino@hel.fi", roles: ["portal-editor"] });
    }
    if (path.endsWith("/branding")) {
      return json({ instanceName: "joinedcontext", orgDomain: "hel.fi", languages: { default: "en", offered: ["en"] } });
    }
    if (isCheck(request.method(), request.url())) {
      stub.proposed.push(body());
      return json(greenVerdict(request.postData()));
    }
    if (/\/ops\/jc_pipeline_sample_source$/.test(path)) {
      return json({ records: SAMPLE, fields: Object.keys(SAMPLE[0]), count: SAMPLE.length, truncated: false, errors: [] });
    }
    if (/\/ops\/jc_pipeline_try_mapping$/.test(path)) return json({ input: {}, records: RECORDS, errors: [] });
    if (/\/ops\/jc_pipeline_validate$/.test(path)) return json(body().type ? WRITABLE : verdicts(stub.refused));
    if (path.endsWith("/types/Vehicle/attributes")) return json(ATTRIBUTES);
    if (request.method() !== "GET") {
      stub.proposed.push(body());
      return json({ kind: "Change", metadata: { name: "chg-1", namespace: "helsinki" }, status: { phase: "PendingApproval" } }, 202);
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/pipelines")) return json(list([PIPELINE]));
    if (path.endsWith("/spaces")) {
      return json(list([{ kind: "ContextSpace", metadata: { name: "helsinki", namespace: "helsinki" }, spec: { dataModelRef: { kind: "DataModel", name: "helsinki" } } }]));
    }
    if (path.endsWith("/datamodels")) {
      return json(list([{ kind: "DataModel", metadata: { name: "helsinki", namespace: "helsinki" }, spec: { linkml: LINKML, version: "v1" } }]));
    }
    if (path.endsWith("/datasources")) {
      return json(list([{ kind: "DataSource", metadata: { name: "hfp", namespace: "helsinki" }, spec: { type: "http", http: { url: "https://hfp.hsl.fi/vehicles.json" } } }]));
    }
    if (path.endsWith("/endpoints")) {
      return json(list([{ kind: "Endpoint", metadata: { name: "helsinki-all", namespace: "helsinki" }, spec: { contextSpaceRef: "helsinki", slug: "vq7m2z4tk6xh3n5jb2ryd3wcfa", audience: "organization" } }]));
    }
    return json(list([]));
  });
}

const section = (page: Page, step: string) => page.locator(`section[aria-labelledby="workbench-${step}-title"]`);

async function shot(page: Page, name: string, steps: string[]): Promise<void> {
  const dir = process.env.JC_SHOTS;
  if (!dir) return;
  for (const [width, label] of [
    [1440, "desktop"],
    [400, "phone"],
  ] as const) {
    await page.setViewportSize({ width, height: 900 });
    for (const step of steps) {
      await section(page, step).scrollIntoViewIfNeeded();
      await section(page, step).screenshot({ path: `${dir}/editor-${name}-${step}-${label}.png` });
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

test.describe("the pipeline editor's target, output and lifetime", () => {
  test("the Target card names what the Endpoint lets the pipeline write, with the reason it does not", async ({ page }) => {
    await stubApi(page, { refused: true, proposed: [] });
    await openEditor(page);
    const card = page.getByTestId("workbench-target-card");
    await expect(card).toContainText("helsinki v1");
    await expect(card).toContainText("Vehicle");
    const nextStop = card.getByRole("row").filter({ hasText: "nextStop" });
    await expect(nextStop).toContainText("not writable through helsinki-all");
    // The slots shown under the mapping are the written type's, never the model's first class.
    await expect(section(page, "mapping")).toContainText("What Vehicle takes");
    // A red record names the rule at its field.
    const first = page.getByTestId("workbench-record-0");
    await expect(first.getByRole("row").filter({ hasText: "delay" })).toContainText("sh:datatype");
    await expect(section(page, "validation").getByRole("status")).toContainText("1 record breaks helsinki v1");
    await shot(page, "red", ["target", "output", "lifetime", "validation"]);
  });

  test("each lifetime mode reads as a sentence and the proposal carries spec.lifetime", async ({ page }) => {
    const stub: Stub = { refused: false, proposed: [] };
    await stubApi(page, stub);
    await openEditor(page);
    const lifetime = section(page, "lifetime");
    const says = lifetime.getByTestId("pipeline-lifetime-sentence");
    await expect(says).toContainText("Each Vehicle stays until someone deletes it");
    await expect(section(page, "validation").getByRole("status")).toContainText("All 2 records are valid");
    await shot(page, "green-keep", ["output", "lifetime", "validation"]);

    await lifetime.getByRole("radio", { name: /Whole entity expires/ }).check();
    await expect(says).toContainText("Each Vehicle is removed 10 minutes after its last observedAt");
    await shot(page, "entity-from", ["lifetime"]);

    await lifetime.getByRole("radio", { name: /After each write/ }).check();
    await expect(says).toContainText("10 minutes after its last write");
    await shot(page, "entity-rolling", ["lifetime"]);

    await lifetime.getByRole("radio", { name: /On a fixed date/ }).check();
    await lifetime.getByLabel(/^Date and time/).fill("2030-12-31T23:00");
    await expect(says).toContainText("Each Vehicle is removed on");
    await shot(page, "entity-fixed-date", ["lifetime"]);

    await lifetime.getByLabel(/^Date and time/).fill("2020-01-01T08:00");
    await expect(lifetime).toContainText("The date is in the past");
    await shot(page, "entity-past-date", ["lifetime"]);

    await lifetime.getByRole("radio", { name: /Only some fields expire/ }).check();
    // nextStop is not writable here, so no lifetime may name it.
    await expect(lifetime.getByRole("combobox", { name: "Field" }).first().locator("option")).not.toContainText(["nextStop"]);
    await expect(says).toContainText("The location of each Vehicle vanishes 10 minutes after its last observedAt");
    await lifetime.getByRole("button", { name: "Add a field" }).click();
    await shot(page, "only-some-fields", ["lifetime"]);

    // The draft's check and the proposal judge the manifest with the lifetime chosen above.
    await page.getByRole("button", { name: "Propose change" }).click();
    await expect.poll(() => stub.proposed.filter((one) => (one as { spec?: unknown }).spec).length).toBeGreaterThan(0);
    const proposed = stub.proposed.filter((one) => (one as { spec?: unknown }).spec).at(-1) as { spec?: { lifetime?: unknown } };
    expect(proposed.spec?.lifetime).toEqual([
      {
        type: "Vehicle",
        attributes: [
          { names: ["location"], from: "observedAt", plus: "PT10M" },
          { names: ["route"], from: "observedAt", plus: "PT10M" },
        ],
      },
    ]);
  });
});

import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { greenVerdict, isCheck } from "./verdict";

// T-3919 (PL-52, PL-56, PL-68): the three pipelines of the CIVITAS/CORE comparison
// (docs/analysis/pipeline-studio-vs-nifi.md), built from nothing on the canvas with the palette
// only: a source, a processor found by search, a compute step, an output, a test on a sample,
// a check and a proposed change. The API is stubbed; T-3967 runs the same walks live on dev.
// `STUDIO_SHOTS=<dir>` saves a screenshot of every step for the comparison page.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@hel.fi",
  roles: ["portal-editor"],
};

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
  colours: {
    primary: "#1d4ed8",
    secondary: "#0f766e",
    accent: "#f59e0b",
    background: "#ffffff",
    text: "#0f172a",
  },
  fonts: { heading: "system-ui, sans-serif", body: "system-ui, sans-serif" },
  languages: { default: "en", offered: ["en"] },
  primaryForeground: "#ffffff",
};

const list = (items: unknown[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items,
});

const dataSource = (name: string, spec: unknown) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataSource",
  metadata: { name, namespace: "helsinki" },
  spec,
});

const DATASOURCES = list([
  dataSource("hsl-hfp", {
    type: "mqtt",
    mqtt: {
      urls: ["tls://mqtt.hsl.fi:8883"],
      topics: ["/hfp/v2/journey/ongoing/vp/bus/#"],
    },
  }),
  dataSource("air-poll", {
    type: "http",
    http: { url: "https://opendata.hel.fi/air/latest.json", interval: "5m" },
  }),
  dataSource("parking-csv", {
    type: "http",
    http: { url: "https://opendata.hel.fi/parking/zones.csv", interval: "24h", format: "csv" },
  }),
  // The same file without `format: csv`: the runner sends it whole (PL-114, T-5350).
  dataSource("parking-whole", {
    type: "http",
    http: { url: "https://opendata.hel.fi/parking/zones.csv", interval: "24h" },
  }),
]);

const ENDPOINTS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "transport", namespace: "helsinki" },
    spec: {
      contextSpaceRef: "transport",
      slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
      audience: "public",
    },
  },
]);

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-studio-1", namespace: "helsinki" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
};

// The caller's grants on the project: an editor proposes, a viewer only reads.
const grants = (verbs: string[]) => ({
  project: "helsinki",
  bootstrap: false,
  grants: [
    {
      role: verbs.includes("propose") ? "steward" : "viewer",
      binding: "helsinki-people",
      scope: "project:helsinki",
      rule: { kinds: ["Pipeline", "DataSource", "Endpoint"], verbs },
    },
  ],
});

async function stubApi(
  page: Page,
  trace: unknown,
  verbs = ["read", "propose"],
): Promise<{ writes: string[] }> {
  const writes: string[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({
        status,
        contentType: "application/json",
        body: JSON.stringify(body),
      });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path.endsWith("/permissions/me")) return json(grants(verbs));
    if (path.endsWith("/branding")) return json(BRANDING);
    if (isCheck(request.method(), request.url()))
      return json(greenVerdict(request.postData()));
    if (path.endsWith("/pipelines/test")) return json(trace);
    const draft =
      /^\/api\/v1\/projects\/([^/]+)\/drafts\/([^/]+)\/([^/]+)$/.exec(path);
    if (draft && request.method() === "PUT") {
      const { manifest } = JSON.parse(request.postData() ?? "{}") as {
        manifest: unknown;
      };
      const [, project, kind, name] = draft;
      return json({
        project,
        kind,
        name,
        manifest,
        touchedBy: "jana.kovacova",
        touchedKind: "person",
        version: 1,
        updatedAt: new Date().toISOString(),
      });
    }
    if (/\/ops\/jc_pipeline_(sample_source|try_mapping|validate)$/.test(path)) {
      return json({
        records: [],
        fields: [],
        count: 0,
        truncated: false,
        errors: [],
      });
    }
    if (request.method() !== "GET") {
      writes.push(`${request.method()} ${path} ${request.postData() ?? ""}`);
      return json(CHANGE, 202);
    }
    if (path === "/api/v1/projects")
      return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/datasources")) return json(DATASOURCES);
    if (path.endsWith("/endpoints")) return json(ENDPOINTS);
    return json(list([]));
  });
  return { writes };
}

// One screenshot of the step, when the comparison page is being made.
async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.STUDIO_SHOTS;
  if (dir) await page.screenshot({ path: `${dir}/${name}.png` });
}

interface Walk {
  name: string;
  source: string;
  processor: string;
  mapping: string;
  sample: { name: string; mimeType: string; body: string };
  entity: { id: string; type: string };
}

const WALKS: Walk[] = [
  {
    name: "hsl-vehicles",
    source: "hsl-hfp",
    processor: "log",
    mapping:
      'root.id = "urn:ngsi-ld:Vehicle:" + this.VP.veh.string()\nroot.type = "Vehicle"\nroot.speed = this.VP.spd',
    sample: {
      name: "hfp.json",
      mimeType: "application/json",
      body: JSON.stringify([
        { VP: { veh: 1234, spd: 8.4, lat: 60.17, long: 24.94 } },
      ]),
    },
    entity: { id: "urn:ngsi-ld:Vehicle:1234", type: "Vehicle" },
  },
  {
    name: "air-quality",
    source: "air-poll",
    processor: "switch",
    mapping:
      'root = if this.pm10 == null { deleted() } else { {"id": "urn:ngsi-ld:AirQualityObserved:" + this.station, "type": "AirQualityObserved", "pm10": this.pm10} }',
    sample: {
      name: "air.json",
      mimeType: "application/json",
      body: JSON.stringify([
        { station: "kallio", pm10: 12 },
        { station: "mannerheimintie" },
      ]),
    },
    entity: {
      id: "urn:ngsi-ld:AirQualityObserved:kallio",
      type: "AirQualityObserved",
    },
  },
  {
    name: "parking-zones",
    source: "parking-csv",
    processor: "unarchive",
    mapping:
      'root.id = "urn:ngsi-ld:ParkingZone:" + this.zone\nroot.type = "ParkingZone"\nroot.fee = this.fee.number()',
    sample: {
      name: "zones.csv",
      mimeType: "text/csv",
      body: "zone,fee\n1,4.0\n2,3.0\n",
    },
    entity: { id: "urn:ngsi-ld:ParkingZone:1", type: "ParkingZone" },
  },
];

test.describe("pipeline studio walk (CIVITAS/CORE comparison)", () => {
  for (const walk of WALKS) {
    test(`builds ${walk.name} from the palette, tests it on a sample and proposes it`, async ({
      page,
    }) => {
      const trace = {
        input: { events: 1, bytes: walk.sample.body.length, sample: {} },
        mapping: [walk.entity],
        validation: [{ index: 0, ok: true, problems: [] }],
        errors: [],
      };
      const { writes } = await stubApi(page, trace);
      await page.goto("/projects/helsinki/pipelines?lang=en");
      await page.getByRole("button", { name: "New pipeline" }).first().click();
      const dialog = page.getByTestId("form-page");
      await dialog.getByLabel(/^Name/).fill(walk.name);
      await dialog.getByText("More options").click();
      await dialog.getByText("Advanced editor").click();
      const canvas = page.getByTestId("flow-canvas");
      await canvas.scrollIntoViewIfNeeded();
      await shot(page, `${walk.name}-1-empty-canvas`);

      // The source: a DataSource of the project, picked on the source node itself (T-3978).
      await page.getByTestId("flow-node-source").dblclick();
      await page
        .getByTestId("flow-node-editor-source")
        .getByTestId("flow-source-ref")
        .selectOption(`DataSource:${walk.source}`);
      await shot(page, `${walk.name}-2-source`);

      // A processor found by search, dragged onto the canvas, and the compute step.
      await page.getByLabel("Find a processor").fill(walk.processor);
      const found = page.getByTestId(`palette-found-${walk.processor}`);
      await found.scrollIntoViewIfNeeded();
      await found.dragTo(canvas);
      await expect(page.getByTestId("flow-node-step-0")).toBeVisible();
      await page.getByTestId("palette-bloblang").dragTo(canvas);
      await expect(page.getByTestId("flow-node-compute")).toBeVisible();
      // The mapping is typed before the sample lands, and the sample keeps it: the type comes
      // from the mapping's own literal, not from the file's name (T-3971).
      await page.getByTestId("flow-bloblang").fill(walk.mapping);
      await dialog
        .getByTestId("pipeline-studio")
        .locator('input[type="file"]')
        .setInputFiles({
          name: walk.sample.name,
          mimeType: walk.sample.mimeType,
          buffer: Buffer.from(walk.sample.body),
        });
      await expect(page.getByTestId("flow-bloblang")).toHaveValue(walk.mapping);
      await expect(dialog.getByTestId("replace-drafted")).toBeVisible();
      await canvas.scrollIntoViewIfNeeded();
      await shot(page, `${walk.name}-3-steps`);

      // The output: the project's Endpoint, picked on the output node itself (T-3978).
      await page.getByTestId("flow-node-output").dblclick();
      await page
        .getByTestId("flow-node-editor-output")
        .getByTestId("flow-output-target")
        .selectOption({ label: "transport" });
      await shot(page, `${walk.name}-4-output`);

      // A test on a sample: the mapped entity comes back and the compute node is green.
      await page.getByRole("button", { name: "Test mapping" }).click();
      await expect(page.getByTestId("flow-node-compute")).toHaveAttribute(
        "data-state",
        "ok",
      );
      await shot(page, `${walk.name}-5-tested`);

      // Checked, then proposed as a change for approval.
      await dialog.getByRole("button", { name: "Check", exact: true }).click();
      await expect(dialog.getByTestId("draft-verdict")).toContainText(
        "Checked",
      );
      await dialog.getByRole("button", { name: "Propose change" }).click();
      await expect(page.getByText("chg-studio-1")).toBeVisible();
      await shot(page, `${walk.name}-6-proposed`);

      const posted = writes.find((write) =>
        write.startsWith("POST /api/v1/projects/helsinki/pipelines "),
      );
      expect(posted, writes.join("\n")).toBeTruthy();
      const manifest = JSON.parse(posted!.split(" ").slice(2).join(" ")) as {
        spec: {
          sources: unknown[];
          steps: { kind?: string }[];
          outputs: unknown[];
        };
      };
      expect(manifest.spec.sources).toEqual([
        { dataSourceRef: { kind: "DataSource", name: walk.source } },
      ]);
      expect(
        manifest.spec.steps.map((step) => Object.keys(step)),
        JSON.stringify(manifest),
      ).toEqual([["processor"], ["kind", "bloblang"]]);
      expect(manifest.spec.outputs).toEqual([
        {
          targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:transport:transport",
          type: walk.entity.type,
          mode: "upsert",
        },
      ]);
    });
  }

  // T-5350, PL-114: a sample is split the way the live run splits it. A CSV http source without
  // `format: csv` sends its whole body as one message, so the test does too and says which field
  // reads it a row at a time; with the field the rows go one message each.
  for (const [source, format, width] of [
    ["parking-whole", "text", 1440],
    ["parking-whole", "text", 400],
    ["parking-csv", "csv", 1440],
    ["parking-csv", "csv", 400],
  ] as const) {
    test(`tests a CSV sample of ${source} as the runner sends it at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, {
        input: { events: 1, bytes: 40, sample: {} },
        mapping: [{ id: "urn:ngsi-ld:ParkingZone:1", type: "ParkingZone" }],
        validation: [{ index: 0, ok: true, problems: [] }],
        errors: [],
      });
      await page.goto("/projects/helsinki/pipelines?lang=en");
      await page.getByRole("button", { name: "New pipeline" }).first().click();
      const dialog = page.getByTestId("form-page");
      await dialog.getByText("More options").click();
      await dialog.getByText("Advanced editor").click();
      await page.getByTestId("flow-node-source").dblclick();
      await page.getByTestId("flow-node-editor-source").getByTestId("flow-source-ref").selectOption(`DataSource:${source}`);
      await page.getByTestId("palette-bloblang").click();
      await page.getByTestId("flow-bloblang").fill("root = this");
      await dialog
        .getByTestId("pipeline-studio")
        .locator('input[type="file"]')
        .setInputFiles({ name: "zones.csv", mimeType: "text/csv", buffer: Buffer.from("zone,tariff\n1,2.40\n2,1.20\n") });
      const hint = dialog.getByTestId("test-whole-body");
      if (format === "text") {
        await expect(hint).toContainText(
          "The source sends a CSV file as one message, so this test does too. To map it one row at a time, add format: csv to the data source's http block (spec.http.format: csv).",
        );
      } else {
        await expect(hint).toHaveCount(0);
      }
      const sent = page.waitForRequest((request) => request.url().endsWith("/pipelines/test"));
      await page.getByRole("button", { name: "Test mapping" }).click();
      const body = (await sent).postDataJSON() as { sample: { format: string } };
      expect(body.sample.format).toBe(format);
      await dialog.getByTestId("pipeline-studio").getByRole("heading", { name: "Try it on a sample" }).scrollIntoViewIfNeeded();
      await shot(page, `csv-${source}-${width}`);
    });
  }

  // A source reads from outside the pipeline: it has no input port to wire into (WireRefusal
  // `intoSource` in pipelineGraph.ts for the keyboard path).
  test("offers no wire into a source", async ({ page }) => {
    await stubApi(page, {});
    await page.goto("/projects/helsinki/pipelines?lang=en");
    await page.getByRole("button", { name: "New pipeline" }).first().click();
    const dialog = page.getByTestId("form-page");
    await dialog.getByText("More options").click();
    await dialog.getByText("Advanced editor").click();
    await expect(page.getByTestId("flow-node-source")).toBeVisible();
    await expect(page.getByTestId("flow-port-out-source")).toHaveCount(1);
    await expect(page.getByTestId("flow-port-in-source")).toHaveCount(0);
    // A wire dragged onto the source anyway says why it takes none (T-4271).
    await page.getByTestId("palette-bloblang").click();
    await page.getByTestId("flow-port-out-compute").dispatchEvent("pointerdown");
    await page.getByTestId("flow-node-source").dispatchEvent("pointerup");
    await expect(page.getByTestId("flow-said")).toHaveText(
      "A source reads from outside the pipeline; nothing wires into it.",
    );
  });

  // A required field left empty proposes nothing: the check is not sent, the form counts what is
  // missing, and the Check names the field and brings its control into view (T-3970).
  test("proposes nothing while the target endpoint is empty", async ({
    page,
  }) => {
    const { writes } = await stubApi(page, {});
    await page.goto("/projects/helsinki/pipelines?lang=en");
    await page.getByRole("button", { name: "New pipeline" }).first().click();
    const dialog = page.getByTestId("form-page");
    await dialog.getByLabel(/^Name/).fill("no-target");
    await dialog.getByRole("button", { name: "Check", exact: true }).click();
    await expect(dialog.getByTestId("required-count")).toHaveText(
      "2 of 3 required fields filled",
    );
    await expect(dialog.getByTestId("check-stopped")).toHaveText(
      "Target endpoint is required",
    );
    await expect(dialog.locator('[data-field="targetEndpoint"]')).toBeFocused();
    await expect(dialog.getByTestId("draft-verdict")).toHaveText(
      "Not checked yet",
    );
    await expect(
      dialog.getByRole("button", { name: "Propose change" }),
    ).toBeDisabled();
    expect(writes).toEqual([]);
  });

  // A step drawn on the canvas with no first source cannot be written (v1alpha2 needs one), so
  // Check and Propose stop and say so rather than propose the pipeline without it (T-3969).
  test("refuses a step drawn with no first source rather than drop it", async ({
    page,
  }) => {
    const { writes } = await stubApi(page, {});
    await page.goto("/projects/helsinki/pipelines?lang=en");
    await page.getByRole("button", { name: "New pipeline" }).first().click();
    const dialog = page.getByTestId("form-page");
    await dialog.getByLabel(/^Name/).fill("no-source");
    await dialog.getByText("More options").click();
    await dialog.getByText("Advanced editor").click();
    const canvas = page.getByTestId("flow-canvas");
    await page.getByLabel("Find a processor").fill("log");
    const found = page.getByTestId("palette-found-log");
    await found.scrollIntoViewIfNeeded();
    await found.dragTo(canvas);
    await expect(page.getByTestId("flow-node-step-0")).toBeVisible();
    await dialog.getByLabel(/^Target endpoint/).selectOption({ label: "transport" });
    await dialog.getByRole("button", { name: "Check", exact: true }).click();
    await expect(dialog.getByTestId("check-stopped")).toContainText(
      "The pipeline has no first source",
    );
    await expect(dialog.locator('[data-field="source"]')).toBeFocused();
    await expect(
      dialog.getByRole("button", { name: "Propose change" }),
    ).toBeDisabled();
    expect(writes).toEqual([]);
  });

  // A viewer reads pipelines; the control that would build one is disabled with the reason
  // (PermissionGuard), and nothing is written.
  test("gives a viewer no working edit control", async ({ page }) => {
    const { writes } = await stubApi(page, {}, ["read"]);
    await page.goto("/projects/helsinki/pipelines?lang=en");
    const add = page.getByRole("button", { name: "New pipeline" });
    await expect(add.first()).toBeVisible();
    for (const button of await add.all()) await expect(button).toBeDisabled();
    await expect(page.getByTestId("form-page")).toHaveCount(0);
    expect(writes).toEqual([]);
  });
});

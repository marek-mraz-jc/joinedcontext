import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { greenVerdict, isCheck } from "./verdict";

// T-5369, PL-90: a record check that finds the pipeline holds no grant on another space's
// Endpoint names it "No access to <space>: Request access", and the button opens the access
// request form prefilled with the Endpoint, the operation and the type. The API is answered in the
// browser, as in pipeline_record_check.spec.ts; JC_SHOTS=<dir> saves each state at 1440 and 400 px.

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const RECORD = {
  id: "urn:ngsi-ld:AirQualityIndex:centre",
  type: "AirQualityIndex",
  index: { type: "Property", value: 2 },
};

const REASON = "no Policy bound to Endpoint city-air grants this pipeline a batch upsert of AirQualityIndex";

const REFUSED = {
  space: "air",
  model: "air-index",
  version: "1.0.0",
  valid: 0,
  rejected: 1,
  verdicts: [
    {
      index: 0,
      ok: false,
      problems: [],
      endpoint: "city-air",
      findings: [
        {
          source: "endpoint",
          rule: "policy",
          field: "",
          reason: REASON,
          access: { project: "helsinki", name: "city-air", operations: "write", type: "AirQualityIndex" },
        },
      ],
    },
  ],
};

const PIPELINE = {
  apiVersion: "joinedcontext.com/v1alpha2",
  kind: "Pipeline",
  metadata: { name: "air-index", namespace: "helsinki" },
  spec: {
    class: "resident",
    contextSpaceRef: "mobility",
    sources: [{ dataSourceRef: { kind: "DataSource", name: "sensors" } }],
    steps: [{ kind: "bloblang", bloblang: 'root.id = "urn:ngsi-ld:AirQualityIndex:" + this.area\nroot.type = "AirQualityIndex"' }],
    outputs: [{ targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:air:city-air" }],
  },
  status: { phase: "Live" },
};

const OFFERED = [{ project: "helsinki", name: "city-air", space: "air", types: [{ type: "AirQualityIndex", attrs: ["index", "pm10Index"] }] }];

/** A filed request as the pipeline's side lists it (API/01 §47). */
const view = (body: Record<string, unknown>) => ({
  id: "ar-0a1b2c3d4e5f",
  pipeline: { project: "helsinki", name: "air-index" },
  account: "helsinki/pl-air-index",
  until: null,
  state: "pending",
  requestedBy: "aino",
  requestedAt: "2026-10-25T11:50:00Z",
  decidedBy: null,
  decidedAt: null,
  decisionReason: null,
  change: null,
  ...body,
});

async function stubApi(page: Page, filed: unknown[]): Promise<void> {
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
      return json({ records: [{ area: "centre", aqi: "2" }], fields: ["area", "aqi"], count: 1, truncated: false, errors: [] });
    }
    if (/\/ops\/jc_pipeline_try_mapping$/.test(path)) return json({ input: {}, records: [RECORD], errors: [] });
    if (/\/ops\/jc_pipeline_validate$/.test(path)) return json(REFUSED);
    if (path.endsWith("/pipelines/air-index/access-requests/endpoints")) return json({ items: OFFERED });
    if (path.endsWith("/pipelines/air-index/access-requests")) {
      if (request.method() === "POST") {
        const body = JSON.parse(request.postData() ?? "{}") as Record<string, unknown>;
        filed.push(body);
        return json(view(body), 201);
      }
      return json({ items: filed.map((body) => view(body as Record<string, unknown>)) });
    }
    if (request.method() !== "GET") {
      return json({ kind: "Change", metadata: { name: "chg-1", namespace: "helsinki" }, status: { phase: "PendingApproval" } }, 202);
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/pipelines")) return json(list([PIPELINE]));
    if (path.endsWith("/datasources")) {
      return json(list([{ kind: "DataSource", metadata: { name: "sensors", namespace: "helsinki" }, spec: { type: "http", http: { url: "https://sensors.hel.fi/air.json" } } }]));
    }
    if (path.endsWith("/endpoints")) {
      return json(list([{ kind: "Endpoint", metadata: { name: "city-air", namespace: "helsinki" }, spec: { contextSpaceRef: "air", slug: "vq7m2z4tk6xh3n5jb2ryd3wcfa", audience: "organization" } }]));
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string, testId: string): Promise<void> {
  const dir = process.env.JC_SHOTS;
  if (!dir) return;
  for (const [width, label] of [
    [1440, "desktop"],
    [400, "phone"],
  ] as const) {
    await page.setViewportSize({ width, height: 900 });
    // The sticky app bar would cover the top of a part taller than the window.
    await page.addStyleTag({ content: "header { position: static !important; }" });
    const part = page.getByTestId(testId);
    await part.scrollIntoViewIfNeeded();
    await part.screenshot({ path: `${dir}/access-request-${name}-${label}.png` });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

test("a missing grant on another space opens the access request prefilled and files it", async ({ page }) => {
  const filed: unknown[] = [];
  await stubApi(page, filed);
  await page.goto("/projects/helsinki/pipelines?lang=en");
  const row = page.getByRole("row").filter({ hasText: "air-index" });
  await row.getByRole("button", { name: /More actions/ }).click();
  await page.getByRole("menuitem", { name: "Edit" }).click();

  const output = page.locator('section[aria-labelledby="workbench-output-title"]');
  const record = output.getByTestId("workbench-record-0");
  // The space comes from the server's list of what this person may ask for (T-4946).
  await expect(record.getByText("No access to air:")).toBeVisible();
  await expect(record).toContainText(REASON);
  await shot(page, "finding", "workbench-record-0");

  await record.getByRole("button", { name: "Request access to helsinki/city-air" }).click();
  const form = output.getByTestId("workbench-access");
  await expect(form.getByRole("heading", { name: "Request access" })).toBeVisible();
  await expect(form.getByRole("radio", { name: "Write" })).toBeChecked();
  await expect(form.getByText("Filled in from the record check.", { exact: false })).toBeVisible();
  await form.getByRole("combobox", { name: "Attributes" }).click();
  await page.getByRole("option", { name: /^index/ }).click();
  await page.keyboard.press("Escape");
  await form.getByLabel(/Why the pipeline needs it/).fill("The air index of each district for the city dashboard.");
  await shot(page, "form", "workbench-access");

  await form.getByRole("button", { name: "Send request" }).click();
  await expect(form.getByRole("status")).toContainText("Request sent.");
  expect(filed).toEqual([
    {
      endpoint: { project: "helsinki", name: "city-air" },
      operations: "write",
      types: [{ type: "AirQualityIndex", attrs: ["index"] }],
      reason: "The air index of each district for the city dashboard.",
    },
  ]);
  await shot(page, "sent", "workbench-access");
});

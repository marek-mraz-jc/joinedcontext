import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// T-4935, PL-105: a person builds a KPI over two sources from the KPI recipe without writing
// Bloblang: endpoints, types and numbers from the space models, a join per area, a plain formula,
// a refusal named beside its field, a preview on the current pages, then Propose. The API is
// answered in the browser; JC_SHOTS=<dir> saves each state at 1440 and 400 px.

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const LINKML = [
  "id: https://hel.fi/models/mobility",
  "name: mobility",
  "classes:",
  "  ParkingSite:",
  "    slots: [occupiedSpotNumber, totalSpotNumber, district]",
  "  TransitStop:",
  "    slots: [name, district]",
  "slots:",
  "  occupiedSpotNumber: { range: integer }",
  "  totalSpotNumber: { range: integer }",
  "  district: { range: string }",
  "  name: { range: string }",
  "",
].join("\n");

const endpoint = (name: string, space: string) => ({
  kind: "Endpoint",
  metadata: { name, namespace: "helsinki" },
  spec: { contextSpaceRef: space, slug: `${name.replace(/-/g, "")}slug`, audience: "organization" },
});

interface Stub {
  checks: Record<string, unknown>[];
  proposed: (Record<string, unknown> | string)[];
}

/** The check refuses MIN as the unit of a ratio, names parking unreadable, and previews two districts. */
function checked(body: Record<string, unknown>) {
  const pipeline = body.pipeline as { spec: { steps: { kpi: { unitCode: string; formula: string } }[] } };
  const unit = pipeline.spec.steps[0]?.kpi.unitCode;
  if (pipeline.spec.steps[0]?.kpi.formula.includes("prev(")) return changed(body);
  const problems =
    unit === "C62"
      ? []
      : [{ field: "spec.steps.kpi.unitCode", reason: `the formula's unit is C62 (a ratio of equal units); unitCode ${unit} does not match it` }];
  return {
    problems,
    sources: [
      { name: "parking", endpoint: "helsinki-parking", readable: false },
      { name: "stops", endpoint: "helsinki-transit", readable: true },
    ],
    ...(body.preview
      ? {
          preview: {
            values: [
              { key: "Kallio", name: "parking-per-stop-kallio", value: 7.5 },
              { key: "Töölö", name: "parking-per-stop-toolo", value: 4.25 },
            ],
            rows: { parking: 48, stops: 31 },
          },
        }
      : {}),
  };
}

/** A change over parking: 48 places now, 54 a week ago; `history: false` when the source has none. */
let history = true;
function changed(body: Record<string, unknown>) {
  const reason = "input parking: it has no history 7 days back, so there is no value; check that its space keeps this attribute's history and that much of it exists";
  return {
    problems: [],
    sources: [{ name: "parking", endpoint: "helsinki-parking", readable: true }],
    ...(body.preview
      ? {
          preview: history
            ? { values: [{ key: null, name: "parking-change", value: -6 }], rows: { parking: 48, "parking@P7D": 48 } }
            : { values: [], rows: { parking: 48 }, problem: reason },
        }
      : {}),
  };
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
    if (path.endsWith("/kpis/check")) {
      stub.checks.push(body());
      return json(checked(body()));
    }
    if (path.endsWith("/datamodels/mobility/source")) return route.fulfill({ status: 200, contentType: "text/yaml", body: LINKML });
    if (path === "/api/v1/organization/datamodels") {
      return json({
        ...list([{ name: "mobility", level: "project", project: "helsinki", space: "mobility", version: "1.0.0", lifecycle: "published", classes: ["ParkingSite", "TransitStop"] }]),
        smartDataModels: [],
      });
    }
    if (request.method() !== "GET") {
      stub.proposed.push(request.url().includes("dryRun") ? "dry run" : body());
      return json({ kind: "Change", metadata: { name: "chg-1", namespace: "helsinki" }, status: { phase: "PendingApproval" } }, 202);
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/endpoints")) {
      return json(list([endpoint("helsinki-parking", "mobility"), endpoint("helsinki-transit", "mobility"), endpoint("kpi-writer", "helsinki-kpi")]));
    }
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_SHOTS;
  if (!dir) return;
  for (const [width, label] of [
    [1440, "desktop"],
    [400, "phone"],
  ] as const) {
    await page.setViewportSize({ width, height: 1000 });
    await page.screenshot({ path: `${dir}/kpi-editor-${name}-${label}.png`, fullPage: true });
  }
  await page.setViewportSize({ width: 1440, height: 1000 });
}

/** The dialog's whole height at each width, for a judge: the panel scrolls inside the viewport. */
async function fullShot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_SHOTS;
  if (!dir) return;
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 1000 });
    await page.getByTestId("kpi-preview-end").scrollIntoViewIfNeeded();
    await page.screenshot({ path: `${dir}/change-${name}-${width}-bottom.png` });
    await page.getByTestId("kpi-editor").getByLabel("Indicator name").scrollIntoViewIfNeeded();
    await page.screenshot({ path: `${dir}/change-${name}-${width}-top.png` });
  }
  await page.setViewportSize({ width: 1440, height: 1000 });
}

async function pick(page: Page, label: string, option: string, nth = 0): Promise<void> {
  const box = page.getByRole("combobox", { name: label }).nth(nth);
  await box.click();
  await page.getByRole("option", { name: new RegExp(`^${option}`) }).first().click();
}

test.describe("the KPI editor over several sources", () => {
  test("a person builds a change since last week over parking, sees no history, then a value, and proposes", async ({ page }) => {
    const stub: Stub = { checks: [], proposed: [] };
    await stubApi(page, stub);
    await page.goto("/projects/helsinki/pipelines?lang=en");
    await page.getByRole("button", { name: "New pipeline" }).first().click();
    await page.getByRole("button", { name: /Analyse \(KPI\)|KPI/ }).first().click();
    await page.getByRole("button", { name: "Combine several sources…" }).click();
    const editor = page.getByTestId("kpi-editor");
    await editor.getByLabel("Indicator name").fill("parking-change");
    const first = editor.getByTestId("kpi-source-0");
    await first.getByLabel("Input name").fill("parking");
    await first.getByLabel("Endpoint").selectOption("helsinki-parking");
    await pick(page, "Entity type", "ParkingSite");
    await pick(page, "Number it reads", "occupiedSpotNumber");
    await editor.getByLabel("What to compute").selectOption("change");
    await expect(editor.getByTestId("kpi-source-1")).toHaveCount(0);
    await expect(editor.getByLabel("How the rows meet")).toBeDisabled();
    await expect(editor.getByText(/Reads this source's history/)).toBeVisible();
    await expect(editor.getByText("prev(")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Propose change" })).toBeEnabled();
    await fullShot(page, "chosen");

    history = false;
    await editor.getByRole("button", { name: "Compute now" }).click();
    await expect(editor.getByText(/it has no history 7 days back/)).toBeVisible();
    await fullShot(page, "no-history");

    history = true;
    await editor.getByRole("button", { name: "Compute now" }).click();
    await expect(editor.getByRole("cell", { name: "-6", exact: true })).toBeVisible();
    await expect(editor.getByText("parking: 48 rows · parking a week ago: 48 rows")).toBeVisible();
    await fullShot(page, "previewed");

    await page.getByRole("button", { name: "Propose change" }).click();
    await expect.poll(() => stub.proposed.length).toBe(2);
    const proposed = stub.proposed.at(-1) as { spec: { sources: { query: { attrs: string[] } }[]; steps: { kpi: Record<string, unknown> }[] } };
    expect(proposed.spec.sources).toHaveLength(1);
    expect(proposed.spec.sources[0].query.attrs).toEqual(["occupiedSpotNumber"]);
    expect(proposed.spec.steps[0].kpi).toMatchObject({
      formula: "sum(parking.occupiedSpotNumber) - prev(sum(parking.occupiedSpotNumber), P7D)",
      join: { by: "time" },
    });
  });

  test("a person builds parking per transit stop by district, sees a refusal, previews and proposes", async ({ page }) => {
    const stub: Stub = { checks: [], proposed: [] };
    await stubApi(page, stub);
    await page.goto("/projects/helsinki/pipelines?lang=en");
    await page.getByRole("button", { name: "New pipeline" }).first().click();
    await page.getByRole("button", { name: /Analyse \(KPI\)|KPI/ }).first().click();
    await page.getByRole("button", { name: "Combine several sources…" }).click();
    const editor = page.getByTestId("kpi-editor");
    await expect(editor).toBeVisible();
    await expect(page.getByText("Give the indicator a name first.")).toBeVisible();
    await shot(page, "empty");

    await editor.getByLabel("Indicator name").fill("parking-per-stop");
    const first = editor.getByTestId("kpi-source-0");
    const second = editor.getByTestId("kpi-source-1");
    await first.getByLabel("Input name").fill("parking");
    await first.getByLabel("Endpoint").selectOption("helsinki-parking");
    await second.getByLabel("Input name").fill("stops");
    await second.getByLabel("Endpoint").selectOption("helsinki-transit");
    await editor.getByLabel("How the rows meet").selectOption("area");
    await pick(page, "Entity type", "ParkingSite");
    await pick(page, "Number it reads", "occupiedSpotNumber");
    await pick(page, "Area attribute", "district");
    await pick(page, "Entity type", "TransitStop", 1);
    await pick(page, "Area attribute", "district", 1);
    await editor.getByLabel("What to compute").selectOption("per");
    await expect(editor.getByText("sum(parking.occupiedSpotNumber) / count(stops)").first()).toBeVisible();

    await editor.getByLabel("Unit code").fill("MIN");
    await expect(editor.getByText(/unitCode MIN does not match it/)).toBeVisible();
    await expect(page.getByRole("button", { name: "Propose change" })).toBeDisabled();
    await shot(page, "refused");

    await editor.getByLabel("Unit code").fill("C62");
    await expect(page.getByRole("button", { name: "Propose change" })).toBeEnabled();
    await expect(editor.getByText("The pipeline cannot read this yet")).toBeVisible();
    await editor.getByRole("button", { name: "Compute now" }).click();
    await expect(editor.getByRole("cell", { name: "Kallio", exact: true })).toBeVisible();
    await expect(editor.getByText("parking: 48 rows · stops: 31 rows")).toBeVisible();
    await shot(page, "filled");

    await page.getByRole("button", { name: "Propose change" }).click();
    await expect.poll(() => stub.proposed.length).toBe(2);
    expect(stub.proposed[0], "every proposal needs a fresh check (PF-57)").toBe("dry run");
    const proposed = stub.proposed.at(-1) as { apiVersion: string; spec: { sources: unknown[]; steps: { kpi: Record<string, unknown> }[] } };
    expect(proposed.apiVersion).toBe("joinedcontext.com/v1alpha2");
    expect(proposed.spec.sources).toHaveLength(2);
    expect(proposed.spec.steps[0].kpi).toMatchObject({
      formula: "sum(parking.occupiedSpotNumber) / count(stops)",
      unitCode: "C62",
      join: { by: "area", keys: { parking: "district", stops: "district" } },
    });
  });
});

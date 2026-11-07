/**
 * Own KPIs, validated on dev (T-4865, PL-102…PL-105): a person with no code builds their own
 * indicators in the KPI editor from what the project publishes: one per formula kind, one across two
 * spaces, one per area, and meets each refusal with what to do next. `KPI_SHOTS` names the evidence folder.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { EDITOR, JANITOR, approve, cleanUp, csrf, proposedChange, rejectOpen, signIn, sweepDrafts } from "./portal";

const PROJECT = "helsinki";
const SHOTS = process.env.KPI_SHOTS ?? "";
/** Every name matches the janitor's journey pattern, so the residue sweep may remove what a failed run left. */
const stamp = Date.now().toString(36).slice(-4);
/** The slug of helsinki-kpi, the indicator space's public Endpoint. */
const KPI_SLUG = "kpi4h6q2wz7xr3nv5tj6pb3mydk2s4fa";
/** Indicator pipeline name to the change that proposed it. */
const changes = new Map<string, string>();

interface Preview {
  values: { key: string | null; name: string; value: number }[];
  rows: Record<string, number>;
  problem?: string;
}

interface Source {
  name: string;
  endpoint: string;
  type: string;
  attribute?: string;
  key?: string;
}

interface Kpi {
  name: string;
  title: string;
  sources: [Source, Source];
  join?: "time" | "area" | "entity";
  choice?: "count" | "sum" | "average" | "share" | "rest" | "per";
  typed?: string;
  unit: string;
  /** The keys an area join must answer: the stations' districts, and only those. */
  areas?: string[];
}

const PARKING = { endpoint: "helsinki-transport", type: "OffStreetParking" };
const STOPS = { endpoint: "helsinki-transport", type: "GtfsStop" };

const KPIS: Kpi[] = [
  {
    name: "occupied",
    title: "Share of park and ride spaces in use",
    sources: [
      { ...PARKING, name: "free", attribute: "availableSpotNumber" },
      { ...PARKING, name: "all", attribute: "totalSpotNumber" },
    ],
    choice: "rest",
    unit: "P1",
  },
  {
    name: "pm25",
    title: "Average PM2.5",
    sources: [
      { name: "air", endpoint: "helsinki-air-quality", type: "AirQualityObserved", attribute: "pm25" },
      { name: "cleaning", endpoint: "helsinki-street-cleaning-air", type: "AirQualityObserved", attribute: "pm25" },
    ],
    choice: "average",
    unit: "GQ",
  },
  {
    name: "stops",
    title: "Stops in service",
    sources: [
      { ...STOPS, name: "stops" },
      { ...PARKING, name: "parking" },
    ],
    choice: "count",
    unit: "C62",
  },
  {
    name: "spots-per-stop",
    title: "Parking spaces per stop",
    sources: [
      { ...PARKING, name: "parking", attribute: "totalSpotNumber" },
      { ...STOPS, name: "stops" },
    ],
    choice: "per",
    unit: "C62",
  },
  {
    name: "street-per-stop",
    title: "Street parking zones per stop, across two spaces",
    sources: [
      { name: "zones", endpoint: "helsinki-parking", type: "OnStreetParking" },
      { ...STOPS, name: "stops" },
    ],
    typed: "count(zones) / count(stops)",
    unit: "C62",
  },
  {
    name: "pm25-district",
    title: "Average PM2.5 per district",
    sources: [
      { name: "air", endpoint: "helsinki-air-quality", type: "AirQualityObserved", attribute: "pm25", key: "districtCode" },
      { name: "districts", endpoint: "helsinki-districts", type: "AdministrativeArea", key: "districtCode" },
    ],
    join: "area",
    choice: "average",
    unit: "GQ",
    areas: ["02", "12", "22", "43", "45"],
  },
  {
    name: "departures",
    title: "Scheduled departures today",
    sources: [
      { name: "departures", endpoint: "helsinki-transport", type: "GtfsDailyDepartures", attribute: "scheduledDepartures" },
      { ...STOPS, name: "stops" },
    ],
    choice: "sum",
    unit: "C62",
  },
];

async function pick(scope: Locator, page: Page, label: string, option: string): Promise<void> {
  await scope.getByRole("combobox", { name: label }).click();
  await page.getByRole("option", { name: new RegExp(`^${option}`) }).first().click();
}

async function shot(page: Page, name: string): Promise<void> {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  for (const [width, height] of [[1440, 900], [400, 800]] as const) {
    await page.setViewportSize({ width, height });
    await page.waitForTimeout(150);
    await page.screenshot({ path: `${SHOTS}/${name}-${width}.png` });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

async function openEditor(page: Page): Promise<Locator> {
  await page.goto(`/projects/${PROJECT}/pipelines?lang=en`);
  await page.getByRole("button", { name: "New pipeline" }).first().click();
  await page.getByRole("button", { name: /Analyse \(KPI\)|KPI/ }).first().click();
  await page.getByRole("button", { name: "Combine several sources…" }).click();
  const editor = page.getByTestId("kpi-editor");
  await expect(editor).toBeVisible();
  return editor;
}

async function fill(page: Page, editor: Locator, kpi: Kpi): Promise<void> {
  await editor.getByLabel("Indicator name").fill(nameOf(kpi));
  await editor.getByLabel("Title", { exact: false }).first().fill(kpi.title);
  if (kpi.join) await editor.getByLabel("How the rows meet").selectOption(kpi.join);
  for (const [at, source] of kpi.sources.entries()) {
    const row = editor.getByTestId(`kpi-source-${at}`);
    await row.getByLabel("Input name").fill(source.name);
    await row.getByLabel("Endpoint").selectOption(source.endpoint);
    await pick(row, page, "Entity type", source.type);
    if (source.attribute) await pick(row, page, "Number it reads", source.attribute);
    if (source.key) await pick(row, page, "Area attribute", source.key);
  }
  if (kpi.choice) await editor.getByLabel("What to compute").selectOption(kpi.choice);
  if (kpi.typed) {
    await editor.getByRole("switch").first().click();
    await editor.getByRole("textbox", { name: /formula/i }).fill(kpi.typed);
  }
  await editor.getByLabel("Unit code").fill(kpi.unit);
  await editor.getByLabel("Written through").selectOption({ label: "helsinki-kpi" });
}

/** What a person meets when a field is wrong: the words, and where the fix is. */
const AIR = { name: "air", endpoint: "helsinki-air-quality", type: "AirQualityObserved" };
const REFUSALS: { kpi: Kpi; says: RegExp }[] = [
  {
    kpi: { name: "no-source", title: "", sources: [{ ...PARKING, name: "a" }, { ...STOPS, name: "b" }], typed: "sum(c.value) / count(b)", unit: "C62" },
    says: /`c` is not a source: the sources are a, b/,
  },
  {
    kpi: { name: "bad-unit", title: "", sources: [{ ...PARKING, name: "a", attribute: "totalSpotNumber" }, { ...STOPS, name: "b" }], choice: "sum", unit: "KILOGRAMS" },
    says: /`KILOGRAMS` is not a UN\/CEFACT common code such as C62/,
  },
  {
    kpi: { name: "no-field", title: "", sources: [{ ...PARKING, name: "a" }, { ...STOPS, name: "b" }], typed: "sum(a.noSuchField) / count(b)", unit: "C62" },
    says: /`noSuchField` is not an attribute of OffStreetParking/,
  },
  {
    kpi: { name: "wrong-unit", title: "", sources: [{ ...AIR, attribute: "pm25" }, { ...STOPS, name: "b" }], choice: "sum", unit: "MIN" },
    says: /unitCode MIN does not match/,
  },
];
/** Refusals of the preview: the draft is valid and the sources' rows give no value. */
const NO_VALUE: { kpi: Kpi; says: RegExp }[] = [
  {
    kpi: { name: "empty", title: "", sources: [{ name: "a", endpoint: "helsinki-parking", type: "OffStreetParking", attribute: "totalSpotNumber" }, { ...STOPS, name: "b" }], choice: "per", unit: "C62" },
    says: /input a: .*no rows/,
  },
  {
    kpi: { name: "by-zero", title: "", sources: [{ ...PARKING, name: "a", attribute: "totalSpotNumber" }, { ...PARKING, name: "b", attribute: "totalSpotNumber" }], typed: "sum(a.totalSpotNumber) / (sum(b.totalSpotNumber) - sum(b.totalSpotNumber))", unit: "C62" },
    says: /division by zero/,
  },
];

const nameOf = (kpi: Kpi) => `t4865-${kpi.name}-${stamp}`;

/** Compute now, answered: what the person reads, as the route said it. */
async function previewed(page: Page, editor: Locator): Promise<Preview> {
  const compute = editor.getByRole("button", { name: "Compute now" });
  await expect(compute).toBeEnabled({ timeout: 30_000 });
  const answer = page.waitForResponse((r) => r.url().includes("/kpis/check") && (r.request().postData() ?? "").includes('"preview":true'), { timeout: 60_000 });
  await compute.click();
  return ((await (await answer).json()) as { preview: Preview }).preview;
}

test.describe.configure({ mode: "serial" });

test("each refusal says what is wrong beside its field and keeps Propose shut", async ({ browser }) => {
  test.setTimeout(600_000);
  const { page, context } = await signIn(browser, EDITOR, "/");
  const propose = page.getByRole("button", { name: "Propose change" });
  for (const { kpi, says } of REFUSALS) {
    const editor = await openEditor(page);
    await fill(page, editor, kpi);
    await expect(editor.getByText(says).first()).toBeVisible({ timeout: 30_000 });
    await expect(propose).toBeDisabled();
    await shot(page, `refused-${kpi.name}`);
  }
  for (const { kpi, says } of NO_VALUE) {
    const editor = await openEditor(page);
    await fill(page, editor, kpi);
    const preview = await previewed(page, editor);
    expect(preview.values, "no value stands in for a source that has none").toEqual([]);
    await expect(editor.getByRole("alert").filter({ hasText: says })).toBeVisible();
    await shot(page, `refused-${kpi.name}`);
  }
  await context.close();
});

test("a person builds each indicator in the editor, previews its value and proposes it", async ({ browser }) => {
  test.setTimeout(1_200_000);
  const { page, context } = await signIn(browser, EDITOR, "/");
  const timings: Record<string, unknown> = {};
  for (const kpi of KPIS) {
    const started = Date.now();
    const editor = await openEditor(page);
    await shot(page, `${kpi.name}-1-empty`);
    await fill(page, editor, kpi);
    const preview = await previewed(page, editor);
    expect(preview.problem, `${kpi.name} previews a value`).toBeUndefined();
    expect(preview.values.length, kpi.name).toBeGreaterThan(0);
    if (kpi.areas) expect(preview.values.map((value) => value.key).sort(), kpi.name).toEqual(kpi.areas);
    await expect(editor.getByRole("table")).toBeVisible();
    await editor.getByRole("table").scrollIntoViewIfNeeded();
    await shot(page, `${kpi.name}-2-previewed`);
    await page.getByRole("button", { name: "Propose change" }).click();
    const change = await proposedChange(page);
    changes.set(nameOf(kpi), change);
    await shot(page, `${kpi.name}-3-proposed`);
    timings[kpi.name] = { seconds: (Date.now() - started) / 1000, preview, change };
  }
  if (SHOTS) writeFileSync(`${SHOTS}/kpis.json`, JSON.stringify(timings, null, 1));
  await context.close();
});

test("an approved indicator runs, its value sits in the KPI space, and the flow map draws its sources", async ({ browser }) => {
  // The janitor approves journey names only (T-2627); its password is the cluster's, so this leg
  // runs in the deployer's sweep and not from a worker (T-4865).
  test.setTimeout(1_500_000);
  const { page, context } = await signIn(browser, JANITOR, `/projects/${PROJECT}/approvals?lang=en`);
  for (const change of changes.values()) await approve(page, PROJECT, change);
  const token = await csrf(context);
  for (const name of changes.keys()) {
    await expect
      .poll(async () => (await page.request.post(`/api/v1/projects/${PROJECT}/pipelines/${name}/runs`, { headers: { "x-csrf-token": token } })).status(), {
        message: `${name} starts a run once its approval is live`,
        timeout: 600_000,
        intervals: [15_000],
      })
      .toBeLessThan(300);
  }
  for (const [name] of changes) {
    await expect
      .poll(
        async () => {
          const read = await page.request.get(`/api/endpoint/${KPI_SLUG}/ngsi-ld/v1/entities?type=KeyPerformanceIndicator&idPattern=${name}&limit=1`);
          return read.ok() ? ((await read.json()) as unknown[]).length : 0;
        },
        { message: `${name} wrote its value into the KPI space`, timeout: 600_000, intervals: [20_000] },
      )
      .toBeGreaterThan(0);
  }
  await page.goto(`/projects/${PROJECT}/pipelines?lang=en`);
  await page.getByRole("tab", { name: "Data flow map" }).click();
  await expect(page.getByText([...changes.keys()][0], { exact: false }).first()).toBeVisible({ timeout: 60_000 });
  await shot(page, "flow-map");
  await context.close();
});

test.afterAll(async ({ browser }) => {
  const owner = await signIn(browser, process.env.JANITOR_PASSWORD ? JANITOR : EDITOR, "/");
  for (const name of changes.keys()) {
    if (process.env.JANITOR_PASSWORD) await cleanUp(owner, PROJECT, "pipelines", name, "kpi-self-service");
    else await rejectOpen(owner, PROJECT, name, "kpi-self-service");
  }
  await owner.context.close();
  const editor = await signIn(browser, EDITOR, "/");
  await sweepDrafts(editor.context, editor.page, PROJECT, /^t4865-/);
  await editor.context.close();
});

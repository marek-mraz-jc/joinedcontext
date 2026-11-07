/**
 * Any input the runner ships, through the real UI on dev (T-0477, PL-50): for a file (`csv`),
 * a database (`sql_select`) and a broker (`nats`) a steward declares the data source from the
 * runner's own form, proposes it, an approver approves; the steward builds the pipeline in the
 * workbench, which tries the mapping on a dropped sample and checks it, proposes it; the approver approves; the
 * entities arrive through the endpoint and the pipeline's counter moves.
 *
 * What the runner reads sits on dev beforehand: the two files in the runner's files volume
 * (`kubectl cp` into `/data`, Architecture/08 §6 rule 5) and the demo feed publishing on
 * `helsinki.demo.counters` (deployment component `demo-feeds`).
 */
import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { APPROVER, STEWARD, approve, proposedChange, signIn } from "./portal";
import { proposeFrom } from "./kindJourney";

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");

interface Case {
  kind: "csv" | "sql_select" | "nats";
  source: string;
  pipeline: string;
  /** Fills the runner's own form for this input. */
  connect: (dialog: Locator) => Promise<void>;
  endpoint: string;
  type: string;
  sample: { name: string; mimeType: string; text: string };
  mapping: string;
  /** Local ids the pipeline mints, one of which must be readable through the endpoint. */
  ids: string[];
}

// Four Finnish Meteorological Institute stations at 07:00 UTC on 2026-09-14, from
// opendata.fmi.fi/timeseries (producer opendata); the same file sits in the runner's /data.
const WEATHER_CSV = [
  '"fmisid","stationname","latitude","longitude","utctime","temperature","humidity","windspeedms","winddirection","precipitation1h"',
  '100971,"Helsinki Kaisaniemi",60.1752281,24.9445896,"20260914T070000",12.5,75.0,2.0,334.0,0.0',
  '101004,"Helsinki Kumpula",60.2030716,24.9613094,"20260914T070000",12.1,79.0,2.1,330.0,0.0',
  '100996,"Helsinki Harmaja",60.1051216,24.9753895,"20260914T070000",12.6,77.0,3.9,352.0,nan',
  '100968,"Vantaa Helsinki-Vantaa airport",60.3293686,24.9727402,"20260914T070000",10.8,83.0,2.2,328.0,0.0',
].join("\n");

const WEATHER_MAPPING = [
  'let domain = env("JC_ORG_DOMAIN")',
  'let at = this.utctime.ts_strptime("%Y%m%dT%H%M%S").ts_format("2006-01-02T15:04:05Z", "UTC")',
  "root = {",
  '  "id": "urn:ngsi-ld:WeatherObserved:%v:helsinki:fmi-%v".format($domain, this.fmisid),',
  '  "type": "WeatherObserved",',
  '  "name": { "type": "LanguageProperty", "languageMap": { "fi": this.stationname } },',
  '  "location": { "type": "GeoProperty", "value": { "type": "Point", "coordinates": [this.longitude.number(), this.latitude.number()] } },',
  '  "dateObserved": { "type": "Property", "value": $at },',
  '  "temperature": { "type": "Property", "value": this.temperature.number(), "unitCode": "CEL", "observedAt": $at },',
  '  "relativeHumidity": { "type": "Property", "value": this.humidity.number() / 100, "observedAt": $at },',
  '  "windSpeed": { "type": "Property", "value": this.windspeedms.number(), "unitCode": "MTS", "observedAt": $at },',
  '  "source": { "type": "Property", "value": "https://opendata.fmi.fi" }',
  "}",
].join("\n");

// One row of the `indicators` table of helsinki-bike-network.db, computed from HSL's
// station_information.json (456 stations, 8372 docking places).
const INDICATOR_ROW = JSON.stringify([
  {
    id: "bike-network-stations",
    name: "Docking stations in the Helsinki city bike network",
    formula: "count(station_id) over station_information",
    value: 456.0,
    unit: "C62",
    observed_at: "2026-09-14T07:08:47Z",
    source: "https://gbfs.theta.fifteen.eu/gbfs/2.2/helsinki/en/station_information.json",
  },
]);

// A KeyPerformanceIndicator carries PF-54's attributes and no other (Development/10 section 4
// closes the object): the upstream URL and the synthetic note live in calculationFormula.
const INDICATOR_MAPPING = [
  'let domain = env("JC_ORG_DOMAIN")',
  "root = {",
  '  "id": "urn:ngsi-ld:KeyPerformanceIndicator:%v:helsinki-kpi:%v".format($domain, this.id),',
  '  "type": "KeyPerformanceIndicator",',
  '  "name": { "type": "Property", "value": this.name },',
  '  "calculationFormula": { "type": "Property", "value": "%v, from %v".format(this.formula, this.source) },',
  '  "currentValue": { "type": "Property", "value": this.value, "unitCode": this.unit, "observedAt": this.observed_at },',
  '  "updatedAt": { "type": "Property", "value": this.observed_at }',
  "}",
].join("\n");

// What the demo feed publishes: a synthetic counter, labelled as such in every message.
const COUNTER_MESSAGE = JSON.stringify([
  { id: "demo-counter-1", count: 5, observedAt: "2026-09-14T07:16:35.123456789Z", note: "synthetic demo reading, not a city measurement" },
]);

const COUNTER_MAPPING = [
  'let domain = env("JC_ORG_DOMAIN")',
  'let at = this.observedAt.ts_format("2006-01-02T15:04:05Z", "UTC")',
  "root = {",
  '  "id": "urn:ngsi-ld:KeyPerformanceIndicator:%v:helsinki-kpi:%v".format($domain, this.id),',
  '  "type": "KeyPerformanceIndicator",',
  '  "name": { "type": "Property", "value": "Demo counter %v".format(this.id.split("-").index(-1)) },',
  '  "calculationFormula": { "type": "Property", "value": "messages published by the demo feed: %v".format(this.note) },',
  '  "currentValue": { "type": "Property", "value": this.count, "observedAt": $at },',
  '  "updatedAt": { "type": "Property", "value": $at }',
  "}",
].join("\n");

async function addItem(dialog: Locator, field: string, value: string): Promise<void> {
  await dialog.locator(`#root_${field}__add`).click();
  await dialog.locator(`#root_${field}_0`).fill(value);
}

const CASES: Case[] = [
  {
    kind: "csv",
    source: `fmi-weather-file-${SUFFIX}`,
    pipeline: `fmi-weather-${SUFFIX}`,
    connect: async (dialog) => {
      await addItem(dialog, "paths", "/data/fmi-helsinki-weather.csv");
    },
    endpoint: "helsinki-all",
    type: "WeatherObserved",
    sample: { name: "fmi-helsinki-weather.csv", mimeType: "text/csv", text: WEATHER_CSV },
    mapping: WEATHER_MAPPING,
    ids: ["fmi-100971", "fmi-101004", "fmi-100996", "fmi-100968"],
  },
  {
    kind: "sql_select",
    source: `bike-network-db-${SUFFIX}`,
    pipeline: `bike-network-kpi-${SUFFIX}`,
    connect: async (dialog) => {
      await dialog.locator("#root_driver").fill("sqlite");
      await dialog.locator("#root_dsn").fill("file:/data/helsinki-bike-network.db?mode=ro");
      await dialog.locator("#root_table").fill("indicators");
      await addItem(dialog, "columns", "*");
    },
    endpoint: "helsinki-kpi",
    type: "KeyPerformanceIndicator",
    sample: { name: "indicators.json", mimeType: "application/json", text: INDICATOR_ROW },
    mapping: INDICATOR_MAPPING,
    ids: ["bike-network-stations", "bike-network-capacity", "bike-network-virtual"],
  },
  {
    kind: "nats",
    source: `demo-counters-nats-${SUFFIX}`,
    pipeline: `demo-counters-${SUFFIX}`,
    connect: async (dialog) => {
      await addItem(dialog, "urls", "nats://demo-nats.dev.svc.cluster.local:4222");
      await dialog.locator("#root_subject").fill("helsinki.demo.counters");
    },
    endpoint: "helsinki-kpi",
    type: "KeyPerformanceIndicator",
    sample: { name: "demo-counter.json", mimeType: "application/json", text: COUNTER_MESSAGE },
    mapping: COUNTER_MAPPING,
    ids: ["demo-counter-1", "demo-counter-2", "demo-counter-3", "demo-counter-4", "demo-counter-5"],
  },
];

async function waitForListed(page: Page, plural: string, name: string): Promise<void> {
  await expect
    .poll(
      async () => {
        const answer = await page.request.get(`/api/v1/projects/${PROJECT}/${plural}`);
        const body = (await answer.json()) as { items?: { metadata: { name: string } }[] };
        return (body.items ?? []).some((item) => item.metadata.name === name);
      },
      { timeout: 240_000, intervals: [3_000] },
    )
    .toBe(true);
}

for (const c of CASES) {
  test(`${c.kind}: declared from the runner's form, tested, approved; entities arrive and the counter moves`, async ({ browser }, info) => {
    const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/datasources?lang=en`);
    const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
    const page = steward.page;

    // 1. The data source, from the input's own form (the type first: a type change clears the draft).
    await page.getByLabel("Type").selectOption(c.kind);
    await page.getByRole("button", { name: "New data source" }).click();
    const dialog = page.getByTestId("form-page");
    await dialog.locator("#root_name").fill(c.source);
    await c.connect(dialog);
    await dialog.getByRole("button", { name: "Check" }).click();
    await expect(dialog.getByText("Planned change")).toBeVisible({ timeout: 30_000 });
    await dialog.getByRole("button", { name: "Propose change" }).click();
    await approve(approver.page, PROJECT, await proposedChange(page));
    await waitForListed(page, "datasources", c.source);

    // 2. The pipeline: the sample drafts a mapping, the case's own mapping replaces it, the test runs it.
    await page.goto(`/projects/${PROJECT}/pipelines?lang=en`, { waitUntil: "load" });
    await page.getByRole("button", { name: "New pipeline" }).click();
    const studio = page.getByTestId("form-page");
    // The workbench (T-2709): the sample file is its sample, the case's mapping its mapping.
    await studio.locator("#workbench-source-pick").selectOption(`datasource:${c.source}`);
    await studio.locator("#root_name").fill(c.pipeline);
    await studio.locator("#workbench-target-pick").selectOption({ label: c.endpoint });
    await studio.getByLabel("Choose a sample file").setInputFiles({
      name: c.sample.name,
      mimeType: c.sample.mimeType,
      buffer: Buffer.from(c.sample.text),
    });
    await studio.getByText("More options").click();
    await studio.locator("#root_compute_kind").selectOption("bloblang");
    await studio.locator("#workbench-bloblang").fill(c.mapping);
    await studio.locator("#root_output_type").fill(c.type);
    await studio.locator("#root_output_mode").selectOption("upsert");
    const propose = studio.getByRole("button", { name: "Propose change" });
    await expect(propose).toBeDisabled();
    const ready = studio.getByText(/(All \d+ records are|1 record is) valid against|names no data model/);
    await expect(ready).toBeVisible({ timeout: 60_000 });
    info.annotations.push({ type: "test", description: (await ready.textContent()) ?? "" });
    // Propose opens on the dialog's own fresh check (PF-57), after the mapping's test (PL-49).
    await proposeFrom(studio);
    await approve(approver.page, PROJECT, await proposedChange(page));
    await waitForListed(page, "pipelines", c.pipeline);

    // 3. Data arrives: the counter moves and one minted entity reads back through the endpoint.
    await expect
      .poll(
        async () => {
          const answer = await page.request.get(`/api/v1/projects/${PROJECT}/pipelines/${c.pipeline}/metrics`);
          const body = answer.ok() ? ((await answer.json()) as { received?: number | null }) : {};
          return body.received ?? 0;
        },
        { timeout: 240_000, intervals: [10_000] },
      )
      .toBeGreaterThan(0);
    const endpoints = (await (await page.request.get(`/api/v1/projects/${PROJECT}/endpoints`)).json()) as {
      items?: { metadata: { name: string }; spec?: { slug?: string } }[];
    };
    const slug = endpoints.items?.find((e) => e.metadata.name === c.endpoint)?.spec?.slug;
    expect(slug, `endpoint ${c.endpoint} has a slug`).toBeTruthy();
    const space = c.endpoint === "helsinki-kpi" ? "helsinki-kpi" : PROJECT;
    const found = await Promise.all(
      c.ids.map(async (local) => {
        const id = `urn:ngsi-ld:${c.type}:hel.fi:${space}:${local}`;
        const answer = await page.request.get(`/api/endpoint/${slug}/ngsi-ld/v1/entities/${encodeURIComponent(id)}`);
        return answer.ok() ? id : null;
      }),
    );
    const arrived = found.filter(Boolean);
    info.annotations.push({ type: "entities", description: arrived.join(", ") });
    expect(arrived.length).toBeGreaterThan(0);

    await steward.context.close();
    await approver.context.close();
  });
}

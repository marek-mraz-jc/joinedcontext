/**
 * The pipeline studio on dev (T-3967, PL-52, PL-56, PL-68): the three walks of
 * `e2e/pipeline_studio_walk.spec.ts` built live by the steward from the palette, each tested on a
 * sample, checked, proposed, approved by the approver, seen Live and seen writing; then the edge
 * cases against the real API, and the viewer.
 *
 * - (a) MQTT: HSL high-frequency positioning (`hsl-hfp`) → a processor → a mapping → Vehicle.
 * - (b) HTTP poll: the city bike station status (`hsl-citybikes-gbfs`), every five minutes →
 *   a processor → a mapping → BikeHireDockingStation.
 * - (c) CSV: a journey DataSource reading the bike stations as `file.csv` of `helsinki-all`, the
 *   platform's own public CSV → a mapping per row → BikeHireDockingStation.
 *
 * Every mapping writes the ids and the very attribute objects the seeded pipelines write from the
 * same feed (`speed` of `helsinki-pipeline-hfp`, `availableBikeNumber` of the bikes pipeline, the
 * constant `source`), and every output merges (`upsert`, PL-79): a journey leaves no entity and no
 * value behind. Its pipelines and DataSource carry the run's HHMM suffix, are removed at the end,
 * and `residue.spec.ts` sweeps them if a run stops half way.
 *
 * `STUDIO_SHOTS=<dir>` saves the steps for `docs/Research/pipeline-studio-vs-nifi.md`; the
 * annotations carry each walk's build time (first click to proposal) and its time to Live.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Locator, Page } from "@playwright/test";
import { APPROVER, STEWARD, VIEWER, approve, checkManifest, csrf, proposedChange, removeCompletely, signIn, sweepDrafts, waitGone } from "./portal";

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const TARGET = "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all";
const ALL = "helsinki-all";
const CSV_SOURCE = `studio-csv-${SUFFIX}`;

interface Walk {
  name: string;
  source: string;
  processor: string;
  mapping: string;
  /** The pipeline's period, when its source is polled. */
  period?: string;
  sample: { name: string; mimeType: string; body: string };
  type: string;
}

const HFP_MAPPING = [
  'root = if this.exists("VP") && this.VP.spd.type() == "number" { {',
  '  "id": "urn:ngsi-ld:Vehicle:%v:%v:%v-%v".format(env("JC_ORG_DOMAIN"), env("JC_SPACE"), this.VP.oper, this.VP.veh),',
  '  "type": "Vehicle",',
  '  "speed": { "type": "Property", "value": this.VP.spd, "unitCode": "MTS", "observedAt": this.VP.tst }',
  "} } else { deleted() }",
].join("\n");

const BIKES_MAPPING = [
  "root = this.data.stations.filter(s -> s.is_installed).map_each(s -> {",
  '  "id": "urn:ngsi-ld:BikeHireDockingStation:%v:%v:%v".format(env("JC_ORG_DOMAIN"), env("JC_SPACE"), s.station_id),',
  '  "type": "BikeHireDockingStation",',
  '  "availableBikeNumber": { "type": "Property", "value": s.num_bikes_available.number(),',
  '    "observedAt": s.last_reported.number().ts_format("2006-01-02T15:04:05Z") }',
  "})",
].join("\n");

/** One CSV row of `file.csv`, one message: the DataSource says `format: csv` (PL-114, T-5350). */
const CSV_MAPPING = [
  'root = if this.id.or("").has_prefix("urn:ngsi-ld:BikeHireDockingStation:") { {',
  '  "id": this.id,',
  '  "type": "BikeHireDockingStation",',
  '  "source": { "type": "Property", "value": "https://www.hsl.fi/en/citybikes" }',
  "} } else { deleted() }",
].join("\n");

const WALKS: Walk[] = [
  {
    name: `studio-hfp-${SUFFIX}`,
    source: "hsl-hfp",
    processor: "log",
    mapping: HFP_MAPPING,
    sample: {
      name: "hfp.json",
      mimeType: "application/json",
      body: JSON.stringify({ VP: { oper: 22, veh: 1234, spd: 8.4, lat: 60.17, long: 24.94, tst: "2026-10-18T08:00:00.000Z" } }),
    },
    type: "Vehicle",
  },
  {
    name: `studio-poll-${SUFFIX}`,
    source: "hsl-citybikes-gbfs",
    processor: "switch",
    mapping: BIKES_MAPPING,
    period: "5m",
    sample: {
      name: "station_status.json",
      mimeType: "application/json",
      body: JSON.stringify({
        data: {
          stations: [
            { station_id: "001", is_installed: true, num_bikes_available: 4, last_reported: 1792310400 },
            { station_id: "002", is_installed: false, num_bikes_available: 0, last_reported: 1792310400 },
          ],
        },
      }),
    },
    type: "BikeHireDockingStation",
  },
  {
    name: `studio-csv-${SUFFIX}`,
    source: CSV_SOURCE,
    processor: "log",
    mapping: CSV_MAPPING,
    period: "24h",
    sample: {
      name: "stations.csv",
      mimeType: "text/csv",
      body: "id,type\nurn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:001,BikeHireDockingStation\n",
    },
    type: "BikeHireDockingStation",
  },
];

/** One step's screenshot, when the comparison page is being made. */
async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.STUDIO_SHOTS;
  if (dir) await page.screenshot({ path: `${dir}/${name}.png` });
}

/** The slug of `helsinki-all`, whose CSV the walk (c) reads and whose entities every walk writes. */
async function slugOf(page: Page, endpoint: string): Promise<string> {
  const answer = await page.request.get(`/api/v1/projects/${PROJECT}/endpoints`);
  expect(answer.ok(), "the project's endpoints").toBe(true);
  const items = ((await answer.json()) as { items?: { metadata: { name: string }; spec?: { slug?: string } }[] }).items ?? [];
  const slug = items.find((one) => one.metadata.name === endpoint)?.spec?.slug;
  expect(slug, `${endpoint} has a slug`).toBeTruthy();
  return slug ?? "";
}

/** Proposes a manifest through the API, checked first, and has the approver approve it. */
async function proposeAndApprove(
  owner: { context: BrowserContext; page: Page },
  approver: Page,
  plural: string,
  manifest: { kind: string } & Record<string, unknown>,
): Promise<void> {
  await checkManifest(owner.page, owner.context, PROJECT, manifest);
  const answer = await owner.page.request.post(`/api/v1/projects/${PROJECT}/${plural}`, {
    headers: { "x-csrf-token": await csrf(owner.context) },
    data: manifest,
  });
  expect(answer.status(), await answer.text()).toBe(202);
  const change = ((await answer.json()) as { metadata?: { name?: string } }).metadata?.name ?? "";
  expect(change, "the proposal names its change").not.toBe("");
  await approve(approver, PROJECT, change);
}

/** The studio of a new pipeline: the form, then its canvas. */
async function openStudio(page: Page, name: string): Promise<{ form: Locator; canvas: Locator }> {
  await page.goto(`/projects/${PROJECT}/pipelines?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: "New pipeline" }).first().click();
  const form = page.getByTestId("form-page");
  await form.getByLabel(/^Name/).fill(name);
  await form.getByText("More options").click();
  await form.getByText("Advanced editor").click();
  const canvas = page.getByTestId("flow-canvas");
  await canvas.scrollIntoViewIfNeeded();
  return { form, canvas };
}

/** Polls a pipeline until the runner calls it Live (PL-47: the runner's word, never Git's). */
async function live(page: Page, name: string): Promise<void> {
  await expect
    .poll(
      async () => {
        const answer = await page.request.get(`/api/v1/projects/${PROJECT}/pipelines/${name}`);
        return answer.ok() ? (((await answer.json()) as { status?: { phase?: string } }).status?.phase ?? "") : `${answer.status()}`;
      },
      { timeout: 900_000, intervals: [15_000], message: `${name} goes Live` },
    )
    .toBe("Live");
}

/** Polls what a Live pipeline has sent: a run's count for a poll, the stream's minute for MQTT. */
async function wrote(page: Page, name: string): Promise<number> {
  let sent = 0;
  await expect
    .poll(
      async () => {
        const runs = await page.request.get(`/api/v1/projects/${PROJECT}/pipelines/${name}/runs`);
        const items = runs.ok() ? (((await runs.json()) as { items?: { sent: number }[] }).items ?? []) : [];
        sent = Math.max(0, ...items.map((run) => run.sent));
        if (sent > 0) return sent;
        const stats = await page.request.get(`/api/v1/projects/${PROJECT}/pipelines/${name}/stats`);
        const text = stats.ok() ? await stats.text() : "";
        // The output node's sent count of the last minute (T-3921), when the runs say nothing yet.
        sent = Math.max(0, ...[...text.matchAll(/"sent"\s*:\s*(\d+)/g)].map((m) => Number(m[1])));
        return sent;
      },
      { timeout: 600_000, intervals: [15_000], message: `${name} writes` },
    )
    .toBeGreaterThan(0);
  return sent;
}

test.setTimeout(3_600_000);

test.describe.serial("pipeline studio on dev (T-3967)", () => {
  test("three pipelines built from the palette go Live and write", async ({ browser, baseURL }, info) => {
    const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/pipelines?lang=en`);
    const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
    const page = steward.page;
    let walked = false;
    let cleanup: unknown;
    try {
      // The CSV the walk (c) reads: the bike stations of helsinki-all, as anybody downloads them.
      const slug = await slugOf(page, ALL);
      await proposeAndApprove(steward, approver.page, "datasources", {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "DataSource",
        metadata: { name: CSV_SOURCE, namespace: PROJECT },
        spec: {
          type: "http",
          http: {
            url: `${baseURL}/api/endpoint/${slug}/file.csv?type=BikeHireDockingStation&attrs=name`,
            verb: "GET",
            headers: { Accept: "text/csv" },
            timeout: "30s",
            // Without it the runner hands the whole file to the mapping as one message (T-5350).
            format: "csv",
          },
        },
      });

      for (const walk of WALKS) {
        const started = Date.now();
        const { form, canvas } = await openStudio(page, walk.name);
        await shot(page, `${walk.name}-1-empty-canvas`);

        await page.getByTestId("flow-node-source").dblclick();
        await page.getByTestId("flow-node-editor-source").getByTestId("flow-source-ref").selectOption(`DataSource:${walk.source}`);
        await shot(page, `${walk.name}-2-source`);

        await page.getByLabel("Find a processor").fill(walk.processor);
        const found = page.getByTestId(`palette-found-${walk.processor}`);
        await found.scrollIntoViewIfNeeded();
        await found.dragTo(canvas);
        await expect(page.getByTestId("flow-node-step-0")).toBeVisible();
        await page.getByTestId("palette-bloblang").dragTo(canvas);
        await expect(page.getByTestId("flow-node-compute")).toBeVisible();
        await page.getByTestId("flow-bloblang").fill(walk.mapping);
        await form
          .getByTestId("pipeline-studio")
          .locator('input[type="file"]')
          .setInputFiles({ name: walk.sample.name, mimeType: walk.sample.mimeType, buffer: Buffer.from(walk.sample.body) });
        await expect(page.getByTestId("flow-bloblang")).toHaveValue(walk.mapping);
        await shot(page, `${walk.name}-3-steps`);

        await page.getByTestId("flow-node-output").dblclick();
        await page.getByTestId("flow-node-editor-output").getByTestId("flow-output-target").selectOption(TARGET);
        await shot(page, `${walk.name}-4-output`);
        if (walk.period) {
          await form.locator("#root_period").fill(walk.period);
        }

        // PipelineTest on the sample: one test at a time per project, so a 409 is waited out.
        await expect(async () => {
          await page.getByRole("button", { name: "Test mapping" }).click();
          await expect(page.getByTestId("flow-node-compute")).toHaveAttribute("data-state", "ok", { timeout: 60_000 });
        }).toPass({ timeout: 300_000, intervals: [15_000] });
        await shot(page, `${walk.name}-5-tested`);

        await form.getByRole("button", { name: "Check", exact: true }).click();
        await expect(form.getByTestId("draft-verdict")).toContainText("Checked", { timeout: 120_000 });
        await form.getByRole("button", { name: "Propose change" }).click();
        const change = await proposedChange(page);
        const built = Date.now() - started;
        await shot(page, `${walk.name}-6-proposed`);

        await approve(approver.page, PROJECT, change);
        const approved = Date.now();
        await live(page, walk.name);
        const sent = await wrote(page, walk.name);
        info.annotations.push({
          type: walk.name,
          description: JSON.stringify({ buildMs: built, liveMs: Date.now() - approved, sent, type: walk.type }),
        });
      }
      walked = true;
    } finally {
      // The DataSource goes only once the Pipelines reading it are gone (409 before), and a
      // cleanup failure never replaces the walk's own failure in the report (T-5350).
      try {
        for (const walk of WALKS) {
          await removeCompletely(steward, PROJECT, "pipelines", walk.name);
        }
        for (const walk of WALKS) {
          await waitGone(page, PROJECT, "pipelines", walk.name);
        }
        await removeCompletely(steward, PROJECT, "datasources", CSV_SOURCE);
        await sweepDrafts(steward.context, page, PROJECT, new RegExp(`^studio-(hfp|poll|csv)-${SUFFIX}$`));
      } catch (error) {
        cleanup = error;
        if (!walked) info.annotations.push({ type: "cleanup failed", description: String(error) });
      } finally {
        await steward.context.close();
        await approver.context.close();
      }
    }
    // Reached only when the walk passed: then a failed cleanup fails the test on its own.
    if (cleanup !== undefined) throw cleanup;
  });

  test("an empty target, a wire into a source and a literal secret are refused by name", async ({ browser }) => {
    const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/pipelines?lang=en`);
    const page = steward.page;
    const name = `studio-edge-${SUFFIX}`;
    try {
      // A required field left empty: the check stops at the field and nothing is proposed.
      await page.goto(`/projects/${PROJECT}/pipelines?lang=en`, { waitUntil: "load" });
      await page.getByRole("button", { name: "New pipeline" }).first().click();
      const form = page.getByTestId("form-page");
      await form.getByLabel(/^Name/).fill(name);
      await form.getByRole("button", { name: "Check", exact: true }).click();
      await expect(form.getByTestId("check-stopped")).toHaveText("Target endpoint is required");
      await expect(form.getByRole("button", { name: "Propose change" })).toBeDisabled();

      // A wire into a source: the canvas says why it takes none (T-4271).
      const { canvas } = await openStudio(page, name);
      await expect(canvas).toBeVisible();
      await page.getByTestId("palette-bloblang").click();
      await page.getByTestId("flow-port-out-compute").dispatchEvent("pointerdown");
      await page.getByTestId("flow-node-source").dispatchEvent("pointerup");
      await expect(page.getByTestId("flow-said")).toHaveText("A source reads from outside the pipeline; nothing wires into it.");

      // A secret typed into a plain field: refused with a pointer to secretRef, never echoed.
      const value = `studio-${SUFFIX}-not-a-real-password`;
      const answer = await page.request.put(`/api/v1/projects/${PROJECT}/drafts/DataSource/${name}`, {
        headers: { "x-csrf-token": await csrf(steward.context) },
        data: {
          manifest: {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "DataSource",
            metadata: { name, namespace: PROJECT },
            spec: { type: "mqtt", mqtt: { urls: ["wss://mqtt.hsl.fi:443/"], topics: ["/hfp/v2/#"], password: value } },
          },
        },
      });
      expect(answer.status(), "a literal secret is refused").toBeGreaterThanOrEqual(400);
      expect(answer.status()).toBeLessThan(500);
      const said = await answer.text();
      expect(said).toMatch(/secretRef/);
      expect(said, "the refusal does not echo the value").not.toContain(value);
    } finally {
      await sweepDrafts(steward.context, page, PROJECT, new RegExp(`^${name}$`));
      await steward.context.close();
    }
  });

  test("a viewer has no working edit control and cannot propose a pipeline", async ({ browser }) => {
    const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/pipelines?lang=en`);
    const page = viewer.page;
    try {
      const add = page.getByRole("button", { name: "New pipeline" });
      await expect(add.first()).toBeVisible({ timeout: 60_000 });
      for (const button of await add.all()) await expect(button).toBeDisabled();
      const answer = await page.request.post(`/api/v1/projects/${PROJECT}/pipelines`, {
        headers: { "x-csrf-token": await csrf(viewer.context) },
        data: {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "Pipeline",
          metadata: { name: `studio-viewer-${SUFFIX}`, namespace: PROJECT },
          spec: { sources: [{ dataSourceRef: { kind: "DataSource", name: "hsl-hfp" } }], steps: [], outputs: [] },
        },
      });
      expect(answer.status(), await answer.text()).toBe(403);
    } finally {
      await viewer.context.close();
    }
  });
});

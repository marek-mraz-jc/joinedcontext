/**
 * The pipeline workbench on dev (T-2712, ADR-N-034, PL-58…PL-63), as the steward: pick the city
 * bike stations feed and see its sample; write a mapping with one field of the wrong type and see
 * the error at each record and field; fix it and see every record valid; see the space the records
 * land in, then propose; the approver approves. After one run the space holds only valid records,
 * the rejected list holds the one record the mapping breaks on purpose, and the pipeline's page
 * shows the run with its counts and its log.
 *
 * The record broken on purpose is the 31st station: the workbench tries a mapping on the first
 * 20 records the source answers (the harness's cap), so the steps show every record valid while
 * the run, which reads them all, rejects exactly that one. The mapping writes the ids the seeded
 * `citybikes-gbfs-info` pipeline writes, with the same values, so the journey leaves no entity
 * behind; the pipeline itself is removed at the end, and its name carries the run's HHMM suffix
 * so `residue.spec.ts` sweeps it if the journey stops half way.
 */
import { expect, test } from "@playwright/test";
import type { Locator } from "@playwright/test";
import { guideShot } from "./guide";
import { APPROVER, STEWARD, approve, proposedChange, removeCompletely, signIn, sweepDrafts } from "./portal";
import { proposeFrom } from "./kindJourney";

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const PIPELINE = `citybikes-wb-${SUFFIX}`;
const SOURCE = "hsl-citybikes-gbfs-info";
const TARGET = "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all";
/** The station the fixed mapping breaks on purpose: past the 20 records the workbench tries. */
const BROKEN = 30;

/** The seeded station mapping, 31 stations of the page, with `totalSlotNumber` from `capacity`. */
function mapping(slots: string): string {
  return [
    'let domain = env("JC_ORG_DOMAIN")',
    `root = this.data.stations.slice(0, ${BROKEN + 1}).enumerated().map_each(e -> {`,
    '  "id": "urn:ngsi-ld:BikeHireDockingStation:%v:helsinki:%v".format($domain, e.value.station_id),',
    '  "type": "BikeHireDockingStation",',
    '  "name": { "type": "LanguageProperty", "languageMap": { "fi": e.value.name.string() } },',
    '  "location": { "type": "GeoProperty", "value": { "type": "Point", "coordinates": [ e.value.lon.number(), e.value.lat.number() ] } },',
    `  "totalSlotNumber": { "type": "Property", "value": ${slots} },`,
    '  "source": { "type": "Property", "value": "https://www.hsl.fi/en/citybikes" }',
    "})",
  ].join("\n");
}
/** Every station's capacity as text: the model wants an integer. */
const WRONG = mapping("e.value.capacity.string()");
/** The capacity as a number, and one station broken on purpose for the run to reject. */
const FIXED = mapping(`if e.index == ${BROKEN} { "many" } else { e.value.capacity.number() }`);

interface Run {
  run: string;
  sent: number;
  rejected: number;
  failed: number;
}

test.setTimeout(900_000);

test("a pipeline built in the workbench writes only what the model takes", async ({ browser }, info) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/pipelines?lang=en`);
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  const page = steward.page;
  const step = (form: Locator, title: string) => form.getByRole("region", { name: new RegExp(`${title}$`) });

  try {
    await page.getByRole("button", { name: "New pipeline" }).first().click();
    const form = page.getByTestId("form-page");

    // 1. The source, and its sample: the feed's page as the runner reads it.
    await form.locator("#workbench-source-pick").selectOption(`datasource:${SOURCE}`);
    await expect(step(form, "Sample").getByRole("table", { name: "Sample records" })).toBeVisible({ timeout: 60_000 });
    await guideShot(page, "pipeline-1-source-and-sample");

    await form.locator("#root_name").fill(PIPELINE);
    // The target is step 3, before the mapping (T-3296): it names the space and its model, and
    // the mapping step below already lists what that model takes.
    await form.locator("#workbench-target-pick").selectOption(TARGET);
    await expect(
      step(form, "Target").getByText("The records land in the space helsinki, checked against the model helsinki."),
    ).toBeVisible();
    await expect(step(form, "Mapping").getByRole("region", { name: /^What .+ takes$/ }).first()).toBeVisible({ timeout: 30_000 });
    await guideShot(page, "pipeline-2-target-and-mapping");
    // A run a minute after the approval, not the seeded pipeline's daily one.
    await form.getByText("More options").click();
    await form.locator("#root_period").fill("60s");

    // 2. One field of the wrong type: the error stands at each record, named by its rule and field.
    await form.locator("#workbench-bloblang").fill(WRONG);
    const output = step(form, "Mapped output");
    const records = output.getByRole("table", { name: "Mapped records" });
    await expect(records.getByText("sh:datatype").first()).toBeVisible({ timeout: 90_000 });
    await expect(records.getByText("totalSlotNumber").first()).toBeVisible();
    await expect(step(form, "Validation").getByText(/records? breaks? helsinki/)).toBeVisible();
    const propose = form.getByRole("button", { name: /^Propose/ }).first();
    await expect(propose).toBeDisabled();
    await guideShot(page, "pipeline-3-errors-at-each-record");

    // 3. Fixed: every record the workbench tried is valid.
    await form.locator("#workbench-bloblang").fill(FIXED);
    const valid = step(form, "Validation").getByText(/All \d+ records are valid against helsinki/);
    await expect(valid).toBeVisible({ timeout: 90_000 });
    info.annotations.push({ type: "workbench", description: (await valid.textContent()) ?? "" });
    await expect(records.getByText("sh:datatype")).toHaveCount(0);
    await guideShot(page, "pipeline-4-all-valid");

    // 4. Proposed, and approved by someone else.
    await proposeFrom(form);
    await approve(approver.page, PROJECT, await proposedChange(page));

    // 5. One run: 30 written, the broken one rejected with its rule, nothing failed.
    let run: Run | undefined;
    await expect
      .poll(
        async () => {
          const answer = await page.request.get(`/api/v1/projects/${PROJECT}/pipelines/${PIPELINE}/runs`);
          const body = answer.ok() ? ((await answer.json()) as { items?: Run[] }) : {};
          run = body.items?.find((one) => one.sent + one.rejected >= BROKEN + 1);
          return run === undefined ? null : { sent: run.sent, rejected: run.rejected, failed: run.failed };
        },
        { timeout: 420_000, intervals: [10_000] },
      )
      .toEqual({ sent: BROKEN, rejected: 1, failed: 0 });
    info.annotations.push({ type: "run", description: JSON.stringify(run) });

    const rejected = await page.request.get(`/api/v1/projects/${PROJECT}/pipelines/${PIPELINE}/rejected`);
    expect(rejected.ok()).toBe(true);
    const list = (await rejected.json()) as {
      total: number;
      items: { rule: string; path: string; record: { id?: string; totalSlotNumber?: { value?: unknown } } }[];
    };
    expect(list.total).toBe(1);
    expect(list.items[0].rule).toBe("sh:datatype");
    expect(list.items[0].path).toBe("totalSlotNumber");
    const brokenId = list.items[0].record.id ?? "";
    expect(brokenId).toMatch(/^urn:ngsi-ld:BikeHireDockingStation:hel\.fi:helsinki:/);

    // The space holds only valid records: the broken station's capacity is still a number there.
    const endpoints = (await (await page.request.get(`/api/v1/projects/${PROJECT}/endpoints`)).json()) as {
      items?: { metadata: { name: string }; spec?: { slug?: string } }[];
    };
    const slug = endpoints.items?.find((one) => one.metadata.name === "helsinki-all")?.spec?.slug;
    expect(slug, "helsinki-all has a slug").toBeTruthy();
    const stored = await page.request.get(
      `/api/endpoint/${slug}/ngsi-ld/v1/entities/${encodeURIComponent(brokenId)}?options=keyValues`,
    );
    if (stored.ok()) {
      const entity = (await stored.json()) as { totalSlotNumber?: unknown };
      expect(entity.totalSlotNumber === undefined || typeof entity.totalSlotNumber === "number").toBe(true);
    } else {
      expect(stored.status(), "the broken station is absent or holds a number").toBe(404);
    }

    // The rejected record, retried after a fix that has not happened: it is replayed through the
    // current model, which still refuses it, so it comes back to the list.
    await page.goto(`/projects/${PROJECT}/pipelines?lang=en`, { waitUntil: "load" });
    await page.getByRole("button", { name: `More actions for ${PIPELINE}` }).click();
    await page.getByRole("menuitem", { name: "Rejected records" }).click();
    const rejectedDialog = page.getByRole("dialog", { name: `Rejected records of ${PIPELINE}` });
    await rejectedDialog.getByRole("checkbox", { name: "Pick every record on this page" }).check();
    await rejectedDialog.getByRole("button", { name: /^Retry \d+ records? after fix$/ }).click();
    await expect(rejectedDialog.getByText(/records? (was|were) replayed through the current model/)).toBeVisible({ timeout: 30_000 });
    await rejectedDialog.getByRole("button", { name: "Close" }).click();
    await expect
      .poll(
        async () => {
          const again = await page.request.get(`/api/v1/projects/${PROJECT}/pipelines/${PIPELINE}/rejected`);
          return again.ok() ? ((await again.json()) as { total: number }).total : 0;
        },
        { timeout: 120_000, intervals: [5_000], message: "the replayed record is refused again" },
      )
      .toBeGreaterThanOrEqual(1);

    // The pipeline's page shows the run with its counts, and its log names the rejected record.
    await page.goto(`/projects/${PROJECT}/pipelines?lang=en`, { waitUntil: "load" });
    await page.getByRole("button", { name: `More actions for ${PIPELINE}` }).click();
    await page.getByRole("menuitem", { name: "Runs and log" }).click();
    const dialog = page.getByRole("dialog");
    const runs = dialog.getByRole("table", { name: "Runs" });
    await expect(runs).toBeVisible({ timeout: 30_000 });
    const log = dialog.getByRole("region", { name: /^Log of the run / });
    await expect(log.getByText("Rejected").first()).toBeVisible({ timeout: 30_000 });
    await expect(log.getByText(brokenId)).toBeVisible();
    await dialog.getByRole("button", { name: "Close" }).click();
  } finally {
    // Janitor: the pipeline goes (a Red change the steward's admin role approves, CC-34) and so
    // do the drafts its form left.
    await removeCompletely(steward, PROJECT, "pipelines", PIPELINE);
    await sweepDrafts(steward.context, page, PROJECT, new RegExp(`^${PIPELINE}$`));
    await steward.context.close();
    await approver.context.close();
  }
});

/**
 * Load in 60 seconds, through the real UI on dev (T-0630, PL-39, PL-43, PL-44, PL-48, PL-49,
 * MF-39, UI-32, CC-34): a steward adds an HTTP data source, checks it (plan and one fetch of the
 * feed), proposes it; an approver approves; the steward builds a pipeline on it in the studio,
 * tests the mapping on the source's own URL, proposes it; the approver approves; the entities
 * appear in Explore. The clock measures the person's part: from the first click to the second
 * approval, excluding the waits for the platform to merge and reconcile.
 */
import { expect, test } from "@playwright/test";
import { journeyClock } from "./journeys";
import { APPROVER, STEWARD, approve, proposedChange, signIn } from "./portal";
import { proposeFrom } from "./kindJourney";
import { FEED, MAPPING } from "./loadFeed";

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const SOURCE = `hsl-citybikes-free-${SUFFIX}`;
const PIPELINE = `citybikes-free-${SUFFIX}`;
const TARGET = "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all";

test("a data source and a pipeline, checked, tested, proposed and approved through the UI; entities in Explore", async ({ browser }, info) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/datasources?lang=en`);
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  const page = steward.page;
  let personMs = 0;
  const tick = (start: number) => {
    personMs += Date.now() - start;
  };

  // 1. The data source: type first (a type change clears the draft), then the dialog.
  const clock = journeyClock("Load");
  let start = Date.now();
  await page.getByLabel("Type").selectOption("http");
  await page.getByRole("button", { name: "New data source" }).click();
  const dialog = page.getByTestId("form-page");
  await dialog.getByLabel(/^Name/).fill(SOURCE);
  await dialog.getByLabel(/^URL/).fill(FEED);
  await dialog.getByLabel(/^Timeout/).fill("15s");
  await dialog.getByRole("button", { name: "Check" }).click();
  await expect(dialog.getByText("Planned change")).toBeVisible({ timeout: 30_000 });
  const probe = dialog.getByTestId("datasource-probe");
  await expect(probe).toBeVisible({ timeout: 30_000 });
  const probeText = (await probe.textContent()) ?? "";
  info.annotations.push({ type: "probe", description: probeText.slice(0, 200) });
  expect(probeText).toMatch(/records|Not fetched/);
  await dialog.getByRole("button", { name: "Propose change" }).click();
  const sourceChange = await proposedChange(page);
  tick(start);

  // 2. Approved by someone else (CC-34).
  start = Date.now();
  await approve(approver.page, PROJECT, sourceChange);
  tick(start);

  // 3. The pipeline, once the mirror lists the approved source (platform time, not the person's).
  await page.goto(`/projects/${PROJECT}/pipelines?lang=en`, { waitUntil: "load" });
  await expect
    .poll(
      async () => {
        const answer = await page.request.get(`/api/v1/projects/${PROJECT}/datasources`);
        const body = (await answer.json()) as { items?: { metadata: { name: string } }[] };
        return (body.items ?? []).some((item) => item.metadata.name === SOURCE);
      },
      { timeout: 240_000, intervals: [3_000] },
    )
    .toBe(true);
  await page.reload({ waitUntil: "load" });

  start = Date.now();
  await page.getByRole("button", { name: "New pipeline" }).click();
  const studio = page.getByTestId("form-page");
  // The workbench (T-2709): source, name, target and mapping in its steps, the rest under More options.
  await studio.locator("#workbench-source-pick").selectOption(`datasource:${SOURCE}`);
  await studio.locator("#root_name").fill(PIPELINE);
  await studio.getByText("More options").click();
  await studio.locator("#root_period").fill("60s");
  // The options carry the endpoint's name; the value is the URN the org domain completes.
  await studio.locator("#workbench-target-pick").selectOption(TARGET);
  await studio.locator("#root_compute_kind").selectOption("bloblang");
  await studio.locator("#workbench-bloblang").fill(MAPPING);
  await studio.locator("#root_output_type").fill("Vehicle");
  await studio.locator("#root_output_mode").selectOption("upsert");
  // Propose is closed until the mapping that ran made records the space's model takes (PL-49,
  // PL-58): the workbench samples the source, tries the mapping and checks it by itself.
  const propose = studio.getByRole("button", { name: "Propose change" });
  await expect(propose).toBeDisabled();
  const ready = studio.getByText(/(All \d+ records are|1 record is) valid against|names no data model/);
  await expect(ready).toBeVisible({ timeout: 60_000 });
  info.annotations.push({ type: "test", description: (await ready.textContent()) ?? "" });
  // Propose opens on the dialog's own fresh check (PF-57): the test above is the mapping's gate
  // (PL-49), the check is the draft's, and the page says "Run the check first" until it ran.
  await proposeFrom(studio);
  const pipelineChange = await proposedChange(page);
  tick(start);

  // 4. Approved by someone else again.
  start = Date.now();
  await approve(approver.page, PROJECT, pipelineChange);
  tick(start);
  clock.person(personMs);
  info.annotations.push({ type: "person-seconds", description: (personMs / 1000).toFixed(1) });
  expect(personMs, "the person's part of Load stays under a minute").toBeLessThan(60_000);

  // 5. The sink is live: Vehicle entities readable through the endpoint (PL-47, T-0646).
  // Explore keeps the space and the type in page state, so every poll chooses them again.
  const chooseVehicle = async () => {
    await page.goto(`/projects/${PROJECT}/explore?lang=en`, { waitUntil: "load" });
    await page.locator("#explore-space").selectOption(PROJECT);
    const kind = page.locator("#explore-type");
    if ((await kind.evaluate((el) => el.tagName)) === "SELECT") {
      await kind.selectOption("Vehicle");
    } else {
      await kind.fill("Vehicle");
      await kind.press("Enter");
    }
  };
  await expect
    .poll(
      async () => {
        await chooseVehicle();
        const count = await page
          .getByText(/^\d+ entit(y|ies)$/)
          .first()
          .textContent({ timeout: 15_000 })
          .catch(() => "0 entities");
        return Number.parseInt(count ?? "0", 10);
      },
      { timeout: 240_000, intervals: [10_000] },
    )
    .toBeGreaterThan(0);
  clock.live();

  await steward.context.close();
  await approver.context.close();
});

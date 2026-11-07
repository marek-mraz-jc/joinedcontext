/**
 * Analyse in 60 seconds, through the real UI on dev (T-0633, PL-45, PL-46, UI-32, CC-34):
 * a steward configures an indicator pipeline with the "Analyse (KPI)" preset, tests the
 * computation on the source endpoint, and proposes it; an approver approves; the pipeline runs
 * and writes the indicator entity into the KPI space, verified in Explore. The clock measures the
 * person's part: from opening the dialog to the approval, excluding platform merge/reconcile waits.
 */
import { expect, test } from "@playwright/test";
import { journeyClock } from "./journeys";
import { APPROVER, STEWARD, approve, proposedChange, signIn } from "./portal";
import { proposeFrom } from "./kindJourney";

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const NAME = `bikes-avg-${SUFFIX}`;

test.setTimeout(420_000);

test("a KPI pipeline configured with the preset, tested on endpoint, proposed, and verified in Explore", async ({
  browser,
}, info) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/pipelines?lang=en`);
  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  const page = steward.page;
  let personMs = 0;
  const tick = (start: number) => {
    personMs += Date.now() - start;
  };

  const clock = journeyClock("Analyse");
  let start = Date.now();
  await page.getByRole("button", { name: "New pipeline" }).click();
  const studio = page.getByTestId("form-page");

  await studio.locator("#studio-preset").selectOption("kpi");
  await studio.locator("#studio-kpi-endpoint").selectOption({ value: "helsinki-all" });
  await studio.locator("#studio-kpi-name").fill(NAME);

  await studio.getByTestId("studio-kpi-test").click();
  const kpiValue = studio.getByTestId("studio-kpi-value");
  await expect(kpiValue).toHaveText(/\d/, { timeout: 60_000 });

  // Propose opens on the dialog's own fresh check (PF-57): the test above is the mapping's gate
  // (PL-49), the check is the draft's, and the page says "Run the check first" until it ran.
  await proposeFrom(studio);
  const change = await proposedChange(page);
  tick(start);

  clock.person(personMs);
  expect(personMs / 1000, "steward person-seconds stays under 60").toBeLessThan(60);

  start = Date.now();
  await approve(approver.page, PROJECT, change);
  tick(start);

  clock.person(personMs);
  const personSeconds = personMs / 1000;
  info.annotations.push({
    type: "person-seconds",
    description: personSeconds.toFixed(1),
  });
  expect(personSeconds, "the person's part stays under a minute").toBeLessThan(60);

  // Steward polls the pipelines page until the row with the name shows Live (180 s).
  const row = page.getByRole("row").filter({ hasText: NAME });
  await expect(async () => {
    await page.goto(`/projects/${PROJECT}/pipelines?lang=en`, {
      waitUntil: "load",
    });
    await expect(row.getByText("Live")).toBeVisible({ timeout: 5_000 });
  }).toPass({ timeout: 180_000, intervals: [10_000] });
  clock.live();

  // Opens Explore, chooses space/endpoint containing 'kpi' and type KeyPerformanceIndicator,
  // expecting a cell containing bikes-avg-${SUFFIX} within 200 s (reload between polls).
  await page.goto(`/projects/${PROJECT}/explore?lang=en`, {
    waitUntil: "load",
  });

  async function selectKpiTargets() {
    const spaceSelect = page.locator("#explore-space");
    await expect(spaceSelect).toBeVisible({ timeout: 10_000 });
    const spaceVal = await spaceSelect
      .locator("option")
      .evaluateAll((options) =>
        options.map((o) => (o as HTMLOptionElement).value).find((v) => v.includes("kpi")),
      );
    if (spaceVal && (await spaceSelect.inputValue()) !== spaceVal) {
      await spaceSelect.selectOption(spaceVal);
    }

    const endpointSelect = page.locator("#explore-endpoint");
    await expect(endpointSelect).toBeEnabled({ timeout: 10_000 });
    const epVal = await endpointSelect
      .locator("option")
      .evaluateAll((options) =>
        options.map((o) => (o as HTMLOptionElement).value).find((v) => v.includes("kpi")),
      );
    if (epVal && (await endpointSelect.inputValue()) !== epVal) {
      await endpointSelect.selectOption(epVal);
    } else if ((await endpointSelect.inputValue()) === "") {
      const firstNonEmpty = await endpointSelect
        .locator("option")
        .evaluateAll((options) =>
          options.map((o) => (o as HTMLOptionElement).value).find((v) => v !== ""),
        );
      if (firstNonEmpty) {
        await endpointSelect.selectOption(firstNonEmpty);
      }
    }

    const typeInput = page.locator("#explore-type");
    await expect(typeInput).toBeVisible({ timeout: 10_000 });
    if ((await typeInput.evaluate((el) => el.tagName)) === "SELECT") {
      await typeInput.selectOption("KeyPerformanceIndicator");
    } else if ((await typeInput.inputValue()) !== "KeyPerformanceIndicator") {
      await typeInput.fill("KeyPerformanceIndicator");
      await typeInput.press("Enter");
    }
  }

  let firstPoll = true;
  await expect
    .poll(
      async () => {
        if (!firstPoll) {
          await page.reload({ waitUntil: "load" });
        }
        firstPoll = false;
        await selectKpiTargets();
        const cell = page.locator("td").filter({ hasText: NAME }).first();
        return await cell.isVisible().catch(() => false);
      },
      { timeout: 200_000, intervals: [10_000] },
    )
    .toBe(true);

  await steward.context.close();
  await approver.context.close();
});

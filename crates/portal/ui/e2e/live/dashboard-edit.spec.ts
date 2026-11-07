/**
 * T-6429: one real edit of a generated dashboard App on dev. The App is generated from a prompt,
 * published with another person's approval and Served; a computed occupancy column goes through
 * Edit the dashboard, Preview change and Propose change, is approved, rebuilt and read in the App.
 *
 * A generation is paid, so this spec runs only with JC_LIVE_MODEL_RUN=1. The steps run in order
 * and each picks up what the last left on dev, so a failed step reruns without a second
 * generation: DASHBOARD_APP names the App (default t6429-bike-dashboard, residue.spec.ts sweeps
 * that shape), DASHBOARD_DIR receives the 1440 + 400 px shots.
 *
 * Every wait is on the thing the step needs, never on the network going idle (T-2452).
 */
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { APPROVER, STEWARD, VIEWER, approve, csrf, goSignedIn, removeCompletely, signIn } from "./portal";

const PROJECT = process.env.DASHBOARD_PROJECT ?? "helsinki";
const ENDPOINT = process.env.DASHBOARD_ENDPOINT ?? "app-helsinki-bikes";
const SLUG = process.env.DASHBOARD_SLUG ?? "jbbdk2657lsp3mp7bt76ukc4ac";
const NAME = process.env.DASHBOARD_APP ?? "t6429-bike-dashboard";
const DIR = process.env.DASHBOARD_DIR ?? "test-results/dashboard-edit";
const APPS_URL = process.env.APPS_URL ?? "https://dev.joinedcontext.com";
const PROMPT =
  "A dashboard of the shared bike stations, kept in src/dashboard.json and drawn by Dashboard alone: a grid of the stations with their free bikes and total slots, and a tile counting the empty ones. src/App.tsx renders Dashboard and nothing else: no map, no chart, no other component. One page, no other pages. grid.source is { kind: \"endpoint\", slug: \"" + SLUG + "\" }. src/App.tsx passes source={endpointSource(useEndpoint()!.slug, transportFor(useClient().config))} and changes no test file of the template.";
const FORMULA = "availableBikeNumber / totalSlotNumber * 100";
const COLUMN = "Occupancy %";
const ROUTE = `/api/v1/projects/${PROJECT}/apps/${NAME}/dashboard`;
const PAGE = `/projects/${PROJECT}/apps/${NAME}?lang=en`;

test.setTimeout(1_800_000);
test.skip(process.env.JC_LIVE_MODEL_RUN !== "1", "a live model run is paid: JC_LIVE_MODEL_RUN=1 with the owner's go");
test.describe.configure({ mode: "serial" });

interface Read {
  path: string;
  config: { grid?: { columns?: { name?: string; label?: string; formula?: string }[] } } | null;
  attributes: { name: string }[];
  edit: { allowed: boolean; reason?: string };
}

/** One full-height shot at 1440 and 400 px, so the sticky sidebar is not cut; back at 1440 afterwards. */
async function both(page: Page, state: string): Promise<void> {
  mkdirSync(DIR, { recursive: true });
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    const height = await page.evaluate(() => document.documentElement.scrollHeight);
    await page.setViewportSize({ width, height: Math.min(height, 4000) });
    await page.screenshot({ path: join(DIR, `${state}-${width}.png`) });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

async function exists(page: Page): Promise<boolean> {
  return (await page.request.get(`/api/v1/projects/${PROJECT}/apps/${NAME}`)).status() === 200;
}

/** The builder run of the App, if one was started: a rerun resumes it instead of paying for another. */
async function builderRun(page: Page): Promise<boolean> {
  const answer = await page.request.get(`/api/v1/projects/${PROJECT}/agent-runs?limit=50`);
  const { items } = (await answer.json()) as { items: { appName?: string; status?: string }[] };
  return items.some((each) => each.appName === NAME && each.status !== "failed");
}

async function dashboard(page: Page): Promise<Read> {
  const answer = await page.request.get(ROUTE);
  expect(answer.status(), "the steward reads the dashboard").toBe(200);
  return (await answer.json()) as Read;
}

/** Waits until the App serves a commit other than `not` and answers it; a failed build ends the wait. */
async function served(page: Page, not: string): Promise<string> {
  let commit = "";
  await expect
    .poll(
      async () => {
        const answer = await page.request.get(`/api/v1/projects/${PROJECT}/apps/${NAME}`);
        const app = (await answer.json()) as { status?: { build?: { commit?: string } } };
        commit = app.status?.build?.commit ?? "";
        const build = (await (await page.request.get(`/api/v1/projects/${PROJECT}/apps/${NAME}/build`)).json()) as { run?: { conclusion?: string; commit?: string; url?: string } };
        if (build.run?.conclusion === "failure" && build.run.commit !== not) throw new Error(`the build of ${build.run.commit} failed: ${build.run.url}`);
        return commit !== "" && commit !== not;
      },
      { timeout: 900_000, intervals: [15_000] },
    )
    .toBe(true);
  return commit;
}

test("a generated dashboard App keeps its file and is published with another person's approval", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  const { page } = steward;
  try {
    if (!(await builderRun(page))) {
      await page.getByRole("button", { name: "Generate your own app" }).first().click();
      await page.getByLabel("Endpoint").first().selectOption(ENDPOINT);
      await page.getByLabel("What should the app do?").first().fill(PROMPT);
      await page.getByText("Details:", { exact: false }).first().click();
      await page.getByLabel("App name").fill(NAME);
      await page.getByRole("button", { name: "Generate the app" }).click();
      await page.waitForURL(new RegExp(`/projects/${PROJECT}/apps/${NAME}`), { timeout: 60_000 });
    }
    if (!(await exists(page))) {
      // The App is a draft of the run until it is published: only then does /dashboard answer.
      await page.goto(PAGE, { waitUntil: "load" });
      // Publish stays disabled until the tests of the newest version ran; the builder fixes its own
      // red tests first, which took 6 min on 2026-11-02 (versions 2 to 4).
      const publish = page.getByRole("button", { name: "Publish this app" });
      await expect(page.getByTestId("run-progress").filter({ hasText: "Waiting for you" })).toBeVisible({ timeout: 900_000 });
      await expect(publish).toBeEnabled({ timeout: 900_000 });
      await both(page, "1-builder-preview");
      await publish.click();
      const notice = page.getByRole("link", { name: /review/i }).first();
      await expect(notice).toBeVisible({ timeout: 120_000 });
      const change = new URL((await notice.getAttribute("href")) ?? "", page.url()).pathname.split("/").pop() ?? "";
      expect(change, "Publish left a change to approve").not.toBe("");
      const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
      await approve(approver.page, PROJECT, change, NAME);
      await approver.context.close();
      await expect.poll(() => exists(page), { timeout: 300_000, intervals: [10_000] }).toBe(true);
    }
    await served(page, "");
    const read = await dashboard(page);
    expect(read.config, read.edit.reason ?? "the App keeps src/dashboard.json").not.toBeNull();
    expect(read.edit.allowed, read.edit.reason ?? "").toBe(true);
    expect(read.attributes.map((each) => each.name)).toEqual(expect.arrayContaining(["availableBikeNumber", "totalSlotNumber"]));
  } finally {
    await steward.context.close();
  }
});

test("one computed occupancy column is previewed, proposed, approved and seen rebuilt", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, PAGE);
  const { page } = steward;
  try {
    const read = await dashboard(page);
    const columns = read.config?.grid?.columns ?? [];
    if (!columns.some((each) => each.formula === FORMULA)) {
      const before = await served(page, "");
      await page.getByText("Edit the dashboard").click();
      await expect(page.getByRole("group", { name: "Columns" })).toBeVisible();
      await both(page, "2-editor");
      await page.getByRole("button", { name: "Add computed column" }).click();
      await page.locator(`#col-${columns.length}-name`).fill("occupancy");
      await page.locator(`#col-${columns.length}-label`).fill(COLUMN);
      await page.locator(`#col-${columns.length}-formula`).fill(FORMULA);
      await page.getByRole("button", { name: "Preview change" }).click();
      await expect(page.getByText("What the Change will hold").first()).toBeVisible();
      await both(page, "3-preview");
      await page.getByRole("button", { name: "Propose change" }).click();
      const notice = page.getByRole("region", { name: new RegExp(NAME) });
      await expect(notice).toBeVisible({ timeout: 120_000 });
      await both(page, "4-proposed");
      const link = notice.getByRole("link").first();
      const change = new URL((await link.getAttribute("href")) ?? "", page.url()).pathname.split("/").pop() ?? "";
      const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
      await approve(approver.page, PROJECT, change, NAME);
      await approver.context.close();
      await served(page, before);
    }
    await page.goto(PAGE, { waitUntil: "load" });
    await both(page, "5-rebuilt-app-page");
    const after = await dashboard(page);
    expect((after.config?.grid?.columns ?? []).some((each) => each.formula === FORMULA), "the file holds the column").toBe(true);

    // The App itself draws the column with a number per station.
    await goSignedIn(page, STEWARD, `${APPS_URL}/apps/${NAME}/`, (opened) => opened.getByRole("heading", { level: 1 }));
    await expect(page.getByRole("columnheader", { name: COLUMN })).toBeVisible({ timeout: 120_000 });
    const rows = page.getByRole("row").filter({ hasText: /\d/ });
    await expect(rows.nth(1)).toBeVisible({ timeout: 60_000 });
    await both(page, "6-rebuilt-app");
  } finally {
    await steward.context.close();
  }
});

test("the edited App keeps its own Endpoint grant and a viewer may not edit it", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, PAGE);
  try {
    const app = await steward.page.request.get(`/api/v1/projects/${PROJECT}/apps/${NAME}`);
    const manifest = (await app.json()) as { spec: { dataNeeds?: { types?: string[] }[] } };
    expect(manifest.spec.dataNeeds?.flatMap((each) => each.types ?? []), "the App still reads only its own type").toEqual(["BikeHireDockingStation"]);
  } finally {
    await steward.context.close();
  }
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/apps?lang=en`);
  try {
    const refused = await viewer.page.request.post(ROUTE, {
      headers: { "x-csrf-token": await csrf(viewer.context) },
      data: { config: { grid: { source: { kind: "endpoint", slug: ENDPOINT }, type: "BikeHireDockingStation" } }, dryRun: true },
    });
    expect(refused.status(), "a viewer may not propose a dashboard edit").toBe(403);
  } finally {
    await viewer.context.close();
  }
});

test("the App is removed", async ({ browser }) => {
  test.skip(process.env.DASHBOARD_KEEP === "1", "kept for a rerun");
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  try {
    await removeCompletely(steward, PROJECT, "apps", NAME);
    await expect.poll(() => exists(steward.page), { timeout: 300_000, intervals: [10_000] }).toBe(false);
  } finally {
    await steward.context.close();
  }
});

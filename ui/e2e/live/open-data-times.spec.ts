/**
 * T-6541 — how fast the open data portal shows Praha's big datasets to a visitor with no session:
 * the dataset card (its h1), the data grid's first rows and the map's first places, at 1440 and
 * 400 px, cold (a fresh browser) and warm (the same page again). Each step also names its slowest
 * request. The run writes one JSON file per environment to TIMES_DIR (default test-results) and a
 * HAR of each cold 1440 load, bodies left out; evidence/T-6541/times.md is built from them.
 *
 * A timing run is by hand, against one environment at a time (PORTAL_URL), and stays out of the
 * hourly sweep (sweep.sh): `npx playwright test -c playwright.live.config.ts --no-deps
 * --project=chromium e2e/live/open-data-times.spec.ts`.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test, type Page } from "@playwright/test";

test.setTimeout(1_500_000);

/** The model classes T-6502 found slow to count on Praha (finding 4). */
const TYPES = (process.env.TIMES_TYPES ?? "Tree,Garden,GreenAsset,NoiseZone,RoadSegment").split(",");
const WIDTHS = [1440, 400];
/** The budgets of T-6541 at 1440 on a warm page. */
const GRID_BUDGET_MS = 2_000;
const MAP_BUDGET_MS = 3_000;
/** A step that never comes is written down as missing after this long. */
const STEP_CUT_MS = 20_000;

interface Step {
  ms: number | null;
  slowest?: { url: string; ms: number; status: number };
}
interface Row {
  dataset: string;
  type: string;
  width: number;
  phase: "cold" | "warm";
  card: Step;
  grid: Step;
  map: Step;
}

/** The slowest request that ended before `until` (ms since the load began). */
function slowestBefore(requests: { url: string; start: number; end: number; status: number }[], until: number) {
  const done = requests.filter((r) => r.end <= until);
  const slow = done.sort((a, b) => b.end - b.start - (a.end - a.start))[0];
  return slow ? { url: slow.url, ms: Math.round(slow.end - slow.start), status: slow.status } : undefined;
}

async function stepTime(page: Page, started: number, wait: () => Promise<unknown>): Promise<number | null> {
  try {
    await wait();
    return Date.now() - started;
  } catch (err) {
    console.log(`step missing: ${String(err).split("\n")[0]}`);
    return null;
  }
}

async function measure(page: Page, dataset: string): Promise<Pick<Row, "card" | "grid" | "map">> {
  const requests: { url: string; start: number; end: number; status: number }[] = [];
  const opened = new Map<string, number>();
  const started = Date.now();
  page.on("request", (request) => opened.set(request.url() + request.method(), Date.now() - started));
  page.on("requestfinished", (request) => {
    const start = opened.get(request.url() + request.method()) ?? 0;
    void request.response().then((response) =>
      requests.push({ url: request.url(), start, end: Date.now() - started, status: response?.status() ?? 0 }),
    );
  });
  await page.goto(`/catalogue/${dataset}?lang=en`, { waitUntil: "commit" });
  const card = await stepTime(page, started, () => expect(page.getByRole("heading", { level: 1 })).toBeVisible({ timeout: STEP_CUT_MS }));
  const [grid, map] = await Promise.all([
    stepTime(page, started, () =>
      expect(page.locator(".jc-grid tbody tr:not(.jc-grid-spacer)").first()).toBeVisible({ timeout: STEP_CUT_MS }),
    ),
    stepTime(page, started, () =>
      expect(page.getByText(/on the map\.|The map shows the first/)).toBeVisible({ timeout: STEP_CUT_MS }),
    ),
  ]);
  page.removeAllListeners("request");
  page.removeAllListeners("requestfinished");
  const step = (ms: number | null): Step => ({ ms, slowest: slowestBefore([...requests], ms ?? Date.now() - started) });
  return { card: step(card), grid: step(grid), map: step(map) };
}

test("Praha's big datasets show their card, first rows and first places in time", async ({ browser, baseURL }) => {
  const env = new URL(baseURL ?? "https://portal.dev.joinedcontext.com").hostname.split(".")[1] ?? "dev";
  const dir = process.env.TIMES_DIR ?? "test-results";
  mkdirSync(dir, { recursive: true });
  const rows: Row[] = [];
  const probe = await browser.newContext();
  const datasets: { name: string; type: string }[] = [];
  for (const type of TYPES) {
    const answer = await probe.request.get(`/api/v1/catalogue?q=${type}&publisher=praha`);
    expect(answer.ok(), `the catalogue answers a search for ${type}`).toBe(true);
    const found = ((await answer.json()) as { datasets: { name: string }[] }).datasets[0];
    if (found) datasets.push({ name: found.name, type });
  }
  await probe.close();
  expect(datasets.length, "the catalogue lists Praha's big datasets").toBeGreaterThan(0);

  for (const { name, type } of datasets) {
    for (const width of WIDTHS) {
      const context = await browser.newContext({
        // A visitor's browser sends no run header: the live config's would make every Endpoint read
        // from the page a CORS preflight the gateway refuses, and the grid would never load.
        extraHTTPHeaders: {},
        viewport: { width, height: width === 400 ? 860 : 900 },
        ...(width === 1440 ? { recordHar: { path: join(dir, `${env}-${name}.har`), content: "omit" as const } } : {}),
      });
      const page = await context.newPage();
      rows.push({ dataset: name, type, width, phase: "cold", ...(await measure(page, name)) });
      rows.push({ dataset: name, type, width, phase: "warm", ...(await measure(page, name)) });
      // Written after each page, so a run cut short keeps what it measured.
      writeFileSync(join(dir, `times-${env}.json`), JSON.stringify(rows, null, 2));
      await context.close();
    }
  }
  const late = rows.filter(
    (r) => r.width === 1440 && r.phase === "warm" && (r.grid.ms === null || r.grid.ms > GRID_BUDGET_MS || (r.map.ms ?? 0) > MAP_BUDGET_MS),
  );
  expect(late.map((r) => `${r.dataset}: grid ${r.grid.ms} ms, map ${r.map.ms} ms`), "warm at 1440 within budget").toEqual([]);
});

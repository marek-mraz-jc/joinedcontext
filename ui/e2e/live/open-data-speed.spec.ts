/**
 * T-6539 — how fast the open data portal draws, as an anonymous visitor: the catalogue home, a
 * search, and every dataset page of the publishers named in SPEED_PUBLISHERS (default Helsinki and
 * Barcelona). Each page opens cold (a fresh context: no HTTP cache, no session) and then warm (a
 * reload in the same context), at 1440 and 400 px. Per load it times
 *
 * - card: the dataset's h1 (the catalogue home and search: the first dataset card),
 * - grid: the first data row of the Endpoint's grid,
 * - map: the map's canvas after `/map` answered with features,
 *
 * and names the slowest request finished before each mark. Rows go to SPEED_OUT/times.jsonl and a
 * HAR per cold load to SPEED_OUT/har/. Read-only, anonymous; paced under the edge's 600 reads a
 * minute per address (T-5903) by one page at a time.
 *
 *   PORTAL_URL=https://portal.stage.joinedcontext.com SPEED_OUT=/workspace/evidence/T-6539/stage \
 *     npx playwright test --config playwright.live.config.ts open-data-speed --no-deps --project=chromium
 */
import { appendFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Browser, Page, Request } from "@playwright/test";

const OUT = process.env.SPEED_OUT ?? "test-results/open-data-speed";
const PUBLISHERS = (process.env.SPEED_PUBLISHERS ?? "hel-fi,barcelona").split(",").filter(Boolean);
const WIDTHS = [1440, 400];
// SPEED_ONLY, a regex over the page paths, reruns a few pages (a transient outage, a fix).
const ONLY = new RegExp(process.env.SPEED_ONLY ?? "");
const LIMIT_MS = 30_000;

interface Listed {
  name: string;
}
interface CataloguePage {
  total: number;
  pageSize: number;
  datasets: Listed[];
}

type Mark = "card" | "grid" | "map";

interface Row {
  page: string;
  width: number;
  pass: "cold" | "warm";
  card: number | null;
  grid: number | null;
  map: number | null;
  note: string[];
  slowest: Partial<Record<Mark, { url: string; ms: number }>>;
}

/** Every request with its own duration, so a mark can name the slowest one before it. */
function recorder(page: Page): (from: number, before: number) => { url: string; ms: number } | undefined {
  const done: { url: string; ms: number; end: number }[] = [];
  page.on("requestfinished", (request: Request) => {
    const timing = request.timing();
    if (timing.responseEnd < 0) return;
    done.push({ url: request.url(), ms: Math.round(timing.responseEnd), end: timing.startTime + timing.responseEnd });
  });
  return (from, before) =>
    done
      .filter((each) => each.end >= from && each.end <= before)
      .reduce<{ url: string; ms: number } | undefined>((max, each) => (!max || each.ms > max.ms ? { url: each.url, ms: each.ms } : max), undefined);
}

/** Milliseconds from `start` until the locator shows, or null past the limit. */
async function until(start: number, wait: Promise<unknown>): Promise<number | null> {
  try {
    await wait;
    return Date.now() - start;
  } catch {
    return null;
  }
}

const gridNotes = new WeakMap<Page, string>();

/**
 * Milliseconds until the grid's first row, or null with the reason in gridNotes when it drew none.
 * A Portal before T-6376 (prod's release) shows a ten-row sample table there instead: its rows count.
 */
async function gridDrawn(page: Page, start: number): Promise<number | null> {
  gridNotes.delete(page);
  const sample = 'section[aria-labelledby="catalogue-sample"]';
  const drawn = await until(start, page.locator(`tr.jc-grid-tr, .jc-grid-empty, ${sample} tbody tr, ${sample} p:not([role=status])`).first().waitFor({ timeout: LIMIT_MS }));
  if (drawn === null) return null;
  if ((await page.locator(`tr.jc-grid-tr, ${sample} tbody tr`).count()) > 0) {
    if ((await page.locator(sample).count()) > 0) gridNotes.set(page, "grid: the release's sample table");
    return drawn;
  }
  gridNotes.set(page, `grid: empty, said so in ${(drawn / 1000).toFixed(1)} s`);
  return null;
}

const mapNotes = new WeakMap<Page, string>();

/**
 * Milliseconds until the map draws its canvas, or null with the reason in mapNotes: the page
 * draws no map for a dataset without places (404) or with no features, so it waits only for the
 * answer then, not the whole limit.
 */
async function mapDrawn(page: Page, start: number): Promise<number | null> {
  mapNotes.delete(page);
  try {
    const answer = await page.waitForResponse((response) => /\/api\/v1\/catalogue\/datasets\/[^/?]+\/map/.test(response.url()), {
      timeout: LIMIT_MS,
    });
    if (answer.status() === 404) {
      mapNotes.set(page, "no places");
      return null;
    }
    const body = answer.ok() ? ((await answer.json()) as { features?: unknown[] }) : {};
    if (!answer.ok() || !body.features?.length) {
      mapNotes.set(page, `${answer.status()}, ${body.features?.length ?? 0} features`);
      return null;
    }
  } catch {
    mapNotes.set(page, "no answer in 30 s");
    return null;
  }
  return until(start, page.locator('section[aria-labelledby="catalogue-map"] canvas').first().waitFor({ timeout: LIMIT_MS }));
}

async function measure(page: Page, path: string, dataset: boolean, pass: Row["pass"], width: number): Promise<Row> {
  const slowest = recorder(page);
  const note: string[] = [];
  // A 429 from the edge is a refusal, never a slow page: the row names it and is not timed (T-6540).
  let refused = 0;
  page.on("response", (response) => {
    if (response.status() === 429) refused += 1;
  });
  const start = Date.now();
  if (pass === "cold") await page.goto(path, { waitUntil: "commit" });
  else await page.reload({ waitUntil: "commit" });
  const row: Row = { page: path, width, pass, card: null, grid: null, map: null, note, slowest: {} };
  const cardWait = dataset
    ? page.locator("main h1").filter({ hasNotText: /^\s*$/ }).first().waitFor({ timeout: LIMIT_MS })
    : page.locator("main li h2 a").first().waitFor({ timeout: LIMIT_MS });
  const [card, grid, map] = await Promise.all([
    until(start, cardWait),
    dataset ? gridDrawn(page, start) : null,
    dataset ? mapDrawn(page, start) : null,
  ]);
  row.card = card;
  row.grid = grid;
  row.map = map;
  for (const [mark, at] of [["card", card], ["grid", grid], ["map", map]] as const) {
    if (at === null) continue;
    const hit = slowest(start, start + at);
    if (hit) row.slowest[mark] = hit;
  }
  if (refused > 0) {
    note.push(`edge 429 x${refused}: not a timing`);
    row.card = row.grid = row.map = null;
  }
  if (dataset) {
    if (grid !== null && gridNotes.has(page)) note.push(gridNotes.get(page) ?? "");
    if (grid === null) {
      const alerts = await page.getByRole("alert").allInnerTexts();
      note.push(gridNotes.get(page) ?? (alerts.length ? `grid: ${alerts.join(" | ").slice(0, 160)}` : "grid: no row in 30 s"));
    }
    if (map === null) note.push(`map: ${mapNotes.get(page) ?? "no answer"}`);
  }
  return row;
}

async function pages(browser: Browser): Promise<{ path: string; dataset: boolean }[]> {
  const context = await browser.newContext();
  const out: { path: string; dataset: boolean }[] = [
    { path: "/catalogue?lang=en", dataset: false },
    { path: "/catalogue?lang=en&q=air", dataset: false },
  ];
  for (const publisher of PUBLISHERS) {
    out.push({ path: `/catalogue?lang=en&publisher=${publisher}`, dataset: false });
    for (let index = 1; index < 50; index += 1) {
      const answer = await context.request.get(`/api/v1/catalogue?publisher=${publisher}&page=${index}`);
      expect(answer.ok(), `catalogue page ${index} of ${publisher}`).toBe(true);
      const listed = (await answer.json()) as CataloguePage;
      out.push(...listed.datasets.map((each) => ({ path: `/catalogue/${each.name}?lang=en`, dataset: true })));
      if (index * listed.pageSize >= listed.total) break;
    }
  }
  await context.close();
  return out;
}

// One test of an hour: a trace of every load would outgrow the disk, and the HAR names the requests.
test.use({ trace: "off", video: "off", screenshot: "off" });

test("the open data portal draws its card, grid and map in time", async ({ browser }) => {
  test.setTimeout(4 * 60 * 60 * 1000);
  mkdirSync(join(OUT, "har"), { recursive: true });
  const all = (await pages(browser)).filter(({ path }) => ONLY.test(path));
  for (const { path, dataset } of all) {
    for (const width of WIDTHS) {
      const har = join(OUT, "har", `${path.replace(/[^a-z0-9]+/gi, "_")}-${width}.har`);
      const context = await browser.newContext({
        viewport: { width, height: 900 },
        // A visitor's browser sends no run marker; the config's header would add a CORS
        // preflight to every read of the api host the grid makes.
        extraHTTPHeaders: {},
        recordHar: { path: har, content: "omit" },
      });
      const page = await context.newPage();
      for (const pass of ["cold", "warm"] as const) {
        const row = await measure(page, path, dataset, pass, width);
        appendFileSync(join(OUT, "times.jsonl"), `${JSON.stringify(row)}\n`);
      }
      await context.close();
    }
  }
});

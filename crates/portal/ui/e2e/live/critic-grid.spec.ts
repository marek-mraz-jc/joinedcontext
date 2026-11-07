/**
 * T-5521 — the UI critic's walk of the entity grid (sdk/src/grid/EntityGrid.tsx through
 * PortalEntityGrid on /projects/{p}/explore, and the public view /v/{slug}). It records; it asserts
 * nothing about taste. Each persona (demo.editor, demo.viewer, a visitor) at 1440 and 400 px walks
 * the grid's steps, and every step leaves a shot `grid-<state>-<who>-<width>.png` and a line in
 * `walk.jsonl` (what happened, how long it took) under CRITIC_DIR, which the critic scores on the
 * fixed C1-C7 rubric of the task. Only the axe step fails the test: serious or critical violations.
 *
 * Read only: no step sends a write. A typed cell is left with Escape, a paste or a review is not
 * walked here (the seeded `t5521-grid` space with its own Policy needs an approver's sign-in, see
 * the task). Each persona signs in with its own seeded rights; passwords come from the environment
 * only (EDITOR_PASSWORD, VIEWER_PASSWORD) and shoot.ts masks anything token-shaped.
 *
 *   CRITIC_DIR=/workspace/evidence/<worker>/T-5521 EDITOR_PASSWORD=… VIEWER_PASSWORD=… \
 *     npx playwright test -c playwright.live.config.ts critic-grid
 */
import { appendFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Browser, Page } from "@playwright/test";
import { axeViolations } from "../axe";
import { EDITOR, VIEWER, journeyMarkerOnPortalOnly, signIn } from "./portal";
import { secretMasks } from "./shoot";

const DIR = process.env.CRITIC_DIR ?? join("test-results", "critic-grid");
const PROJECT = process.env.CRITIC_PROJECT ?? "helsinki";
/** The space and type of the `Many` walk: dev's largest readable type through the space's first endpoint. */
const SPACE = process.env.CRITIC_SPACE ?? "helsinki";
const TYPE = process.env.CRITIC_TYPE ?? "AirQualityObserved";
const EXPLORE = `/projects/${PROJECT}/explore?lang=en&space=${SPACE}&type=${TYPE}`;
const WIDTHS = [1440, 400];
const LOADS = 5;

interface Step {
  who: string;
  width: number;
  step: string;
  ok: boolean;
  ms?: number;
  note?: string;
  shot?: string;
}

/** Serious or critical axe violations of the running test; each test checks its own at its end. */
let serious: string[] = [];

test.setTimeout(1_800_000);
test.beforeEach(() => {
  serious = [];
});
test.afterEach(() => {
  expect.soft(serious, "axe: no serious or critical violation on the walked states").toEqual([]);
});

/** One line per step in walk.jsonl, written at once: a failed test restarts the worker and loses its memory. */
const steps = {
  push(entry: Step): void {
    mkdirSync(DIR, { recursive: true });
    appendFileSync(join(DIR, "walk.jsonl"), `${JSON.stringify(entry)}\n`);
  },
};

/** p75 of a list of milliseconds, nearest rank. */
function p75(values: number[]): number {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.max(0, Math.ceil(sorted.length * 0.75) - 1)];
}

async function snap(page: Page, state: string, who: string): Promise<string> {
  mkdirSync(DIR, { recursive: true });
  const width = page.viewportSize()?.width ?? 0;
  const file = join(DIR, `grid-${state}-${who}-${width}.png`);
  await page.screenshot({ path: file, fullPage: true, mask: secretMasks(page) });
  return file;
}

/** Runs one step, records its outcome and a shot of the state it leaves; a failed step does not stop the walk. */
async function step(page: Page, who: string, name: string, run: () => Promise<string | void>): Promise<void> {
  const width = page.viewportSize()?.width ?? 0;
  const started = Date.now();
  try {
    const note = await run();
    steps.push({ who, width, step: name, ok: true, ms: Date.now() - started, note: note || undefined, shot: await snap(page, name, who) });
  } catch (error) {
    const note = String(error instanceof Error ? error.message : error).split("\n")[0].slice(0, 300);
    steps.push({ who, width, step: name, ok: false, ms: Date.now() - started, note, shot: await snap(page, name, who).catch(() => undefined) });
  }
}

async function axe(page: Page, who: string, where: string): Promise<void> {
  const width = page.viewportSize()?.width ?? 0;
  const found = await axeViolations(page);
  const bad = found.filter((line) => /\((serious|critical)\)/.test(line));
  serious.push(...bad.map((line) => `${who} ${width} ${where}: ${line}`));
  steps.push({ who, width, step: `axe-${where}`, ok: bad.length === 0, note: found.map((line) => line.slice(0, 160)).join(" | ") || "clean" });
}

/** Controls in the main region smaller than 44 x 44 px at this width (C6 touch targets). */
async function smallTargets(page: Page): Promise<string> {
  const small = await page.locator("main").evaluate((main) =>
    [...main.querySelectorAll<HTMLElement>("button, a[href], select, input, [role=tab], [role=gridcell][tabindex]")]
      .filter((el) => {
        const box = el.getBoundingClientRect();
        return box.width > 0 && box.height > 0 && (box.width < 44 || box.height < 44);
      })
      .map((el) => `${el.tagName.toLowerCase()}:${(el.getAttribute("aria-label") ?? el.textContent ?? "").trim().slice(0, 30)}`),
  );
  return `${small.length} under 44 px: ${small.slice(0, 12).join("; ")}`;
}

const firstRow = (page: Page) => page.locator("tbody tr").first();

/** Opens the explore address `LOADS` times and records the first row's arrival (C5). */
async function timedLoads(page: Page, who: string, address: string, row: () => ReturnType<typeof firstRow>, label: string): Promise<void> {
  const times: number[] = [];
  const failed: string[] = [];
  for (let load = 0; load < LOADS; load += 1) {
    const started = Date.now();
    await page.goto(address, { waitUntil: "commit" });
    if (load === 0) {
      await page.waitForTimeout(300);
      steps.push({ who, width: page.viewportSize()?.width ?? 0, step: `${label}-loading`, ok: true, shot: await snap(page, `${label}-loading`, who) });
    }
    // A load that shows no row is recorded with what the page said instead, and the walk goes on.
    const shown = await expect(row())
      .toBeVisible({ timeout: 60_000 })
      .then(() => true, () => false);
    if (shown) {
      times.push(Date.now() - started);
    } else {
      failed.push((await page.locator("main").innerText()).replace(/\s+/g, " ").slice(0, 120));
    }
  }
  steps.push({
    who,
    width: page.viewportSize()?.width ?? 0,
    step: `${label}-first-row`,
    ok: failed.length === 0,
    ms: times.length > 0 ? p75(times) : undefined,
    note: `p75 of ${times.join(", ")} ms; ${failed.length} of ${LOADS} loads showed no row${failed.length > 0 ? `: ${failed.join(" | ")}` : ""}`,
  });
  await expect(row(), `at least one of ${LOADS} loads shows a row`).toBeVisible({ timeout: 60_000 });
}

/** The grid steps of one signed-in persona at one width. */
async function walkGrid(page: Page, who: string): Promise<void> {
  await timedLoads(page, who, EXPLORE, () => firstRow(page), "many");
  await step(page, who, "many", async () => `${await page.getByText(/ of \d+ loaded/).first().textContent()}; access: ${await page.getByRole("region", { name: "Your access" }).innerText().catch(() => "none shown")}`);
  await axe(page, who, "many");
  await step(page, who, "touch-targets", () => smallTargets(page));

  await step(page, who, "keyboard", async () => {
    await page.getByRole("gridcell").nth(2).click();
    const moves: number[] = [];
    for (const key of ["ArrowRight", "ArrowDown", "ArrowLeft", "ArrowUp"]) {
      // The grid keeps focus on itself and moves the active cell by aria-activedescendant.
      const grid = page.getByRole("grid").first();
      const before = await grid.getAttribute("aria-activedescendant");
      const started = Date.now();
      await page.keyboard.press(key);
      const moved = await expect
        .poll(() => grid.getAttribute("aria-activedescendant"), { timeout: 2_000, intervals: [10] })
        .not.toBe(before)
        .then(() => true, () => false);
      moves.push(moved ? Date.now() - started : -1);
    }
    const focus = await page.evaluate(() => {
      const el = document.activeElement as HTMLElement | null;
      const style = el ? getComputedStyle(el) : null;
      return `${el?.tagName} ${el?.getAttribute("role")} outline ${style?.outlineStyle} ${style?.outlineWidth} shadow ${style?.boxShadow !== "none"}`;
    });
    return `moves ${moves.join(", ")} ms (-1: focus did not move); focus ${focus}`;
  });

  await step(page, who, "typed", async () => {
    const started = Date.now();
    await page.keyboard.press("7");
    await page.waitForTimeout(400);
    const editor = await page.locator("tbody input, tbody textarea, tbody select").count();
    const said = (await page.locator("[role=status], [role=alert]").allInnerTexts()).join(" | ").slice(0, 200);
    await page.keyboard.press("Escape");
    return `editor opened: ${editor > 0} after ${Date.now() - started} ms; messages: ${said || "none"}`;
  });

  await step(page, who, "scrolled", async () => {
    const started = Date.now();
    const scroller = page.locator(".jc-grid-scroll, [data-grid-scroll]").first();
    const target = (await scroller.count()) ? scroller : page.locator("main");
    let rows = 0;
    for (let round = 0; round < 60 && rows < 2_000; round += 1) {
      await target.evaluate((el) => el.scrollBy(0, 20_000)).catch(() => page.mouse.wheel(0, 20_000));
      await page.waitForTimeout(500);
      rows = Number((await page.getByText(/ of \d+ loaded/).first().textContent().catch(() => ""))?.match(/^(\d+)/)?.[1] ?? 0);
    }
    const pager = await page.getByRole("button", { name: /^Page \d+/ }).count();
    return `${rows} rows loaded after ${Date.now() - started} ms; pager buttons ${pager}`;
  });

  await page.evaluate(() => window.scrollTo(0, 0));
  await page.locator(".jc-grid-scroll").first().evaluate((el) => el.scrollTo(0, 0)).catch(() => undefined);
  await step(page, who, "detail", async () => {
    await page.getByRole("button", { name: /^Open: / }).first().click();
    const dialog = page.getByRole("dialog").first();
    await expect(dialog).toBeVisible({ timeout: 30_000 });
    return (await dialog.innerText()).replace(/\s+/g, " ").slice(0, 400);
  });
  await axe(page, who, "detail");
  await step(page, who, "history", async () => {
    await page.getByRole("dialog").first().getByRole("tab", { name: "History" }).click();
    await page.waitForTimeout(3_000);
    return (await page.getByRole("dialog").first().innerText()).replace(/\s+/g, " ").slice(0, 400);
  });
  await page.keyboard.press("Escape");

  await step(page, who, "export", async () => {
    await page.getByLabel("Export format").selectOption("csv");
    const started = Date.now();
    await page.getByRole("button", { name: "Export the view" }).click();
    const link = page.getByRole("link", { name: /^Download / });
    await expect(link.or(page.getByText(/no rows to export/))).toBeVisible({ timeout: 120_000 });
    return `${await link.textContent().catch(() => "no link")} after ${Date.now() - started} ms`;
  });

  await step(page, who, "filtered-one", async () => {
    await page.getByRole("combobox", { name: "Filter: name" }).selectOption({ label: "contains" });
    await page.getByRole("textbox", { name: "Value: name" }).fill("Kallio");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(2_500);
    return `${await page.locator("tbody tr").count()} rows; ${await page.getByText(/ of \d+ loaded/).first().textContent().catch(() => "no count")}`;
  });
  await step(page, who, "empty", async () => {
    await page.getByRole("textbox", { name: "Value: name" }).fill("zz-no-such-station");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(2_500);
    return (await page.getByRole("tabpanel", { name: "Entities" }).innerText()).split("\n").slice(-8).join(" / ").slice(0, 300);
  });
  await page.getByRole("combobox", { name: "Filter: name" }).selectOption({ label: "All" }).catch(() => undefined);

  await step(page, who, "column-menu", async () => {
    await page.getByRole("button", { name: "Column options: source" }).click();
    return (await page.getByRole("dialog", { name: "Column options: source" }).innerText()).replace(/\s+/g, " ");
  });
  await step(page, who, "column-hidden", async () => {
    await page.getByRole("dialog", { name: "Column options: source" }).getByRole("button", { name: "Hide field" }).click();
    const restore = await page.getByRole("button", { name: /hidden|Show|field/i }).allInnerTexts();
    return `restore controls: ${restore.join(" / ") || "none found"}`;
  });
  // The way back (T-5620): the count names the hidden field, its list brings it back by keyboard.
  await step(page, who, "column-restore", async () => {
    const toggle = page.getByRole("button", { name: "Hidden fields (1)" });
    await toggle.focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("list", { name: "Hidden fields" })).toBeVisible();
    await snap(page, "column-restore-list", who);
    await page.getByRole("button", { name: "Show field: source" }).focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("button", { name: "Column options: source" })).toBeFocused();
    return "source back, focus on its column options";
  });

  await page.getByRole("textbox", { name: "Value: name" }).fill("").catch(() => undefined);
  await step(page, who, "sorted", async () => {
    const before = await firstRow(page).innerText();
    const started = Date.now();
    await page.getByRole("button", { name: /^Sort all rows by pm10/ }).click();
    await expect.poll(() => firstRow(page).innerText(), { timeout: 30_000 }).not.toBe(before);
    return `first row changed after ${Date.now() - started} ms`;
  });

  // The grid keeps its sort per browser: what the next visit shows, then a clean start.
  await step(page, who, "reopened", async () => {
    await page.goto(EXPLORE, { waitUntil: "load" });
    await page.waitForTimeout(5_000);
    const kept = await page.locator("main").innerText();
    await page.evaluate(() => {
      localStorage.clear();
      sessionStorage.clear();
    });
    await page.goto(EXPLORE, { waitUntil: "load" });
    await expect(firstRow(page)).toBeVisible({ timeout: 60_000 });
    return `before clearing storage: ${kept.match(/[^\n]* loaded[^\n]*/)?.[0] ?? "no count"}`;
  });
  await step(page, who, "refused-type", async () => {
    const types = page.locator("#explore-type");
    const refused = await types.locator("option", { hasText: "not granted" }).first().getAttribute("value");
    if (!refused) return "every type is granted";
    await types.selectOption(refused);
    await page.waitForTimeout(3_000);
    return `${refused}: ${(await page.locator("main").innerText()).match(/[^\n]*(not|refus|denied|permission)[^\n]*/i)?.[0] ?? "no reason shown"}`;
  });

  await step(page, who, "unknown-type", async () => {
    await page.goto(`/projects/${PROJECT}/explore?lang=en&space=${SPACE}&type=NoSuchType`, { waitUntil: "load" });
    await page.waitForTimeout(5_000);
    return (await page.locator("main").innerText()).split("\n").slice(-6).join(" / ").slice(0, 300);
  });
}

async function persona(browser: Browser, who: { user: string; password: string }, name: string): Promise<void> {
  const { context, page } = await signIn(browser, who, `/projects/${PROJECT}/explore?lang=en`);
  try {
    for (const width of WIDTHS) {
      await page.setViewportSize({ width, height: width === 400 ? 860 : 1000 });
      await walkGrid(page, name);
    }
  } finally {
    await context.close();
  }
}

test("the editor walks the grid", async ({ browser }) => {
  await persona(browser, EDITOR, "editor");
});

test("the viewer walks the grid", async ({ browser }) => {
  await persona(browser, VIEWER, "viewer");
});

test("a visitor walks a public view", async ({ browser }) => {
  const context = await browser.newContext();
  // The view reads the api host after a 308; the journey header there fails its CORS preflight.
  await journeyMarkerOnPortalOnly(context, new URL(process.env.PORTAL_URL ?? "https://portal.dev.joinedcontext.com").host);
  const page = await context.newPage();
  try {
    const listed = (await (await page.request.get("/api/v1/catalogue")).json()) as { datasets: { name: string }[] };
    // The first dataset whose public view opens; the ones that do not are recorded (a catalogue entry
    // pointing at a view that is not published).
    let slug: string | undefined;
    const unpublished: string[] = [];
    for (const dataset of listed.datasets.slice(0, 15)) {
      const detail = (await (await page.request.get(`/api/v1/catalogue/datasets/${dataset.name}`)).json()) as { endpoint?: { url?: string } };
      const candidate = /\/api\/endpoint\/([^/]+)\//.exec(detail.endpoint?.url ?? "")?.[1];
      if (!candidate) continue;
      await page.goto(`/v/${candidate}?lang=en`, { waitUntil: "load" });
      const row = page.locator("table tbody tr").first();
      const closed = page.getByText("This view is not published");
      await expect(row.or(closed)).toBeVisible({ timeout: 60_000 }).catch(() => undefined);
      if (await row.isVisible()) {
        slug = candidate;
        break;
      }
      unpublished.push(`${dataset.name} -> /v/${candidate}`);
      if (unpublished.length === 1) await step(page, "anonymous", "public-unpublished", async () => "first catalogue dataset's view");
    }
    steps.push({ who: "anonymous", width: 0, step: "public-catalogue", ok: Boolean(slug), note: `opened ${slug}; not published: ${unpublished.join(", ") || "none"}` });
    expect(slug, "dev publishes at least one public view a catalogue dataset points at").toBeTruthy();
    for (const width of WIDTHS) {
      await page.setViewportSize({ width, height: width === 400 ? 860 : 1000 });
      await timedLoads(page, "anonymous", `/v/${slug}?lang=en`, () => page.locator("table tbody tr").first(), "public");
      await step(page, "anonymous", "public", async () => `/v/${slug}: ${(await page.locator("main").innerText()).replace(/\s+/g, " ").slice(0, 300)}`);
      await axe(page, "anonymous", "public");
      await step(page, "anonymous", "public-touch-targets", () => smallTargets(page));
    }
  } finally {
    await context.close();
  }
});

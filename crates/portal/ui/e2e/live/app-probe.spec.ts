/**
 * The App probe (T-2795, AP-136): every published App opens, reads data and keeps strangers out.
 *
 * Signed in as `demo.probe`, a member of every App's default group who reads `App` and nothing
 * else, the probe lists the published Apps of every project it may read and, per App:
 *   1. opens it inside the Portal (`/projects/{project}/apps/{name}/open`) and waits for one row
 *      the App reads through `/api/endpoint/{slug}/ngsi-ld/v1/`;
 *   2. opens its own address ("Open in new window") and waits for a row there too, then lays it
 *      out at 375, 768, 1440 and 2560 px: one h1, nothing scrolling sideways, no response of
 *      400 or above, and at 1440 px a first view showing data items (T-3580);
 *   3. opens that address in a fresh context, signed in as nobody: a `public` App must show
 *      rows, any other must send the visitor to sign in or refuse them;
 * and records every console error on the way. `scripts/app-probe.ts` turns what it saw into
 * the summary of the check `apps` (APP_PROBE_OUT); `app-probe.sh` files and publishes it.
 *
 * The one test is green when the probe ran, whatever the Apps' verdicts: a broken App is a task
 * and a red chip, not a red journey. Every wait is on the thing the step needs (T-2452).
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { expect, test } from "@playwright/test";
import type { Browser, BrowserContext, Page, Response } from "@playwright/test";
import { DATA_WAIT_S, WIDTHS, firstViewItems, summaryOf } from "../../scripts/app-probe";
import type { Layout, Observation } from "../../scripts/app-probe";
import { PROBE, journeyMarkerOnPortalOnly, signIn } from "./portal";

const OUT = process.env.APP_PROBE_OUT ?? "test-results/app-probe/summary.json";
const ROWS = /\/api\/endpoint\/[^/?#]+\/ngsi-ld\/v1\/(temporal\/)?entities(?:[?#]|$)/;

test.setTimeout(3_600_000);

interface Listed {
  metadata: { name: string };
  spec?: { lifecycle?: string; visibility?: string };
}

/** Milliseconds from `open` until a response of the page carries one row, or null. */
async function firstRow(page: Page, open: () => Promise<unknown>): Promise<number | null> {
  const started = Date.now();
  const rows = page
    .waitForResponse(
      async (response: Response) => {
        if (!ROWS.test(response.url()) || response.status() !== 200) return false;
        const body: unknown = await response.json().catch(() => null);
        return Array.isArray(body) && body.length > 0;
      },
      { timeout: DATA_WAIT_S * 1000 },
    )
    .then(() => Date.now() - started)
    .catch(() => null);
  await open();
  return rows;
}

/** Every `console.error` and uncaught exception of a page and its frames. */
function collectErrors(page: Page, into: string[]): void {
  page.on("console", (message) => {
    if (message.type() === "error") into.push(message.text());
  });
  page.on("pageerror", (error) => into.push(`${error.name}: ${error.message}`));
}

/** `firstViewItems` as page source: its own `document`, no argument to type. */
const COUNT_ITEMS = `(${firstViewItems.toString()})()`;

/** Its own window at each width, and at 1440 px the data items its first view shows. */
async function layOut(page: Page): Promise<{ layout: Layout[]; items: number }> {
  const layout: Layout[] = [];
  let items = 0;
  for (const width of WIDTHS) {
    await page.setViewportSize({ width, height: width < 800 ? 820 : 1000 });
    // Two frames: what the resize re-rendered is drawn before it is measured.
    await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
    if (width === 1440) {
      await page.waitForFunction(COUNT_ITEMS, undefined, { timeout: 10_000 }).catch(() => null);
      items = await page.evaluate<number>(COUNT_ITEMS);
    }
    layout.push(
      await page.evaluate((at) => ({
        width: at,
        h1: document.querySelectorAll("h1").length,
        sideways: document.documentElement.scrollWidth > window.innerWidth + 1,
      }), width),
    );
  }
  return { layout, items };
}

async function anonymousVisit(browser: Browser, address: string, isPublic: boolean): Promise<Observation["anonymous"]> {
  // A stranger sends no journey marker: the live config's would preflight every cross-origin read.
  const context = await browser.newContext({ extraHTTPHeaders: {} });
  try {
    const page = await context.newPage();
    let status = 0;
    page.on("response", (response) => {
      if (response.request().isNavigationRequest() && response.frame() === page.mainFrame()) status = response.status();
    });
    const rows = await firstRow(page, () => page.goto(address, { waitUntil: "load" }).catch(() => null));
    if (rows !== null) return "data";
    if (new URL(page.url()).pathname.includes("/realms/") || status === 401 || status === 403) return "refused";
    // A public App that showed no rows and a login App that showed neither a sign-in nor a refusal.
    return isPublic ? "refused" : "blank";
  } finally {
    await context.close();
  }
}

async function probe(browser: Browser, context: BrowserContext, page: Page, project: string, app: Listed): Promise<Observation> {
  const name = app.metadata.name;
  const visibility = app.spec?.visibility ?? "project";
  const consoleErrors: string[] = [];
  let refused = false;
  const onResponse = (response: Response) => {
    if (response.request().resourceType() === "document" && response.status() === 403) refused = true;
  };
  page.on("response", onResponse);
  collectErrors(page, consoleErrors);
  const dataMs = await firstRow(page, () => page.goto(`/projects/${project}/apps/${name}/open?lang=en`, { waitUntil: "load" }));
  page.off("response", onResponse);
  page.removeAllListeners("console");
  page.removeAllListeners("pageerror");
  if (refused) {
    return {
      project, name, visibility, refused, dataMs: null, windowDataMs: null, consoleErrors: [],
      failedRequests: [], layout: [], items: null, anonymous: "refused",
    };
  }

  const link = page.getByRole("link", { name: /Open in new window/ });
  const address = (await link.count()) ? await link.first().getAttribute("href") : null;
  let windowDataMs: number | null = null;
  const failedRequests: string[] = [];
  let layout: Layout[] = [];
  let items: number | null = null;
  let anonymous: Observation["anonymous"] = "blank";
  if (address) {
    const own = await context.newPage();
    collectErrors(own, consoleErrors);
    own.on("response", (response) => {
      if (response.status() >= 400) failedRequests.push(`${response.status()} ${response.url()}`);
    });
    windowDataMs = await firstRow(own, () => own.goto(address, { waitUntil: "load" }));
    ({ layout, items } = await layOut(own));
    await own.close();
    anonymous = await anonymousVisit(browser, address, visibility === "public");
  }
  return { project, name, visibility, refused, dataMs, windowDataMs, consoleErrors, failedRequests, layout, items, anonymous };
}

test("every published App opens, reads data and keeps strangers out (AP-136)", async ({ browser }) => {
  const observations: Observation[] = [];
  const { context, page } = await signIn(browser, PROBE, "/?lang=en");
  // An App on *.apps.{domain} reads the Portal's basemap style cross-origin; the marker on it forced
  // a CORS preflight no person's browser sends, and 15 Apps were filed with a blank map (T-5416).
  await journeyMarkerOnPortalOnly(context, new URL(page.url()).host);
  try {
    const projects = await page.request.get("/api/v1/projects");
    expect(projects.ok(), "the probe lists the projects").toBe(true);
    const names = ((await projects.json()) as { items: { name: string }[] }).items.map((p) => p.name);
    for (const project of names) {
      const apps = await page.request.get(`/api/v1/projects/${project}/apps`);
      // A project whose Apps the probe may not read is not the probe's to check.
      if (!apps.ok()) continue;
      const published = ((await apps.json()) as { items: Listed[] }).items.filter(
        (app) => app.spec?.lifecycle === "published",
      );
      for (const app of published) {
        observations.push(await probe(browser, context, page, project, app));
      }
    }
  } finally {
    await context.close();
    mkdirSync(dirname(OUT), { recursive: true });
    writeFileSync(OUT, `${JSON.stringify(summaryOf(observations, `app-probe ${new Date().toISOString()}`), null, 2)}\n`);
  }
  expect(observations.length, "at least one published App was probed").toBeGreaterThan(0);
});

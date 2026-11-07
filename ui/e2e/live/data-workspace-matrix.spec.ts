/**
 * T-5064 step 2 — the data workspace's matrix on dev: person x width x theme x language x view.
 * Since T-6210 a space's views are the grid and the API tab; a visitor's are the public view
 * `/v/{slug}` and form `/f/{slug}`. Read only. Each cell leaves a shot and a verdict line (no
 * sideways scroll, one h1, no serious axe finding) in MATRIX_DIR/matrix.jsonl; a red verdict fails
 * the run at its end, so one bad cell still leaves every other shot.
 *
 *   MATRIX_DIR=... VIEWER_PASSWORD=... EDITOR_PASSWORD=... [PORTAL_PASSWORD=... PORTAL_TOTP=...] \
 *     npx playwright test -c playwright.live.config.ts --no-deps --project=chromium data-workspace-matrix
 */
import { appendFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "../axe";
import { EDITOR, STEWARD, VIEWER, signIn } from "./portal";

test.setTimeout(1_800_000);

const DIR = process.env.MATRIX_DIR ?? join("test-results", "data-workspace-matrix");
const TYPE = process.env.MATRIX_TYPE ?? "Event";
const WIDTHS = [1440, 400];
const THEMES = ["light", "dark"] as const;
const LANGS = ["en", "sk"];

/** One cell: draw it, shoot it, judge it. */
async function cell(page: Page, who: string, view: string, address: (lang: string) => string, ready: (page: Page) => Promise<void>): Promise<string[]> {
  const red: string[] = [];
  for (const width of WIDTHS) {
    for (const theme of THEMES) {
      for (const lang of LANGS) {
        const name = `${who}-${view}-${width}-${theme}-${lang}`;
        await page.setViewportSize({ width, height: 900 });
        await page.emulateMedia({ colorScheme: theme });
        await page.goto(address(lang), { waitUntil: "load" });
        await ready(page);
        await expect(page.getByText(/^(Loading|Načítava)/), "the cell is drawn, not its skeleton").toHaveCount(0, { timeout: 60_000 });
        const sideways = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
        const headings = await page.getByRole("heading", { level: 1 }).count();
        const axe = await axeViolations(page);
        await page.screenshot({ path: join(DIR, `${name}.png`), fullPage: true });
        const ok = sideways <= 0 && headings === 1 && axe.length === 0;
        appendFileSync(join(DIR, "matrix.jsonl"), `${JSON.stringify({ cell: name, ok, sideways, headings, axe })}\n`);
        if (!ok) red.push(name);
      }
    }
  }
  return red;
}

const PEOPLE = [
  ["viewer", VIEWER],
  ["editor", EDITOR],
  ...(process.env.PORTAL_TOTP ? ([["steward", STEWARD]] as const) : []),
] as const;

for (const [who, person] of PEOPLE) {
  test(`${who}: grid and API at every width, theme and language`, async ({ browser }) => {
    mkdirSync(DIR, { recursive: true });
    const { context, page } = await signIn(browser, person, "/projects/helsinki/spaces/helsinki?lang=en");
    try {
      const space = (lang: string) => `/projects/helsinki/spaces/helsinki?lang=${lang}&type=${TYPE}`;
      const grid = await cell(page, who, "grid", space, async (p) => {
        // A read of 100 rows can pass dev's 10 s cut under load; the page then says so with no rows.
        await expect(async () => {
          if ((await p.getByRole("button", { name: /Retry|Skúsiť/ }).count()) > 0 || (await p.locator("tbody tr td").count()) === 0) await p.reload();
          await expect(p.locator("tbody tr td").first()).toBeVisible({ timeout: 25_000 });
        }).toPass({ timeout: 120_000 });
      });
      const api = await cell(page, who, "api", (lang) => `${space(lang)}&view=api`, async (p) => {
        await expect(p.locator("#space-data-view-tab-api")).toHaveAttribute("aria-selected", "true", { timeout: 60_000 });
        await p.waitForTimeout(1_500);
      });
      expect([...grid, ...api]).toEqual([]);
    } finally {
      await context.close();
    }
  });
}

test("visitor: the public view and form at every width, theme and language", async ({ browser }) => {
  mkdirSync(DIR, { recursive: true });
  const context = await browser.newContext();
  const page = await context.newPage();
  try {
    const listed = (await (await page.request.get("/api/v1/catalogue")).json()) as { datasets: { name: string }[] };
    let slug: string | undefined;
    for (const dataset of listed.datasets) {
      const detail = (await (await page.request.get(`/api/v1/catalogue/datasets/${dataset.name}`)).json()) as { endpoint?: { url?: string } };
      slug = /\/api\/endpoint\/([^/]+)\//.exec(detail.endpoint?.url ?? "")?.[1];
      if (slug) break;
    }
    expect(slug, "dev publishes a public Endpoint").toBeTruthy();
    const ready = async (p: Page) => {
      await expect(p.getByRole("heading", { level: 1 })).toBeVisible({ timeout: 60_000 });
      await p.waitForTimeout(1_500);
    };
    const shown = await cell(page, "visitor", "view", (lang) => `/v/${slug}?lang=${lang}`, ready);
    const form = await cell(page, "visitor", "form", (lang) => `/f/${slug}?lang=${lang}`, ready);
    expect([...shown, ...form]).toEqual([]);
  } finally {
    await context.close();
  }
});

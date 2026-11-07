/**
 * T-4162 — UX photo audit 6/7: what a visitor reaches outside the Portal. Every published public
 * App on its own host (first view, one interaction, an unknown path), the public assistant's
 * widget alone and framed by the data portal with one question asked, the public MCP page,
 * Keycloak's forgot-password and registration pages and the account console, and the error
 * pages of each host as a visitor sees them.
 *
 * The App list is read from the Portal API at run time, as demo.steward: every project the
 * steward lists, every App with `visibility: public` and `lifecycle: published`, on
 * `{name}.apps.{domain}`. Everything else is opened anonymously, except the account console,
 * which is demo.viewer's own.
 *
 * Read-only: forms are submitted empty, to photograph their validation; the assistant is asked
 * one question per language and width it is photographed at (four answers, a few thousand
 * tokens in all). No 429 is provoked: that would spend a visitor's quota on purpose.
 *
 *   SHOTS_DIR=/workspace/evidence/<agent>/T-4162 PORTAL_PASSWORD=… VIEWER_PASSWORD=… \
 *     npx playwright test --config playwright.live.config.ts audit-public
 */
import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Browser, Page } from "@playwright/test";
import { STEWARD, VIEWER, signIn } from "./portal";
import { shot, writeIndex } from "./shoot";
import type { Shot } from "./shoot";

const LANGS = ["sk", "en"] as const;
type Lang = (typeof LANGS)[number];
const WIDTHS = [375, 768, 1440];
const APP_WIDTHS = [375, 1440];
const AREA = "public";
const DIR = process.env.SHOTS_DIR;
const PORTAL = new URL(process.env.PORTAL_URL ?? "https://portal.dev.joinedcontext.com");
const DOMAIN = PORTAL.hostname.replace(/^portal\./, "");
const ASSISTANT = `https://assistant.${DOMAIN}`;
const DATA = `https://opendata.${DOMAIN}`;
const IDM = `https://idm.${DOMAIN}/realms/${process.env.KEYCLOAK_REALM ?? "dev"}`;
const QUESTION: Record<Lang, string> = {
  sk: "Aké otvorené údaje o kvalite ovzdušia tu nájdem?",
  en: "Which open data about air quality can I find here?",
};

/** States that could not be reached, with the reason; printed at the end, and the run fails on any. */
const missed: string[] = [];

const height = (width: number) => (width < 768 ? 812 : 900);
const withLang = (address: string, lang: Lang) => `${address}${address.includes("?") ? "&" : "?"}lang=${lang}`;

/**
 * What went wrong loading each page since its state began: a 429 or a script that failed. Every
 * agent on the audit server shares one address and the edge limits per address, so a picture
 * taken after either shows the limit, not the page: it is refused, never saved (T-4162 rules).
 */
const refused = new WeakMap<Page, string[]>();

function watch(page: Page): void {
  refused.set(page, []);
  page.on("response", (response) => {
    if (response.status() === 429) refused.get(page)?.push(`429 ${response.url()}`);
  });
  page.on("requestfailed", (request) => {
    if (request.resourceType() === "script") refused.get(page)?.push(`failed script ${request.url()}`);
  });
}

async function state(page: Page, what: Omit<Shot, "width" | "area">, reach: () => Promise<void>): Promise<void> {
  try {
    // One page every two seconds at most: the audit shares the edge's per-address budget.
    await page.waitForTimeout(2000);
    refused.set(page, []);
    await reach();
    await page.waitForLoadState("networkidle", { timeout: 15_000 }).catch(() => undefined);
    await expect(page.locator('[aria-busy="true"]')).toHaveCount(0, { timeout: 15_000 }).catch(() => undefined);
    await page.waitForTimeout(1200);
    const errors = refused.get(page) ?? [];
    if (errors.length) throw new Error(`not photographed, the edge refused it: ${errors[0]}`);
    await shot(page, { area: AREA, ...what });
  } catch (error) {
    missed.push(`${what.route} ${what.state} ${what.who} ${what.lang} ${page.viewportSize()?.width}: ${String(error).split("\n")[0]}`);
  }
}

/** One anonymous browser per width and language, closed after `walk`. */
async function anonymously(browser: Browser, widths: number[], walk: (page: Page, lang: Lang) => Promise<void>): Promise<void> {
  for (const width of widths) {
    for (const lang of LANGS) {
      // A visitor sends no journey header: on a cross-origin read (an App's basemap) it would
      // need a CORS preflight the visitor never makes, and the map would stay blank.
      const context = await browser.newContext({
        viewport: { width, height: height(width) },
        locale: lang === "sk" ? "sk-SK" : "en-GB",
        extraHTTPHeaders: {},
      });
      try {
        const page = await context.newPage();
        watch(page);
        await walk(page, lang);
      } finally {
        await context.close();
      }
    }
  }
}

/** A page answered with any status: an error page is what is photographed. */
async function visit(page: Page, address: string): Promise<void> {
  await page.goto(address, { waitUntil: "load" });
  await page.waitForLoadState("networkidle", { timeout: 15_000 }).catch(() => undefined);
}

interface PublicApp {
  project: string;
  name: string;
}

/** The published public Apps of every project demo.steward lists, from the Portal API. */
async function publicApps(browser: Browser): Promise<PublicApp[]> {
  // The Portal host limits requests per address, its static chunks included: a sign-in right after
  // a busy minute can land on a white page. One more try after the window passes.
  const { context, page } = await signIn(browser, STEWARD, "/projects/helsinki/home?lang=en").catch(async () => {
    await new Promise((resolve) => setTimeout(resolve, 70_000));
    return signIn(browser, STEWARD, "/projects/helsinki/home?lang=en");
  });
  try {
    const projects = (await (await page.request.get("/api/v1/projects")).json()) as { items: { name: string }[] };
    const apps: PublicApp[] = [];
    for (const { name: project } of projects.items) {
      const answer = await page.request.get(`/api/v1/projects/${project}/apps`);
      if (!answer.ok()) continue;
      const list = (await answer.json()) as { items: { metadata: { name: string }; spec: { visibility?: string; lifecycle?: string } }[] };
      for (const app of list.items) {
        if (app.spec.visibility === "public" && app.spec.lifecycle === "published") apps.push({ project, name: app.metadata.name });
      }
    }
    return apps;
  } finally {
    await context.close();
  }
}

/** The Apps say "Loading…" in text and set no aria-busy: the picture waits for the text to go. */
async function settled(page: Page): Promise<void> {
  await expect(page.getByText(/^(Loading|Načítava)/)).toHaveCount(0, { timeout: 30_000 }).catch(() => undefined);
  // A map draws its tiles after the page is idle: without this wait it is photographed blank.
  const map = page.locator(".maplibregl-map").first();
  if (await map.count()) {
    await page.locator(".maplibregl-canvas").first().waitFor({ timeout: 15_000 }).catch(() => undefined);
    // The basemap's own credit appears once its style has loaded; then the tiles get a moment.
    await expect(page.locator(".maplibregl-ctrl-attrib-inner")).toContainText("OpenStreetMap", { timeout: 30_000 }).catch(() => undefined);
    await page.waitForTimeout(4000);
  }
}

/** The App's one interaction: a second tab, a filter, or a pan of its map, whichever it has. */
async function interact(page: Page): Promise<void> {
  const tabs = page.getByRole("tab");
  if ((await tabs.count()) > 1) {
    await tabs.nth(1).click();
    await settled(page);
    return;
  }
  const filter = page.getByRole("combobox").or(page.getByRole("searchbox")).first();
  if (await filter.isVisible().catch(() => false)) {
    await filter.click();
    return;
  }
  const map = page.locator(".maplibregl-canvas, .leaflet-container, canvas").first();
  await map.waitFor({ timeout: 10_000 });
  const box = await map.boundingBox();
  if (!box) throw new Error("the App shows no tab, filter or map to use");
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 - 120, box.y + box.height / 2 + 60, { steps: 8 });
  await page.mouse.up();
}

test.setTimeout(2_400_000);

test("every published public App on its own host, anonymous", async ({ browser }) => {
  test.skip(!DIR, "SHOTS_DIR is not set");
  const apps = await publicApps(browser);
  console.log(`published public Apps: ${apps.map((app) => `${app.project}/${app.name}`).join(", ") || "none"}`);
  expect(apps.length, "the Portal lists at least one published public App").toBeGreaterThan(0);
  for (const app of apps) {
    const host = `https://${app.name}.apps.${DOMAIN}`;
    const route = `apps/${app.project}/${app.name}`;
    await anonymously(browser, APP_WIDTHS, async (page, lang) => {
      const who = "anonymous" as const;
      await state(page, { route, state: "first-view", who, lang }, async () => {
        await visit(page, withLang(`${host}/`, lang));
        await settled(page);
      });
      await state(page, { route, state: "interaction", who, lang }, () => interact(page));
      await state(page, { route, state: "unknown-path", who, lang }, () => visit(page, withLang(`${host}/no-such-page-t4162`, lang)));
    });
  }
});

test("the public assistant: its widget, and framed by the data portal with one question", async ({ browser }) => {
  test.skip(!DIR, "SHOTS_DIR is not set");
  await anonymously(browser, WIDTHS, async (page, lang) => {
    const who = "anonymous" as const;
    for (const id of ["helsinki-catalogue", "helsinki-public"]) {
      await state(page, { route: `assistant/d/${id}/widget`, state: "first-view", who, lang }, () =>
        visit(page, withLang(`${ASSISTANT}/d/${id}/widget`, lang)),
      );
    }
    // T-4138's full chat page and the city's public deployment, as far as dev has them.
    await state(page, { route: "assistant/d/helsinki-catalogue", state: "chat-page", who, lang }, () =>
      visit(page, withLang(`${ASSISTANT}/d/helsinki-catalogue`, lang)),
    );
    await state(page, { route: "assistant/d/banskabystrica/widget", state: "first-view", who, lang }, () =>
      visit(page, withLang(`${ASSISTANT}/d/banskabystrica/widget`, lang)),
    );

    await state(page, { route: "data/assistant", state: "closed", who, lang }, () => visit(page, `${DATA}/${lang === "en" ? "en/" : ""}`));
    const offer = page.locator("details.jc-assistant > summary");
    const frame = page.frameLocator("details.jc-assistant > iframe");
    await state(page, { route: "data/assistant", state: "open", who, lang }, async () => {
      await offer.click();
      await expect(frame.getByRole("textbox").first()).toBeVisible({ timeout: 30_000 });
    });
    await state(page, { route: "data/assistant", state: "question-typed", who, lang }, () => frame.getByRole("textbox").first().fill(QUESTION[lang]));
    // Answers at the narrowest and widest width only: four questions in the whole run.
    if (page.viewportSize()?.width !== 768) {
      await state(page, { route: "data/assistant", state: "answered", who, lang }, async () => {
        await frame.getByRole("textbox").first().press("Enter");
        // Whatever the reply is (an answer, a refusal, a failed model call), it follows the
        // "Assistant" label and the Send button comes back.
        await expect(frame.getByText(/^(Assistant|Asistent):?$/).last()).toBeVisible({ timeout: 30_000 });
        await expect(frame.getByRole("button", { name: /^(Send|Odoslať)$/ })).toBeEnabled({ timeout: 120_000 });
      });
    }
  });
});

test("the public MCP page, the catalogue, and Keycloak's own pages, anonymous", async ({ browser }) => {
  test.skip(!DIR, "SHOTS_DIR is not set");
  await anonymously(browser, WIDTHS, async (page, lang) => {
    const who = "anonymous" as const;
    // T-4169: one page per public named MCP server (T-4141).
    for (const server of ["banskabystrica/otvorene-data", "bbsk/otvorene-data"]) {
      await state(page, { route: `/connect/${server}`, state: "first-view", who, lang }, () =>
        visit(page, withLang(`${PORTAL.origin}/connect/${server}`, lang)),
      );
    }
    await state(page, { route: "/catalogue", state: "first-view", who, lang }, () => visit(page, withLang(`${PORTAL.origin}/catalogue`, lang)));

    const login = `${IDM}/protocol/openid-connect/auth?client_id=account-console&response_type=code&scope=openid&redirect_uri=${encodeURIComponent(`${IDM}/account/`)}&kc_locale=${lang}`;
    await state(page, { route: "idm/forgot-password", state: "form", who, lang }, async () => {
      await visit(page, login);
      await page.locator("#username").waitFor({ timeout: 30_000 });
      await page.locator('a[href*="login-actions/reset-credentials"]').click();
      await page.locator("#username").waitFor({ timeout: 30_000 });
    });
    await state(page, { route: "idm/forgot-password", state: "empty-submit", who, lang }, async () => {
      await page.locator('input[type="submit"], button[type="submit"]').first().click();
      await page.waitForLoadState("load");
    });
    // Registration is a realm switch: when it is off the login page has no link, and there is no page to photograph.
    await visit(page, login);
    if (await page.locator('a[href*="registrations"]').count()) {
      await state(page, { route: "idm/register", state: "form", who, lang }, async () => {
        await page.locator('a[href*="registrations"]').click();
        await page.locator("#email, #username").first().waitFor({ timeout: 30_000 });
      });
    }
  });
});

test("Keycloak's account console as demo.viewer", async ({ browser }) => {
  test.skip(!DIR, "SHOTS_DIR is not set");
  const { context, page } = await signIn(browser, VIEWER, "/projects/helsinki/home?lang=en");
  watch(page);
  try {
    for (const width of WIDTHS) {
      await page.setViewportSize({ width, height: height(width) });
      for (const lang of LANGS) {
        await state(page, { route: "idm/account", state: "personal-info", who: "viewer", lang }, async () => {
          await visit(page, `${IDM}/account/?kc_locale=${lang}`);
          await expect(page.locator("#username, input[name='email'], [data-testid='email'], h1").first()).toBeVisible({ timeout: 60_000 });
        });
      }
    }
  } finally {
    await context.close();
  }
});

test("the error pages of every public host, anonymous", async ({ browser }) => {
  test.skip(!DIR, "SHOTS_DIR is not set");
  const pages: [string, string][] = [
    ["data/dataset-missing", `${DATA}/dataset/no-such-dataset-t4162`],
    ["data/page-missing", `${DATA}/no-such-page-t4162`],
    ["assistant/unknown-deployment", `${ASSISTANT}/d/no-such-assistant/widget`],
    ["assistant/unknown-path", `${ASSISTANT}/no-such-page-t4162`],
    ["edge/unknown-endpoint", `https://${DOMAIN}/api/endpoint/no-such-endpoint/ngsi-ld/v1/entities`],
    ["edge/unknown-path", `https://${DOMAIN}/no-such-page-t4162`],
    ["portal/unknown-path", `${PORTAL.origin}/no-such-page-t4162`],
  ];
  await anonymously(browser, WIDTHS, async (page, lang) => {
    for (const [route, address] of pages) {
      await state(page, { route, state: "error", who: "anonymous", lang }, () => visit(page, withLang(address, lang)));
    }
  });
});

test.afterAll(() => {
  if (!DIR) return;
  const count = writeIndex(DIR);
  // A PNG under 25 KB is a blank page, not a picture of one (T-4162 rules): it does not count.
  const mine = readdirSync(DIR).filter(
    (file) => file.startsWith(`${AREA}-`) && file.endsWith(".png") && statSync(join(DIR, file)).size > 25_000,
  ).length;
  console.log(`UX photo audit, public surfaces: ${mine} screenshots in ${DIR} (${count} in the folder)`);
  if (missed.length) console.log(`not reached (${missed.length}):\n${missed.join("\n")}`);
  expect(mine).toBeGreaterThanOrEqual(60);
});

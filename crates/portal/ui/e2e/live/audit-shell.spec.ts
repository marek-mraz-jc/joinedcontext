/**
 * T-4157 — UX photo audit 1/7: the shell around every page. Home, the navigation (open, and the
 * drawer on a phone), the project switcher, global search with and without results, the inbox,
 * activity, what's new, the profile menu, the 404 page, the 403 page (the viewer on a
 * steward page) and Keycloak's login page with the error a wrong password gets.
 *
 * The jobs menu is not photographed: the Portal renders it only once the person has started a
 * job, and starting one is a write.
 *
 * Every state at 375, 768 and 1440 px, in sk and en, as demo.steward and demo.viewer, and the
 * login as an anonymous visitor, saved by `shot` (shoot.ts, T-4156) under `$SHOTS_DIR`, which
 * then gets its contact sheet. Read-only: menus and dialogs are opened and closed with Escape,
 * search is typed and never submitted, and the wrong password belongs to a user that does not
 * exist. The count printed at the end is read from the folder.
 *
 *   SHOTS_DIR=/workspace/evidence/<agent>/T-4157 PORTAL_PASSWORD=… VIEWER_PASSWORD=… \
 *     npx playwright test --config playwright.live.config.ts audit-shell
 */
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { STEWARD, VIEWER, portalReady, signIn } from "./portal";
import { shot, writeIndex } from "./shoot";
import type { Shot } from "./shoot";

const here = dirname(fileURLToPath(import.meta.url));
const LANGS = ["sk", "en"] as const;
type Lang = (typeof LANGS)[number];
const WIDTHS = [375, 768, 1440];
const PROJECT = "helsinki";
const P = `/projects/${PROJECT}`;
const AREA = "shell";
const DIR = process.env.SHOTS_DIR;

const locales = Object.fromEntries(
  LANGS.map((lang) => [lang, JSON.parse(readFileSync(join(here, `../../src/locales/${lang}.json`), "utf8")) as Record<string, unknown>]),
) as Record<Lang, Record<string, unknown>>;

/** A label as the Portal renders it in `lang`, from the same locale file. */
function words(lang: Lang, key: string): string {
  const value = key.split(".").reduce<unknown>((node, part) => (node as Record<string, unknown>)[part], locales[lang]);
  if (typeof value !== "string") throw new Error(`no ${lang} text at ${key}`);
  return value;
}

const startsWith = (text: string) => new RegExp(`^${text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`);

/** States that could not be reached, with the reason; printed at the end, and the run fails on any. */
const missed: string[] = [];

async function state(page: Page, what: Omit<Shot, "width">, reach: () => Promise<void>): Promise<void> {
  try {
    await reach();
    // Fonts, icons and lazy panels settle before the picture: nothing busy, then a beat.
    await expect(page.locator('[aria-busy="true"]')).toHaveCount(0, { timeout: 15_000 }).catch(() => undefined);
    await page.waitForTimeout(1200);
    await shot(page, what);
  } catch (error) {
    missed.push(`${what.route} ${what.state} ${what.who} ${what.lang} ${page.viewportSize()?.width}: ${String(error).split("\n")[0]}`);
  } finally {
    await page.keyboard.press("Escape").catch(() => undefined);
    await page.keyboard.press("Escape").catch(() => undefined);
  }
}

async function open(page: Page, address: string, lang: Lang): Promise<void> {
  await page.goto(`${address}${address.includes("?") ? "&" : "?"}lang=${lang}`, { waitUntil: "load" });
  await expect(portalReady(page).or(page.locator("main h1")).first()).toBeVisible({ timeout: 60_000 });
  await page.locator("main h1:visible").first().waitFor({ timeout: 30_000 }).catch(() => undefined);
}

async function walkShell(page: Page, who: "steward" | "viewer", lang: Lang): Promise<void> {
  const base = { area: AREA, who, lang };

  await state(page, { ...base, route: `${P}/home`, state: "first-run" }, () => open(page, `${P}/home`, lang));

  // The navigation: a drawer behind the menu button where the screen is narrow, open beside the
  // page where it is wide.
  await state(page, { ...base, route: `${P}/home`, state: "navigation-open" }, async () => {
    await open(page, `${P}/home`, lang);
    const menu = page.locator('button[aria-controls="portal-sidebar"]');
    if (await menu.isVisible()) {
      await menu.click();
      await expect(menu).toHaveAttribute("aria-expanded", "true");
    }
    await expect(page.locator("#portal-sidebar")).toBeVisible();
  });

  await state(page, { ...base, route: `${P}/home`, state: "project-switcher" }, async () => {
    await open(page, `${P}/home`, lang);
    const menu = page.locator('button[aria-controls="portal-sidebar"]');
    if (await menu.isVisible()) await menu.click();
    await page.getByRole("button", { name: words(lang, "nav.projects") }).first().click();
  });

  await state(page, { ...base, route: `${P}/home`, state: "profile-menu" }, async () => {
    await open(page, `${P}/home`, lang);
    // The header's account button is named "Signed in as <name>".
    await page.getByRole("button", { name: startsWith(words(lang, "auth.signedInAs").split("{")[0]) }).first().click();
    await expect(page.getByRole("menu")).toBeVisible();
  });

  const search = page.getByRole("combobox", { name: words(lang, "palette.search") });
  await state(page, { ...base, route: `${P}/home`, state: "search-empty" }, async () => {
    await open(page, `${P}/home`, lang);
    await page.keyboard.press("Control+k");
    await expect(search).toBeFocused();
  });
  await state(page, { ...base, route: `${P}/home`, state: "search-results" }, async () => {
    await open(page, `${P}/home`, lang);
    await page.keyboard.press("Control+k");
    await search.fill("air");
    await expect(page.getByRole("option").first()).toBeVisible({ timeout: 30_000 });
  });
  await state(page, { ...base, route: `${P}/home`, state: "search-no-results" }, async () => {
    await open(page, `${P}/home`, lang);
    await page.keyboard.press("Control+k");
    await search.fill("qzxwv-nothing-is-called-this");
    await page.waitForTimeout(1500);
  });

  await state(page, { ...base, route: `${P}/inbox`, state: "page" }, () => open(page, `${P}/inbox`, lang));
  await state(page, { ...base, route: `${P}/activity`, state: "page" }, () => open(page, `${P}/activity`, lang));
  await state(page, { ...base, route: `/projects/${PROJECT}/health`, state: "page" }, () => open(page, `/projects/${PROJECT}/health`, lang));

  // Opening the list marks it read in this browser's storage only; nothing reaches the Portal.
  await state(page, { ...base, route: `${P}/home`, state: "whats-new" }, async () => {
    await open(page, `${P}/home`, lang);
    await page.getByRole("button", { name: startsWith(words(lang, "help.title")) }).first().click();
    await page.getByRole("menuitem", { name: startsWith(words(lang, "whatsNew.menu")) }).click();
    await expect(page.getByRole("dialog", { name: words(lang, "whatsNew.title") })).toBeVisible();
  });

  await state(page, { ...base, route: "/no-such-page", state: "not-found" }, () => open(page, "/no-such-page-t4157", lang));
  if (who === "viewer") {
    await state(page, { ...base, route: `${P}/settings/danger`, state: "forbidden" }, () => open(page, `${P}/settings/danger`, lang));
    await state(page, { ...base, route: "/organization/people", state: "forbidden" }, () => open(page, "/organization/people", lang));
  }
}

test.describe.configure({ mode: "serial" });
test.setTimeout(1_800_000);

for (const [who, person] of [
  ["steward", STEWARD],
  ["viewer", VIEWER],
] as const) {
  test(`the shell as demo.${who}`, async ({ browser }) => {
    test.skip(!DIR, "SHOTS_DIR is not set");
    const { context, page } = await signIn(browser, person, `${P}/home?lang=en`);
    try {
      for (const width of WIDTHS) {
        await page.setViewportSize({ width, height: width < 768 ? 812 : 900 });
        for (const lang of LANGS) await walkShell(page, who, lang);
      }
    } finally {
      await context.close();
    }
  });
}

test("Keycloak's login page and a wrong password, anonymous", async ({ browser }) => {
  test.skip(!DIR, "SHOTS_DIR is not set");
  for (const width of WIDTHS) {
    for (const lang of LANGS) {
      const context = await browser.newContext({ viewport: { width, height: width < 768 ? 812 : 900 } });
      const page = await context.newPage();
      const base = { area: AREA, who: "anonymous" as const, lang };
      try {
        // Where an anonymous visitor lands: the Portal's own login page, or Keycloak's form when
        // the edge sends them there first.
        await page.goto(`${P}/home?lang=${lang}`, { waitUntil: "load" });
        const signInButton = page.getByRole("button", { name: words(lang, "auth.signIn") });
        await expect(page.locator("#username").or(signInButton).first()).toBeVisible({ timeout: 30_000 });
        // Keycloak's own submit button is also "Sign in": the Portal's page is the one without the form.
        if ((await signInButton.count()) && !(await page.locator("#username").count())) {
          await state(page, { ...base, route: "/login", state: "portal" }, async () => {
            await expect(signInButton.first()).toBeVisible();
          });
          await signInButton.first().click();
        }
        await state(page, { ...base, route: "idm/login", state: "form" }, async () => {
          await page.locator("#username").waitFor({ timeout: 30_000 });
          // Keycloak takes its language from `kc_locale`, the same switch its own menu sets.
          await page.goto(`${page.url()}&kc_locale=${lang}`, { waitUntil: "load" });
          await page.locator("#username").waitFor({ timeout: 30_000 });
        });
        await state(page, { ...base, route: "idm/login", state: "wrong-password" }, async () => {
          // A user that does not exist: no real account is locked or counted.
          await page.fill("#username", "audit.nobody@example.invalid");
          await page.fill("#password", "not-a-password-t4157");
          await Promise.all([page.waitForURL(() => true, { waitUntil: "load" }), page.click("#kc-login")]);
          await page.locator("#username").waitFor({ timeout: 30_000 });
        });
      } finally {
        await context.close();
      }
    }
  }
});

test.afterAll(() => {
  if (!DIR) return;
  const count = writeIndex(DIR);
  const shell = readdirSync(DIR).filter((file) => file.startsWith(`${AREA}-`) && file.endsWith(".png")).length;
  console.log(`UX photo audit, shell: ${shell} screenshots in ${DIR} (${count} in the folder)`);
  if (missed.length) console.log(`not reached (${missed.length}):\n${missed.join("\n")}`);
  expect(missed, "every state of the shell is reached").toEqual([]);
  expect(shell).toBeGreaterThanOrEqual(60);
});

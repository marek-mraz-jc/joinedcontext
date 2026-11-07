/**
 * The User Guide's screenshots, taken by the live journeys themselves so a guide never shows a
 * Portal that no longer exists (T-3270). With `GUIDE_SHOTS=1` a journey's `guideShot(page, name)`
 * writes `test-results/guide/en/{name}.png` and `test-results/guide/sk/{name}.png` of the page as
 * it stands, at 1280 px; without it the call does nothing, so the journeys cost the same as before.
 * `scripts/publish-guide-shots.sh` quantizes the results into the docs repository's
 * `User-Guide/img/`, where `scripts/check-guide-shots.py` holds the guides to the shots that exist.
 */
import { copyFileSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";

/** What a guide may call a shot: lower-case words and digits joined by hyphens. */
const SHOT_NAME = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

/** The guide's two languages, in the order they are shot (T-3270). */
export const GUIDE_LANGS = ["en", "sk"] as const;
export type GuideLang = (typeof GUIDE_LANGS)[number];

/** The width every guide shot is taken at, so the guides show one Portal at one size. */
export const GUIDE_WIDTH = 1280;
const GUIDE_HEIGHT = 800;

/** Where a shot is written, relative to the run's directory; a name that is not a shot name throws. */
export function guideShotPath(lang: GuideLang, name: string, root = "test-results/guide"): string {
  if (!SHOT_NAME.test(name)) throw new Error(`guide shot name ${JSON.stringify(name)} is not lower-case-words-with-hyphens`);
  return join(root, lang, `${name}.png`);
}

/** Whether this run takes guide shots at all. */
export function guideShotsOn(env: NodeJS.ProcessEnv = process.env): boolean {
  return env.GUIDE_SHOTS === "1";
}

/** The language menu's words in a locale, read as the walker reads the bundles (no JSON import in Playwright's loader). */
function langWords(lang: GuideLang): { label: string; en: string; sk: string } {
  const here = dirname(fileURLToPath(import.meta.url));
  return (JSON.parse(readFileSync(join(here, `../../src/locales/${lang}.json`), "utf8")) as { lang: { label: string; en: string; sk: string } }).lang;
}

const LABELS: Record<GuideLang, { label: string; name: string }> = {
  en: { label: langWords("en").label, name: langWords("en").en },
  sk: { label: langWords("sk").label, name: langWords("sk").sk },
};

/**
 * Switches the Portal's language through its own header menu, as a person would; `false`, with the
 * menu closed again, when the installation does not offer `to` (dev speaks English only).
 */
async function switchTo(page: Page, from: GuideLang, to: GuideLang): Promise<boolean> {
  await page.getByRole("button", { name: LABELS[from].label }).first().click();
  const item = page.getByRole("menuitem", { name: LABELS[to].name });
  if (!(await item.count())) {
    await page.keyboard.press("Escape");
    return false;
  }
  await item.click();
  await page.waitForFunction((lang) => document.documentElement.lang === lang, to);
  return true;
}

/** The page's address in `lang`, through the `?lang=` the Portal's language detector reads. */
async function reloadIn(page: Page, lang: GuideLang): Promise<void> {
  const url = new URL(page.url());
  url.searchParams.set("lang", lang);
  await page.goto(url.toString(), { waitUntil: "load" });
  // Loaded means in the language, with the page's heading and no skeleton still pulsing.
  await page.waitForFunction(
    (want) =>
      document.documentElement.lang === want &&
      document.querySelector("main h1") !== null &&
      document.querySelector('[class*="animate-pulse"]') === null,
    lang,
    { timeout: 60_000 },
  );
}

/** How a shot reaches Slovak when the header's menu does not offer it. */
export interface GuideShotOptions {
  /**
   * The page shows the same after a reload (a list, a saved change's page): the Slovak shot is
   * taken after a reload with `?lang=sk`. Left out, an unsaved form would come back empty, so the
   * Slovak guide shows the English shot until the installation offers Slovak (T-3270).
   */
  reloads?: boolean;
  root?: string;
}

/**
 * Shoots the page as it stands, in English and in Slovak, for the guide step `name`. A password
 * field is masked, so no shot can carry one. The page is left in English at the size it had.
 * The language is switched through the header's menu, so a shot is taken with no dialog open:
 * a modal dialog hides the header from the menu's click.
 */
export async function guideShot(page: Page, name: string, options: GuideShotOptions = {}): Promise<void> {
  if (!guideShotsOn()) return;
  const root = options.root ?? "test-results/guide";
  const lang = await page.evaluate(() => document.documentElement.lang);
  if (lang !== "en") throw new Error(`guide shot ${name}: the page is in ${JSON.stringify(lang)}; journeys shoot from English (?lang=en)`);
  if (await page.locator("[role=dialog][aria-modal=true]").count()) {
    throw new Error(`guide shot ${name}: a dialog is open; shoot the page before or after it`);
  }
  const size = page.viewportSize();
  await page.setViewportSize({ width: GUIDE_WIDTH, height: GUIDE_HEIGHT });
  const shoot = async (current: GuideLang) => {
    const path = guideShotPath(current, name, root);
    mkdirSync(join(path, ".."), { recursive: true });
    await page.screenshot({ path, animations: "disabled", caret: "hide", mask: [page.locator("input[type=password]")] });
    return path;
  };
  try {
    const english = await shoot("en");
    if (await switchTo(page, "en", "sk")) {
      await shoot("sk");
      await switchTo(page, "sk", "en");
    } else if (options.reloads) {
      await reloadIn(page, "sk");
      await shoot("sk");
      await reloadIn(page, "en");
    } else {
      const slovak = guideShotPath("sk", name, root);
      mkdirSync(join(slovak, ".."), { recursive: true });
      copyFileSync(english, slovak);
    }
  } finally {
    if (size) await page.setViewportSize(size);
  }
}

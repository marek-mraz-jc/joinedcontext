import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { axeViolations } from "../axe";
import { STEWARD, VIEWER, signIn } from "./portal";

const PROJECT = "helsinki";
const REPORT = process.env.FORMS_REPORT ?? "test-results/forms-checklist.json";
const HERE = dirname(fileURLToPath(import.meta.url));
const ALLOWED = join(HERE, "forms-checklist.allowed.json");
/** The three languages besides English the Portal ships, walked for overflow and leftovers (T-1600). */
const LOCALES = ["sk", "cs", "de"] as const;
type Lang = "en" | (typeof LOCALES)[number];

interface Finding {
  page: string;
  check: string;
  detail: string;
}

/**
 * What each page is still allowed to fail, per rule: how many of that finding stand today, and
 * the task that removes them. Measured on dev, and it only ever shrinks — the second case below
 * fails when a page is cleaner than its entry, so an entry cannot outlive the defect it names.
 */
type Allowed = Record<string, Record<string, { findings: number; task: string }>>;

// Empty on purpose until the live lane can run: the demo people are absent from the `dev`
// realm, so nothing has been measured yet (T-2230). Empty means every finding fails, which is
// the end state; the first successful run against dev fills it, one entry per page and rule.
const allowed = JSON.parse(readFileSync(ALLOWED, "utf8")) as Allowed;

/** The findings the walks collected, read by the last test (`workers: 1`, serial). */
let collected: Finding[] | null = null;
/** The pages the first walk found, walked again in each language. */
let walked: string[] = [];

/** Every finding beyond what the allow-list grants that page and rule, as a line to read. */
function unexpected(findings: Finding[]): string[] {
  const seen = new Map<string, number>();
  return findings.flatMap((finding) => {
    const key = `${finding.page}\u0000${finding.check}`;
    const nth = (seen.get(key) ?? 0) + 1;
    seen.set(key, nth);
    const budget = allowed[finding.page]?.[finding.check]?.findings ?? 0;
    return nth > budget ? [`${finding.page} — ${finding.check}: ${finding.detail || "(no detail)"}`] : [];
  });
}

/**
 * TS-19, UI-44: the form checklist of goal.md, "Forms people trust", measured on dev
 * (UI-44, UI-45, UI-61) — and now held.
 *
 * It was a survey that asserted nothing, so a regression in any create form was written to a
 * file nobody reads while the lane stayed green (T-2230). The walk is unchanged; what is new is
 * that every finding it makes has to be one `forms-checklist.allowed.json` already names, with
 * the task that closes it. Nothing is submitted with valid data, so nothing is proposed: the
 * only button it clicks inside a form is Check, which is a dry run.
 */
test.describe.serial("the create forms of a project", () => {
test("every create form, against the checklist", async ({ browser }) => {
  test.setTimeout(1_200_000);
  const findings: Finding[] = [];
  const { page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);
  const pages = await projectPages(page);

  for (const path of pages) {
    await page.setViewportSize({ width: 1600, height: 1000 });
    await page.goto(`${path}?lang=en`, { waitUntil: "load" });
    const note = (check: string, detail: string) => findings.push({ page: path, check, detail });

    for (const violation of await axeViolations(page)) note("axe on the page", violation);
    const opener = page.getByRole("main").getByRole("button", { name: /^New / }).first();
    if (!(await opener.count())) continue;
    if (await opener.isDisabled()) {
      note("create is disabled for a steward", (await opener.getAttribute("title")) ?? "no reason given");
      continue;
    }
    await opener.click();
    const dialog = page.getByTestId("form-page");
    if (!(await dialog.isVisible().catch(() => false))) {
      note("New opens no form", (await opener.innerText()).trim());
      continue;
    }

    await surfaceChecks(page, dialog, note, opener, "the form");

    // An empty submit: the form says what is missing, beside the field, in words.
    // Only a check is ever clicked. A form that proposes on its first button would open a real
    // Change from its placeholder values; the New role form did exactly that (T-1492).
    await opener.click();
    if (await dialog.isVisible().catch(() => false)) {
      await emptyCheck(page, dialog, note);
      await page.keyboard.press("Escape");
      await page.waitForTimeout(400);
    }
  }

  // UI-44: the first row's Edit and Remove, on every page — the pages without a New button
  // included, which the walk above could only run axe on. Neither is ever confirmed.
  for (const path of pages) {
    const note = (check: string, detail: string) => findings.push({ page: path, check, detail });
    await rowDialogs(page, path, "en", note);
  }

  // A viewer sees why, never a dead button.
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/spaces?lang=en`);
  for (const path of pages) {
    await viewer.page.goto(`${path}?lang=en`, { waitUntil: "load" });
    const opener = viewer.page.getByRole("main").getByRole("button", { name: /^New / }).first();
    if (!(await opener.count())) continue;
    const disabled = (await opener.isDisabled()) || (await opener.getAttribute("aria-disabled")) === "true";
    const reason = (await opener.getAttribute("title")) ?? (await opener.getAttribute("aria-describedby"));
    if (!disabled) findings.push({ page: path, check: "a viewer is offered New", detail: "enabled" });
    else if (!reason) findings.push({ page: path, check: "a viewer's disabled New gives no reason", detail: "" });
  }

  writeFileSync(REPORT, JSON.stringify({ pages, findings }, null, 2));
  console.log(`forms checklist: ${pages.length} pages, ${findings.length} findings -> ${REPORT}`);
  collected = findings;
  walked = pages;

  // The survey keeps discovering; the allow-list is what stops a new finding hiding in it.
  expect(
    unexpected(findings),
    "a create form broke a checklist rule that forms-checklist.allowed.json does not name",
  ).toEqual([]);
});

// UI-45: every form in Slovak, Czech and German: nothing overflows at 400 px and no English string
// the language translates is left on the page, the create form, or the first row's dialogs. A
// finding is filed under the page with its language, `/projects/helsinki/spaces [sk]`.
test("every form in the other three languages", async ({ browser }) => {
  test.setTimeout(1_800_000);
  expect(walked.length, "the English walk found no pages, so there is nothing to translate").toBeGreaterThan(0);
  const findings: Finding[] = [];
  const { page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);
  for (const lang of LOCALES) {
    for (const path of walked) {
      const note = (check: string, detail: string) => findings.push({ page: `${path} [${lang}]`, check, detail });
      await page.setViewportSize({ width: 1600, height: 1000 });
      await page.goto(`${path}?lang=${lang}`, { waitUntil: "load" });
      await leftovers(page.getByRole("main"), lang, "the page", note);

      // The create form by its address, so no localised button name has to be guessed (T-2474).
      await page.goto(`${path}/new?lang=${lang}`, { waitUntil: "load" });
      const form = page.getByTestId("form-page");
      if (await form.isVisible().catch(() => false)) {
        await leftovers(form, lang, "the create form", note);
        await overflow(page, form, "the create form", note);
      }
      await rowDialogs(page, path, lang, note);
    }
  }
  writeFileSync(REPORT, JSON.stringify({ pages: walked, findings: [...(collected ?? []), ...findings] }, null, 2));
  console.log(`forms checklist, ${LOCALES.join("/")}: ${findings.length} findings -> ${REPORT}`);
  collected = [...(collected ?? []), ...findings];
  expect(
    unexpected(findings),
    "a translated form broke a checklist rule that forms-checklist.allowed.json does not name",
  ).toEqual([]);
});

test("the allow-list names no page that is already clean", () => {
  expect(collected, "the walk above did not finish, so there is nothing to compare").not.toBeNull();
  const counted = new Map<string, number>();
  for (const finding of collected ?? []) {
    const key = `${finding.page}\u0000${finding.check}`;
    counted.set(key, (counted.get(key) ?? 0) + 1);
  }
  const stale = Object.entries(allowed).flatMap(([page, rules]) =>
    Object.entries(rules).flatMap(([check, entry]) => {
      const now = counted.get(`${page}\u0000${check}`) ?? 0;
      return now < entry.findings
        ? [`${page} — ${check}: allows ${entry.findings}, found ${now} (${entry.task})`]
        : [];
    }),
  );
  expect(stale, "lower or delete these entries; the allow-list only shrinks").toEqual([]);
});
});

/** Every page of the project the navigation links to. */
async function projectPages(page: Page): Promise<string[]> {
  const hrefs = await page
    .getByRole("navigation", { name: "Main navigation" })
    .locator(`a[href^="/projects/${PROJECT}/"]`)
    .evaluateAll((links) => links.map((a) => new URL((a as HTMLAnchorElement).href).pathname));
  return [...new Set(hrefs)];
}

/** Inputs, selects and text areas of the form with no accessible name. */
async function unnamedFields(dialog: Locator): Promise<string[]> {
  return dialog.locator("input:not([type=hidden]), select, textarea").evaluateAll((fields) =>
    fields
      .filter((field) => {
        const el = field as HTMLInputElement;
        const labelled = (el.labels?.length ?? 0) > 0 || el.getAttribute("aria-label") || el.getAttribute("aria-labelledby");
        return !labelled && el.offsetParent !== null;
      })
      .map((field) => `${field.tagName.toLowerCase()}[name=${field.getAttribute("name") ?? ""}][id=${field.id}]`),
  );
}

type Note = (check: string, detail: string) => void;

/** One locale file, flattened to `key -> text`. */
function strings(lang: Lang): Map<string, string> {
  const flat = new Map<string, string>();
  const walk = (node: unknown, prefix: string) => {
    if (typeof node === "string") flat.set(prefix, node);
    else if (node && typeof node === "object") {
      for (const [key, value] of Object.entries(node)) walk(value, prefix ? `${prefix}.${key}` : key);
    }
  };
  walk(JSON.parse(readFileSync(join(HERE, `../../src/locales/${lang}.json`), "utf8")), "");
  return flat;
}

const english = strings("en");
const translated = Object.fromEntries(LOCALES.map((lang) => [lang, strings(lang)])) as Record<
  (typeof LOCALES)[number],
  Map<string, string>
>;

/** A string of the locale files, by key, in the language asked for. */
function say(lang: Lang, key: string): string {
  const text = (lang === "en" ? english : translated[lang]).get(key);
  if (text === undefined) throw new Error(`no ${key} in ${lang}.json`);
  return text;
}

/**
 * The English sentences a language translates differently, without placeholders: a line of the
 * page that is exactly one of them is a string the page did not translate. Words that stay the same
 * in both (a product name, "Name") are left out, so they are never read as a leftover.
 */
const leftoverLines = Object.fromEntries(
  LOCALES.map((lang) => [
    lang,
    new Set(
      [...english].flatMap(([key, text]) => {
        const other = translated[lang].get(key);
        return other !== undefined && other !== text && !text.includes("{") && text.includes(" ")
          ? [text.trim()]
          : [];
      }),
    ),
  ]),
) as Record<(typeof LOCALES)[number], Set<string>>;

/** Lines of `where` left in English, and translation keys shown raw instead of their text. */
async function leftovers(where: Locator, lang: Lang, what: string, note: Note): Promise<void> {
  if (lang === "en") return;
  const lines = (await where.innerText().catch(() => ""))
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  for (const line of new Set(lines)) {
    if (leftoverLines[lang].has(line)) note(`${what} shows an untranslated string`, line);
    else if (english.has(line) && /^[a-z][A-Za-z]*(\.[A-Za-z0-9]+)+$/.test(line)) note(`${what} shows a raw translation key`, line);
  }
}

/** How far `surface` scrolls sideways at a phone's width, noted when it does at all. */
async function overflow(page: Page, surface: Locator, what: string, note: Note): Promise<void> {
  await page.setViewportSize({ width: 400, height: 900 });
  await page.waitForTimeout(300);
  const wide = await surface.evaluate((el) => el.scrollWidth - el.clientWidth).catch(() => 0);
  if (wide > 1) note(`${what} overflows sideways at 400 px`, `${wide}px`);
  await page.setViewportSize({ width: 1600, height: 1000 });
}

/**
 * The checks every open form or dialog is held to: axe, a name on every field, focus moved in,
 * no sideways scroll at 400 px, Escape closes it and gives focus back to what opened it.
 */
async function surfaceChecks(page: Page, surface: Locator, note: Note, opener: Locator, what: string): Promise<void> {
  for (const violation of await axeViolations(page)) note(`axe with ${what} open`, violation);
  for (const name of await unnamedFields(surface)) note("field without an accessible name", name);
  if (!(await surface.evaluate((el) => el.contains(document.activeElement)))) {
    note(`focus is not moved into ${what}`, "");
  }
  await overflow(page, surface, what, note);
  await page.keyboard.press("Escape");
  await page.waitForTimeout(400);
  if (await surface.isVisible().catch(() => false)) note(`Escape does not close ${what}`, "");
  else if (!(await opener.evaluate((el) => el === document.activeElement).catch(() => false))) {
    note(`focus does not return to the button that opened ${what}`, "");
  }
}

/** The empty-submit rule: Check on an empty form says what is missing, beside the field, in words. */
async function emptyCheck(page: Page, dialog: Locator, note: Note): Promise<void> {
  const submit = dialog.getByRole("button", { name: /^Check/ }).first();
  if (!(await submit.count())) {
    note("the form has no Check step before it proposes", (await dialog.getByRole("button").allInnerTexts()).join(", "));
    return;
  }
  if (await submit.isDisabled()) {
    const why = (await submit.getAttribute("title")) ?? (await submit.getAttribute("aria-describedby"));
    if (!why) note("submit is disabled on an empty form without saying why", (await submit.innerText()).trim());
    return;
  }
  await submit.click();
  await page.waitForTimeout(1500);
  if (!(await dialog.isVisible().catch(() => false))) {
    note("an empty form was accepted", "the dialog closed");
    return;
  }
  const invalid = await dialog.locator("[aria-invalid=true]").count();
  const said = (await dialog.locator("[role=alert], [role=status], [aria-live]").allInnerTexts()).join(" | ");
  if (invalid === 0 && said.trim() === "") note("an empty submit says nothing", "");
  if (invalid === 0 && said.trim() !== "") note("the error is not tied to a field (no aria-invalid)", said.slice(0, 200));
  if (/\b[45]\d\d\b|^\s*[{[]|bad request|unprocessable/i.test(said)) note("the error is a status code or raw JSON", said.slice(0, 200));
}

/** A regular expression that matches `text` literally. */
function literal(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * The first row's Edit and Remove of a list page, opened from the row's menu, checked and closed.
 * Nothing is typed and nothing is proposed: Edit is left with Escape, and Remove's confirmation
 * stays disabled because the name is never typed back (T-1600, UI-44). In a language other than
 * English only what translation breaks is checked: overflow and leftover English.
 */
async function rowDialogs(page: Page, path: string, lang: Lang, note: Note): Promise<void> {
  const more = new RegExp(`^${literal(say(lang, "rowActions.more").split("{name}")[0])}`);
  for (const [key, what] of [
    ["resourceEdit.button", "the edit form"],
    ["resourceDelete.button", "the remove dialog"],
  ] as const) {
    await page.setViewportSize({ width: 1600, height: 1000 });
    await page.goto(`${path}?lang=${lang}`, { waitUntil: "load" });
    const menu = page.getByRole("main").getByRole("button", { name: more }).first();
    // A page with no rows, or rows without the manifest menu, has no row dialog to open.
    if (!(await menu.isVisible({ timeout: 10_000 }).catch(() => false))) return;
    await menu.click();
    const item = page.getByRole("menuitem", { name: new RegExp(`^${literal(say(lang, key))}`) }).first();
    if (!(await item.count())) {
      note(`the row menu has no ${say(lang, key)}`, (await page.getByRole("menuitem").allInnerTexts()).join(", "));
      await page.keyboard.press("Escape");
      continue;
    }
    if ((await item.getAttribute("aria-disabled")) === "true") {
      // A steward may edit and remove in the demo project; a disabled item has to say why.
      if (!(await item.getAttribute("title"))) note(`${what} is disabled for a steward without a reason`, "");
      else note(`${what} is disabled for a steward`, (await item.getAttribute("title")) ?? "");
      await page.keyboard.press("Escape");
      continue;
    }
    await item.click();
    const surface = page.locator("[data-testid=form-page], [role=dialog]").last();
    if (!(await surface.isVisible({ timeout: 10_000 }).catch(() => false))) {
      note(`${say(lang, key)} opens nothing`, "");
      continue;
    }
    if (key === "resourceDelete.button") {
      const propose = surface.getByRole("button", { name: say(lang, "resourceDelete.propose") });
      if ((await propose.count()) && !(await propose.isDisabled())) {
        note("removal can be proposed before the name is typed back", "");
      }
    }
    if (lang === "en") {
      await surfaceChecks(page, surface, note, menu, what);
    } else {
      await leftovers(surface, lang, what, note);
      await overflow(page, surface, what, note);
      await page.keyboard.press("Escape");
    }
  }
}

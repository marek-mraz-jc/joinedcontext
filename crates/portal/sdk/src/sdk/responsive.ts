// The four-width check of an application's screens (T-2777, T-2825, UI-84, SDK-12), for a
// Playwright spec: `layoutProblems(page, width)` lists what is wrong at the width the page has, and
// the spec asserts the list is empty. Nothing here imports `@playwright/test` at run time: a second
// copy of it (the SDK's, linked, beside the app's) is refused by Playwright, so the assertions stay
// in the app's own spec.
import type { Page } from "@playwright/test";

/** A phone, a tablet, a laptop and a wall. */
export const WIDTHS = [
  { width: 375, height: 812 },
  { width: 768, height: 1024 },
  { width: 1440, height: 900 },
  { width: 2560, height: 1440 },
] as const;

/** The blocks that must never cover each other: every view, card, field and header part. */
export const BLOCKS = [
  ".jc-card",
  ".jc-tile",
  ".jc-table-wrap",
  ".jc-map",
  ".jc-chart",
  ".jc-field",
  ".jc-filter",
  ".jc-detail",
  ".jc-page-header",
  ".jc-export",
  ".jc-form-actions",
  ".jc-tabs",
  ".jc-sidebar-toggle",
  ".jc-grid",
  ".jc-header h1",
  ".jc-header nav",
].join(", ");

/** What an application may be built from besides the template's classes. */
export const LIVE_BLOCKS = `${BLOCKS}, article, aside, figure, form, table`;

/**
 * Pairs of visible blocks, neither inside the other, whose boxes intersect by more than a pixel.
 * A block that is not drawn (the content of a closed `<details>`, `visibility: hidden`) is not
 * counted: Chromium still lays it out when asked for its box, wherever it would stand (T-2981).
 * Neither is one clipped to nothing, as text kept for a screen reader is: a chart's table of
 * values has the size of its rows, since a table never shrinks to its `width: 1px` (T-2827).
 */
export async function overlaps(page: Page, blocks: string = BLOCKS): Promise<string[]> {
  return page.evaluate((selector) => {
    const name = (el: Element) =>
      `${el.tagName.toLowerCase()}.${[...el.classList].join(".")} "${(el.textContent ?? "").trim().slice(0, 30)}"`;
    const nothing = /^rect\(\s*0(px)?,?\s+0(px)?,?\s+0(px)?,?\s+0(px)?\s*\)$/;
    const clippedAway = (el: Element) => {
      for (let at: Element | null = el; at; at = at.parentElement) {
        const style = getComputedStyle(at);
        if (style.position === "absolute" || style.position === "fixed") {
          if (nothing.test(style.clip)) return true;
        }
        if (style.clipPath === "inset(50%)") return true;
      }
      return false;
    };
    const boxes = [...document.querySelectorAll(selector)]
      .filter((el) => el.checkVisibility({ visibilityProperty: true }) && !clippedAway(el))
      .map((el) => ({ el, box: el.getBoundingClientRect() }))
      .filter(({ box }) => box.width > 0 && box.height > 0);
    const found: string[] = [];
    for (let i = 0; i < boxes.length; i++) {
      for (let j = i + 1; j < boxes.length; j++) {
        const a = boxes[i];
        const b = boxes[j];
        if (a.el.contains(b.el) || b.el.contains(a.el)) continue;
        const width = Math.min(a.box.right, b.box.right) - Math.max(a.box.left, b.box.left);
        const height = Math.min(a.box.bottom, b.box.bottom) - Math.max(a.box.top, b.box.top);
        if (width > 1 && height > 1) found.push(`${name(a.el)} × ${name(b.el)}`);
      }
    }
    return found;
  }, blocks);
}

/**
 * Controls a person cannot fully see or reach because a box that hides its overflow cuts them
 * off: a pager button pushed past the edge of a card, a save button wider than its panel. A
 * scrolling box is not counted, since its content is one scroll away.
 */
export async function clippedControls(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const controls = document.querySelectorAll("button, input, select, textarea, a[href], [role='button'], [tabindex]:not([tabindex='-1'])");
    const found: string[] = [];
    for (const control of controls) {
      const box = control.getBoundingClientRect();
      if (box.width === 0 || box.height === 0 || !control.checkVisibility({ visibilityProperty: true })) continue;
      for (let parent = control.parentElement; parent; parent = parent.parentElement) {
        const style = getComputedStyle(parent);
        const hides = ["hidden", "clip"];
        // Inside a box that scrolls, the control is reachable however far out it sits.
        if ([style.overflowX, style.overflowY].some((overflow) => overflow === "auto" || overflow === "scroll")) break;
        if (!hides.includes(style.overflowX) && !hides.includes(style.overflowY)) continue;
        const clip = parent.getBoundingClientRect();
        // A one-pixel box that hides its overflow is how text is kept for a screen reader only.
        if (clip.width <= 1 || clip.height <= 1) break;
        if (box.left < clip.left - 1 || box.right > clip.right + 1 || box.top < clip.top - 1 || box.bottom > clip.bottom + 1) {
          const name = (control.getAttribute("aria-label") ?? control.textContent ?? "").trim().slice(0, 30);
          found.push(`${control.tagName.toLowerCase()} "${name}" in ${parent.tagName.toLowerCase()}.${[...parent.classList].join(".")}`);
          break;
        }
      }
    }
    return found;
  });
}

/** Where axe-core sits, found from this module the way Node finds an import. */
function axePath(): string {
  try {
    const url = (import.meta as ImportMeta & { resolve: (specifier: string) => string }).resolve("axe-core/axe.min.js");
    return decodeURIComponent(new URL(url).pathname);
  } catch {
    throw new Error("the responsive check needs axe-core: add \"axe-core\": \"4.13.0\" to the app's devDependencies");
  }
}

let axeSource: string | undefined;

/**
 * axe-core's bundle as text, read once through a blank page of the same browser: it carries no
 * Content Security Policy, and `axe.source` is the bundle axe injects into frames itself. axe sets
 * it only where it sees a CommonJS `module`, so the blank page gets one first.
 */
async function loadAxe(page: Page): Promise<string> {
  if (axeSource !== undefined) return axeSource;
  const blank = await page.context().newPage();
  try {
    await blank.evaluate(() => {
      (window as unknown as { module: object }).module = { exports: {} };
    });
    await blank.addScriptTag({ path: axePath() });
    const source = await blank.evaluate(() => (window as unknown as { axe: { source?: string } }).axe.source);
    if (typeof source !== "string") throw new Error("axe-core did not expose its source (axe.source)");
    axeSource = source;
    return source;
  } finally {
    await blank.close();
  }
}

/** What axe finds at WCAG 2.1 A and AA, one line per rule with the elements it names. */
export async function axeViolations(page: Page): Promise<string[]> {
  // Through the protocol, not a <script> tag: a published App is served with the static host's
  // Content Security Policy, which forbids an inline script but not what the test driver evaluates.
  await page.evaluate(await loadAxe(page));
  return page.evaluate(async () => {
    const run = (window as unknown as {
      axe: { run: (context: Document, options: object) => Promise<{ violations: { id: string; nodes: { target: string[] }[] }[] }> };
    }).axe;
    const result = await run.run(document, { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"] } });
    return result.violations.map((violation) => `${violation.id}: ${violation.nodes.map((node) => node.target.join(" ")).join(", ")}`);
  });
}

/**
 * Everything wrong with the page at the width it has: a sideways scroll, a table that scrolls
 * sideways on a phone (it should be cards there), overlapping blocks, controls a box cuts off and
 * axe violations. Empty means the view passes.
 */
export async function layoutProblems(page: Page, blocks: string = BLOCKS): Promise<string[]> {
  const problems: string[] = [];
  const { sideways, width } = await page.evaluate(() => ({
    sideways: document.documentElement.scrollWidth - window.innerWidth,
    width: window.innerWidth,
  }));
  if (sideways > 0) problems.push(`the page scrolls sideways by ${sideways} px`);
  if (width < 600) {
    const scrolling = await page
      .locator(".jc-table-wrap")
      .evaluateAll((wraps) => wraps.filter((wrap) => wrap.scrollWidth > wrap.clientWidth + 1).length);
    if (scrolling > 0) problems.push(`${scrolling} table(s) scroll sideways on a phone`);
  }
  for (const pair of await overlaps(page, blocks)) problems.push(`overlap: ${pair}`);
  for (const control of await clippedControls(page)) problems.push(`cut off: ${control}`);
  for (const violation of await axeViolations(page)) problems.push(`axe ${violation}`);
  return problems;
}

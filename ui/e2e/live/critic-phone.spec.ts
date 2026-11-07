/**
 * T-5526 — UI critic 6/6: the data screens at phone width, with touch, on dev. The grid of the
 * explorer and of a space, one row's detail and its history, the access pages and the MQTT pages,
 * each at 400 x 860 and 360 x 740, as demo.editor and demo.viewer. Per screen it measures whether
 * the page scrolls sideways (document.scrollingElement.scrollWidth against the viewport) and the
 * smallest touch target, and fails on either: a page wider than the phone, or a control under
 * 44 x 44 px. Inline links inside running text are exempt, as in WCAG 2.5.8; a control drawn only
 * for the mouse (`aria-hidden`, `tabindex=-1`) still counts, because a finger finds it too.
 * An open dialog must fit the viewport and close with a visible button. Read-only.
 *
 *   EDITOR_PASSWORD=… VIEWER_PASSWORD=… SHOTS_DIR=… npx playwright test \
 *     --config playwright.live.config.ts critic-phone
 */
import { expect, test } from "@playwright/test";
import type { Browser, Page } from "@playwright/test";
import { EDITOR, VIEWER, goSignedIn, openToJudge } from "./portal";
import { secretMasks } from "./shoot";
import { smallTouchTargets, touchTargets } from "../touch";
import type { TouchTarget } from "../touch";

test.setTimeout(900_000);

const PROJECT = "helsinki";
const SIZES = [
  { width: 400, height: 860 },
  { width: 360, height: 740 },
];
const PEOPLE = [
  { who: "editor", person: EDITOR },
  { who: "viewer", person: VIEWER },
];
const FINGER = 44;

/** One screen's measurement: what the per-screen row of the critic's table holds. */
interface PhoneRow {
  screen: string;
  scrollWidth: number;
  viewport: number;
  smallest: { size: string; what: string } | null;
  small: string[];
  dialog: string[];
}

/** Measures the page as it stands. */
async function measure(page: Page, screen: string): Promise<PhoneRow> {
  const targets = await touchTargets(page);
  const smallest = targets.reduce<TouchTarget | null>((least, target) => (least === null || target.edge < least.edge ? target : least), null);
  const small = await smallTouchTargets(page, FINGER);
  const rest = await page.evaluate(() => {
    const viewport = innerWidth;
    const scroller = document.scrollingElement ?? document.documentElement;
    const dialog: string[] = [];
    // A screen the Portal could not draw is a finding, never a pass with nothing measured.
    if (/Part of the Portal did not load/.test(document.querySelector("main")?.textContent ?? "")) dialog.push("the page did not load");
    const open = document.querySelector("dialog[open]");
    if (open) {
      const box = open.getBoundingClientRect();
      if (box.width > viewport + 0.5) dialog.push(`dialog ${Math.round(box.width)} px wide in ${viewport}`);
      if (box.left < -0.5) dialog.push(`dialog starts ${Math.round(box.left)} px off screen`);
      const close = [...open.querySelectorAll("button")].find((button) => /close|zavrie/i.test(`${button.getAttribute("aria-label") ?? ""} ${button.textContent ?? ""}`));
      const closeBox = close?.getBoundingClientRect();
      if (!close || !closeBox || closeBox.right > viewport + 0.5 || closeBox.bottom < 0 || closeBox.top > innerHeight) dialog.push("no visible close button");
    }
    return { scrollWidth: scroller.scrollWidth, viewport, dialog };
  });
  return {
    screen,
    scrollWidth: rest.scrollWidth,
    viewport: rest.viewport,
    smallest: smallest ? { size: smallest.size, what: smallest.what } : null,
    small: small.slice(0, 25),
    dialog: rest.dialog,
  };
}

async function firstName(page: Page, plural: string): Promise<string | undefined> {
  const answer = await page.request.get(`/api/v1/projects/${PROJECT}/${plural}`);
  if (!answer.ok()) return undefined;
  return ((await answer.json()) as { items?: { metadata: { name: string } }[] }).items?.[0]?.metadata.name;
}

async function settle(page: Page): Promise<void> {
  await page.locator("main h1").first().waitFor({ timeout: 60_000 });
  // Never `networkidle`: the Portal keeps its activity and drafts streams open (portal.ts).
  await page.waitForTimeout(2500);
}

async function snap(page: Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  const body = await page.screenshot({ fullPage: !(await page.locator("dialog[open]").count()), mask: secretMasks(page) });
  await test.info().attach(`${name}.png`, { body, contentType: "image/png" });
  if (dir) {
    const { mkdirSync, writeFileSync } = await import("node:fs");
    mkdirSync(dir, { recursive: true });
    writeFileSync(`${dir}/${name}.png`, body);
  }
}

/** The screens, in the order a person reaches them; each returns false when it is not there. */
function screens(space: string | undefined): { name: string; open: (page: Page) => Promise<boolean> }[] {
  const go = (path: string) => async (page: Page) => {
    // The edge refuses a burst of chunk loads (429): openToJudge waits it out and the Portal's own reload.
    if (await openToJudge(page, `/${path}${path.includes("?") ? "&" : "?"}lang=en`)) return false;
    await settle(page);
    return true;
  };
  const rowOpen = async (page: Page) => {
    if (!space) return false;
    await go(`projects/${PROJECT}/spaces/${space}`)(page);
    const open = page.getByRole("button", { name: /^Open: / }).first();
    if (!(await open.waitFor({ timeout: 30_000 }).then(() => true, () => false))) return false;
    await open.click();
    await page.locator("dialog[open]").waitFor({ timeout: 15_000 });
    await page.waitForTimeout(800);
    return true;
  };
  return [
    { name: "explore", open: go(`projects/${PROJECT}/explore`) },
    ...(space
      ? [
          { name: "explore-space", open: go(`projects/${PROJECT}/explore?space=${encodeURIComponent(space)}`) },
          { name: "space-grid", open: go(`projects/${PROJECT}/spaces/${space}`) },
          { name: "row-detail", open: rowOpen },
          {
            name: "row-history",
            open: async (page: Page) => {
              if (!(await rowOpen(page))) return false;
              const tab = page.locator("dialog[open]").getByRole("tab", { name: /history/i });
              if (!(await tab.count())) return false;
              await tab.first().click();
              await page.waitForTimeout(2000);
              return true;
            },
          },
        ]
      : []),
    { name: "settings-access", open: go(`projects/${PROJECT}/settings/access`) },
    { name: "policies", open: go(`projects/${PROJECT}/policies`) },
    { name: "mqttbrokers", open: go(`projects/${PROJECT}/mqttbrokers`) },
    { name: "organization", open: go("organization") },
  ];
}

async function walk(browser: Browser, who: string, person: { user: string; password: string }, size: { width: number; height: number }) {
  const context = await browser.newContext({ viewport: size, hasTouch: true, isMobile: true, deviceScaleFactor: 2 });
  const page = await context.newPage();
  const rows: PhoneRow[] = [];
  try {
    // At phone width the main navigation sits behind the banner's Menu button.
    await goSignedIn(page, person, `/projects/${PROJECT}/spaces?lang=en`, (signedIn) => signedIn.getByRole("banner").getByRole("button", { name: "Menu" }));
    expect(await page.evaluate(() => matchMedia("(pointer: coarse)").matches)).toBe(true);
    const space = await firstName(page, "spaces");
    for (const screen of screens(space)) {
      if (!(await screen.open(page))) continue;
      rows.push(await measure(page, screen.name));
      // A masked field stays masked at phone width: no password box turned into plain text.
      expect(await page.locator("input[data-secret], input[autocomplete=current-password]").evaluateAll((inputs) => inputs.filter((input) => (input as HTMLInputElement).type !== "password").length)).toBe(0);
      await snap(page, `critic-phone-${screen.name}-${who}-${size.width}`);
    }
  } finally {
    await context.close();
  }
  return rows;
}

for (const size of SIZES) {
  for (const { who, person } of PEOPLE) {
    test(`at ${size.width} px with touch, as ${who}, no data screen scrolls sideways and every control takes a finger`, async ({ browser }, info) => {
      const rows = await walk(browser, who, person, size);
      await info.attach(`critic-phone-${who}-${size.width}.json`, { body: JSON.stringify(rows, null, 2), contentType: "application/json" });
      if (process.env.SHOTS_DIR) {
        const { appendFileSync } = await import("node:fs");
        for (const row of rows) appendFileSync(`${process.env.SHOTS_DIR}/rows.jsonl`, `${JSON.stringify({ who, ...row })}\n`);
      }
      expect(rows.length).toBeGreaterThan(3);
      const failures = Object.fromEntries(
        rows
          .map((row) => [
            row.screen,
            [...(row.scrollWidth > row.viewport ? [`scrolls sideways: ${row.scrollWidth} > ${row.viewport}`] : []), ...row.small.map((s) => `small: ${s}`), ...row.dialog],
          ])
          .filter(([, problems]) => problems.length),
      );
      expect(failures).toEqual({});
    });
  }
}

// C5 at phone width: the first row of a space's grid on DevTools' "Slow 4G" profile (562.5 ms
// round trip, 1.4 Mbps down, 675 kbps up), five loads with a warm cache, as a person who comes
// back to the page. Recorded for the critic's score; the bar (p75 ≤ 2 s) is the rubric's call.
test("at 400 px on Slow 4G the space grid shows its first row", async ({ browser }, info) => {
  const context = await browser.newContext({ viewport: SIZES[0], hasTouch: true, isMobile: true });
  const page = await context.newPage();
  try {
    await goSignedIn(page, EDITOR, `/projects/${PROJECT}/spaces?lang=en`, (signedIn) => signedIn.getByRole("banner").getByRole("button", { name: "Menu" }));
    const space = await firstName(page, "spaces");
    test.skip(!space, "the project has no space");
    const cdp = await context.newCDPSession(page);
    await cdp.send("Network.enable");
    await cdp.send("Network.emulateNetworkConditions", { offline: false, latency: 562.5, downloadThroughput: (1.4 * 1024 * 1024) / 8, uploadThroughput: (675 * 1024) / 8 });
    const times: number[] = [];
    for (let load = 0; load < 5; load += 1) {
      const start = Date.now();
      await page.goto(`/projects/${PROJECT}/spaces/${space}?lang=en`);
      await page.getByRole("button", { name: /^Open: / }).first().waitFor({ timeout: 120_000 });
      times.push(Date.now() - start);
    }
    const sorted = [...times].sort((a, b) => a - b);
    const p75 = sorted[Math.ceil(sorted.length * 0.75) - 1];
    await info.attach("critic-phone-slow4g.json", { body: JSON.stringify({ times, p75 }), contentType: "application/json" });
    if (process.env.SHOTS_DIR) {
      const { writeFileSync } = await import("node:fs");
      writeFileSync(`${process.env.SHOTS_DIR}/slow4g.json`, JSON.stringify({ times, p75 }));
    }
    expect(times).toHaveLength(5);
  } finally {
    await context.close();
  }
});

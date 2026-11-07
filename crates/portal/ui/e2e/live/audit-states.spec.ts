/**
 * T-4163 — UX photo audit 7/7: the states a single happy screenshot hides, on fifteen
 * representative pages (one per area of T-4157..T-4162). Each page as demo.steward (the public
 * ones as an anonymous visitor) in:
 *
 * - `loading`: the network throttled (Chrome's own emulation, 2.5 s latency), photographed while
 *   the page waits;
 * - `error`: every Portal API read answered 500 in this browser only (the one `page.route` stub);
 * - `focus`: eight Tab presses, so the focus ring on the eighth control is in the picture;
 * - `dark`: `prefers-color-scheme: dark`, which the Portal's tokens follow (src/tokens.css);
 * - `zoom-200`: the layout a 1440 px window shows at 200 %, 720 CSS px wide;
 * - `empty`: a filter that matches nothing, where the page has a search box;
 * - `long-name` and `validation`: a 60-character name typed into a create form, and the form
 *   submitted empty, where the page is a form. Nothing is saved: an empty form is refused, and
 *   the typed name is never submitted;
 * - `long-list`: the longest list the page holds on dev (the Explore grid, the all-Endpoints list).
 *
 * English at 1440 px, and loading, error and dark again in Slovak at 375 px, saved by `shot`
 * (shoot.ts, T-4156) under `$SHOTS_DIR`, which then gets its contact sheet. Read-only.
 *
 *   SHOTS_DIR=/workspace/evidence/<agent>/T-4163 PORTAL_PASSWORD=… \
 *     npx playwright test --config playwright.live.config.ts audit-states
 */
import { readdirSync } from "node:fs";
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { STEWARD, portalReady, signIn } from "./portal";
import { shot, writeIndex } from "./shoot";
import type { Shot } from "./shoot";

const AREA = "states";
const DIR = process.env.SHOTS_DIR;
const P = "/projects/helsinki";
const LONG_NAME = "a-sixty-character-name-for-the-ux-photo-audit-of-t-4163-xyz";

interface Target {
  route: string;
  /** Which audit area (T-4157..T-4162) the page stands for. */
  area: string;
  who: "steward" | "anonymous";
  search?: boolean;
  form?: boolean;
  longList?: boolean;
}

// One page per area, read from ui/src/router.tsx; the form pages are `$plural/new`.
const TARGETS: Target[] = [
  { route: `${P}/home`, area: "shell", who: "steward" },
  { route: `${P}/approvals`, area: "shell", who: "steward" },
  { route: `${P}/access`, area: "access", who: "steward" },
  { route: `${P}/policies`, area: "access", who: "steward", search: true },
  { route: `${P}/policies/new`, area: "access", who: "steward", form: true },
  { route: `${P}/spaces`, area: "data", who: "steward", search: true },
  { route: `${P}/datasources/new`, area: "data", who: "steward", form: true },
  { route: `${P}/pipelines`, area: "data", who: "steward", search: true },
  { route: "/endpoints", area: "data", who: "steward", search: true, longList: true },
  { route: `${P}/explore`, area: "explore", who: "steward", longList: true },
  { route: `${P}/apps`, area: "apps", who: "steward", search: true },
  { route: `${P}/models`, area: "data", who: "steward", search: true },
  { route: "/organization", area: "shell", who: "steward" },
  { route: "/catalogue", area: "public", who: "anonymous", search: true },
  { route: "/glossary", area: "public", who: "anonymous" },
];

/** States that could not be reached, with the reason; printed at the end and written to the task. */
const missed: string[] = [];

async function settle(page: Page): Promise<void> {
  await expect(page.locator('[aria-busy="true"]')).toHaveCount(0, { timeout: 20_000 }).catch(() => undefined);
  await page.waitForTimeout(1200);
}

async function open(page: Page, route: string, lang: string, signedIn: boolean): Promise<void> {
  await page.goto(`${route}${route.includes("?") ? "&" : "?"}lang=${lang}`, { waitUntil: "load" });
  const ready = signedIn ? portalReady(page).or(page.locator("main h1")).first() : page.locator("main").first();
  await expect(ready).toBeVisible({ timeout: 60_000 });
  await page.locator("main h1:visible").first().waitFor({ timeout: 30_000 }).catch(() => undefined);
}

/** `AUDIT_STATES=error,dark` reshoots only those states; unset, every state. */
const ONLY = process.env.AUDIT_STATES?.split(",").filter(Boolean);

/** Waits until the page has drawn some text, so a blank picture is the page's and not the wait's. */
async function drawn(page: Page): Promise<void> {
  await page.waitForLoadState("networkidle", { timeout: 20_000 }).catch(() => undefined);
  await page.waitForFunction(() => (document.body?.innerText ?? "").trim().length > 0, undefined, { timeout: 20_000 }).catch(() => undefined);
}

/**
 * The edge limits every IP to 300 requests a minute, shared by all agents: a 429 on a script
 * chunk leaves a white page that is the limit's and not the Portal's. Such a state is taken again
 * once, a minute later.
 */
async function state(page: Page, what: Omit<Shot, "width">, reach: () => Promise<boolean | void>, settled = true): Promise<void> {
  if (ONLY && !ONLY.includes(what.state)) return;
  let limited = false;
  const watch = (response: { status(): number }) => {
    if (response.status() === 429) limited = true;
  };
  page.on("response", watch);
  try {
    for (let attempt = 0; attempt < 2; attempt += 1) {
      limited = false;
      if ((await reach()) !== false) {
        if (settled) await settle(page);
        await shot(page, what);
      }
      if (!limited) return;
      missed.push(`${what.route} ${what.state} ${what.lang}: 429 from the edge, ${attempt ? "kept" : "taken again"}`);
      await page.waitForTimeout(60_000);
    }
  } catch (error) {
    missed.push(`${what.route} ${what.state} ${what.lang} ${page.viewportSize()?.width}: ${String(error).split("\n")[0]}`);
  } finally {
    page.off("response", watch);
    await page.keyboard.press("Escape").catch(() => undefined);
  }
}

/** Every Portal API read but the session answers 500; the page's own frame stays. */
async function failApi(page: Page): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    if (/\/auth\/|\/session/.test(new URL(route.request().url()).pathname)) return route.continue();
    await route.fulfill({
      status: 500,
      contentType: "application/problem+json",
      body: JSON.stringify({ type: "about:blank", title: "Internal Server Error", status: 500 }),
    });
  });
}

async function walk(page: Page, context: BrowserContext, target: Target, lang: "en" | "sk"): Promise<void> {
  const signedIn = target.who === "steward";
  const base = { area: `${AREA}-${target.area}`, route: target.route, who: target.who, lang };

  await state(
    page,
    { ...base, state: "loading" },
    async () => {
      const cdp = await context.newCDPSession(page);
      await cdp.send("Network.enable");
      await cdp.send("Network.emulateNetworkConditions", {
        offline: false,
        latency: 2500,
        downloadThroughput: 50_000,
        uploadThroughput: 20_000,
      });
      try {
        await page.goto(`${target.route}?lang=${lang}`, { waitUntil: "commit" });
        await page.waitForTimeout(4000);
        await shot(page, { ...base, state: "loading" });
      } finally {
        await cdp.send("Network.emulateNetworkConditions", { offline: false, latency: 0, downloadThroughput: -1, uploadThroughput: -1 });
        await cdp.detach();
      }
      return false;
    },
    false,
  );

  await state(page, { ...base, state: "error" }, async () => {
    await failApi(page);
    try {
      await page.goto(`${target.route}?lang=${lang}`, { waitUntil: "load" });
      await drawn(page);
      await page.waitForTimeout(2000);
      await shot(page, { ...base, state: "error" });
    } finally {
      await page.unrouteAll({ behavior: "ignoreErrors" });
    }
    return false;
  });

  await state(page, { ...base, state: "dark" }, async () => {
    await page.emulateMedia({ colorScheme: "dark" });
    try {
      await open(page, target.route, lang, signedIn);
      await settle(page);
      await shot(page, { ...base, state: "dark" });
    } finally {
      await page.emulateMedia({ colorScheme: "light" });
    }
    return false;
  });

  if (lang === "sk") return;

  await state(page, { ...base, state: "focus" }, async () => {
    await open(page, target.route, lang, signedIn);
    await settle(page);
    for (let i = 0; i < 8; i += 1) await page.keyboard.press("Tab");
    await shot(page, { ...base, state: "focus" });
    return false;
  });

  if (target.search) {
    await state(page, { ...base, state: "empty" }, async () => {
      await open(page, target.route, lang, signedIn);
      const box = page.locator("main").getByRole("searchbox").or(page.locator('main input[type="search"]')).first();
      if (!(await box.count())) return false;
      await box.fill("zzzz-no-such-thing-t4163");
      await page.waitForTimeout(800);
    });
  }

  if (target.form) {
    await state(page, { ...base, state: "validation" }, async () => {
      await open(page, target.route, lang, signedIn);
      await settle(page);
      // An empty form is refused before anything is sent: this is the only press of a submit.
      const submit = page.locator('main button[type="submit"]').first();
      // A submit held disabled until the form is filled is the disabled-with-reason state: hovered,
      // so its reason (a title or tooltip), if it has one, is in the picture.
      if (await submit.isDisabled()) await submit.hover();
      else await submit.click();
      await page.waitForTimeout(800);
    });
    await state(page, { ...base, state: "long-name" }, async () => {
      await open(page, target.route, lang, signedIn);
      await settle(page);
      const name = page.locator("main").getByRole("textbox", { name: /^Name/ }).first();
      await name.fill(LONG_NAME);
      await name.press("Tab");
    });
  }

  // 200 % of a 1440 px window is the layout of 720 CSS px; the same session, so no second sign-in.
  await state(page, { ...base, state: "zoom-200" }, async () => {
    const before = page.viewportSize();
    await page.setViewportSize({ width: 720, height: 450 });
    try {
      await open(page, target.route, lang, signedIn);
      await settle(page);
      await shot(page, { ...base, state: "zoom-200" });
    } finally {
      if (before) await page.setViewportSize(before);
    }
    return false;
  });

  if (target.longList) {
    await state(page, { ...base, state: "long-list" }, async () => {
      await open(page, target.route, lang, signedIn);
      await settle(page);
      await page.mouse.wheel(0, 4000);
    });
  }
}

test.describe.configure({ mode: "serial" });
test.setTimeout(3_600_000);

test("cross-cutting states as demo.steward", async ({ browser }) => {
  test.skip(!DIR, "SHOTS_DIR is not set");
  const { context, page } = await signIn(browser, STEWARD, `${P}/home?lang=en`);
  try {
    for (const [lang, width] of [
      ["en", 1440],
      ["sk", 375],
    ] as const) {
      await page.setViewportSize({ width, height: width < 768 ? 812 : 900 });
      for (const target of TARGETS.filter((t) => t.who === "steward")) await walk(page, context, target, lang);
    }
  } finally {
    await context.close();
  }
});

test("cross-cutting states of the public pages, anonymous", async ({ browser }) => {
  test.skip(!DIR, "SHOTS_DIR is not set");
  for (const [lang, width] of [
    ["en", 1440],
    ["sk", 375],
  ] as const) {
    const context = await browser.newContext({ viewport: { width, height: width < 768 ? 812 : 900 } });
    const page = await context.newPage();
    try {
      for (const target of TARGETS.filter((t) => t.who === "anonymous")) await walk(page, context, target, lang);
    } finally {
      await context.close();
    }
  }
});

test.afterAll(() => {
  if (!DIR) return;
  const count = writeIndex(DIR);
  const states = readdirSync(DIR).filter((file) => file.startsWith(`${AREA}-`) && file.endsWith(".png")).length;
  console.log(`UX photo audit, states: ${states} screenshots in ${DIR} (${count} in the folder)`);
  if (missed.length) console.log(`not reached (${missed.length}):\n${missed.join("\n")}`);
  expect(states).toBeGreaterThanOrEqual(60);
});

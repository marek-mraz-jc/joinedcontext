/**
 * T-5525 — UI critic 5/6: data workspace navigation. How many clicks and seconds from sign-in to
 * one type's rows, to one row, to that row's history, and back, as demo.editor and demo.viewer.
 *
 * The walk records; it judges no taste. Each step writes its clicks, its milliseconds, the
 * address it ended on and what the browser's Back or a reload kept into `$SHOTS_DIR/critic-nav.json`
 * beside a 1440 and a 400 px picture of the state. Every deep link the walk produced is then
 * opened in a fresh browser context: as the other persona (the same rights in helsinki, so the
 * same filtered state must open) and signed out (sign-in and no row data). The axe step fails on
 * serious or critical violations of the pages walked.
 *
 * Read-only: the personas only read helsinki's seeded space `helsinki` (type AirQualityObserved)
 * and its Endpoint; no request but GET leaves the browser for the Portal's API.
 *
 *   SHOTS_DIR=/workspace/evidence/<agent>/T-5525 VIEWER_PASSWORD=… EDITOR_PASSWORD=… \
 *     npx playwright test --config playwright.live.config.ts critic-nav
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Browser, Locator, Page } from "@playwright/test";
import { axeViolations } from "../axe";
import { EDITOR, VIEWER, signIn } from "./portal";

test.setTimeout(900_000);

const PROJECT = process.env.CRITIC_PROJECT ?? "helsinki";
const OTHER_PROJECT = process.env.CRITIC_OTHER_PROJECT ?? "banskabystrica";
const SPACE = process.env.CRITIC_SPACE ?? "helsinki";
const SPACE_TITLE = process.env.CRITIC_SPACE_TITLE ?? "Helsinki city context";
const TYPE = process.env.CRITIC_TYPE ?? "AirQualityObserved";
const ROW = process.env.CRITIC_ROW ?? "Herttoniemi";
const HISTORY_ATTRIBUTE = process.env.CRITIC_HISTORY_ATTRIBUTE ?? "PM10";
const ENDPOINT = process.env.CRITIC_ENDPOINT ?? "helsinki-air-quality";
const DIR = process.env.SHOTS_DIR;
const PERSONAS = [
  { who: "editor", cred: EDITOR },
  { who: "viewer", cred: VIEWER },
] as const;

interface Step {
  persona: string;
  task: string;
  clicks: number;
  ms: number;
  url: string;
  kept?: string;
  note?: string;
}

const steps: Step[] = [];
const links: { persona: string; task: string; url: string; expect: string }[] =
  [];
const axe: { persona: string; page: string; serious: string[] }[] = [];
const hops: { persona: string; ms: number[] }[] = [];

/** Counts each click a person makes, and times a step from its first click to its landmark. */
class Walk {
  clicks = 0;
  started = 0;
  /** Milliseconds from each click to the landmark of the page it opened. */
  hops: number[] = [];
  constructor(
    readonly page: Page,
    readonly persona: string,
  ) {}
  begin(): void {
    this.clicks = 0;
    this.started = Date.now();
  }
  async click(target: Locator): Promise<void> {
    this.clicks += 1;
    await target.click();
  }
  /** A click that changes the route, timed until `landed` is visible. */
  async hop(target: Locator, landed: Locator): Promise<void> {
    const t = Date.now();
    await this.click(target);
    await expect(landed).toBeVisible({ timeout: 60_000 });
    this.hops.push(Date.now() - t);
  }
  async type(target: Locator, text: string): Promise<void> {
    this.clicks += 1;
    await target.fill(text);
  }
  done(task: string, extra: Partial<Step> = {}): Step {
    const step = {
      persona: this.persona,
      task,
      clicks: this.clicks,
      ms: Date.now() - this.started,
      url: relative(this.page.url()),
      ...extra,
    };
    steps.push(step);
    console.log(
      `${step.persona.padEnd(7)}| ${task.padEnd(34)}| ${String(step.clicks).padStart(2)} clicks | ${String(step.ms).padStart(6)} ms | ${step.url}${step.kept ? ` | kept: ${step.kept}` : ""}${step.note ? ` | ${step.note}` : ""}`,
    );
    return step;
  }
}

const relative = (url: string) => {
  const u = new URL(url);
  u.searchParams.delete("lang");
  return `${u.pathname}${u.search}`;
};

async function pictures(
  page: Page,
  persona: string,
  state: string,
): Promise<void> {
  if (!DIR) return;
  mkdirSync(DIR, { recursive: true });
  const size = page.viewportSize() ?? { width: 1440, height: 900 };
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
    await page.waitForTimeout(400);
    await page.screenshot({
      path: join(DIR, `nav-${state}-${persona}-${width}.png`),
      fullPage: false,
    });
  }
  await page.setViewportSize(size);
}

async function axeCheck(
  page: Page,
  persona: string,
  name: string,
): Promise<void> {
  const serious = (await axeViolations(page))
    .filter((v) => /\((serious|critical)\)/.test(v))
    .map((v) => v.slice(0, 200));
  axe.push({ persona, page: name, serious });
}

const heading = (page: Page, name: string | RegExp) =>
  page.getByRole("heading", { level: 1, name });
const rowDialog = (page: Page) =>
  page.getByRole("dialog", { name: /^Details:/ });
const typeButton = (page: Page) =>
  page.locator("main").getByRole("button", { name: TYPE, exact: true }).first();

/** The focus after a route change: what a screen reader announces on arrival (UI-16). */
const focusIsHeading = (page: Page) =>
  page.evaluate(() => document.activeElement?.tagName === "H1");

/** Every request but a read is stopped in the browser: the walk changes nothing. */
async function readOnly(page: Page): Promise<void> {
  await page.route(
    (url) => url.pathname.startsWith("/api/"),
    (route) =>
      ["GET", "HEAD"].includes(route.request().method())
        ? route.continue()
        : route.abort(),
  );
}

async function walk(
  browser: Browser,
  persona: string,
  cred: { user: string; password: string },
): Promise<void> {
  const t0 = Date.now();
  const { context, page } = await signIn(browser, cred, "/?lang=en");
  await page.setViewportSize({ width: 1440, height: 900 });
  await readOnly(page);
  const w = new Walk(page, persona);
  let stage = "sign-in";
  steps.push({
    persona,
    task: "sign-in lands on",
    clicks: 1,
    ms: Date.now() - t0,
    url: relative(page.url()),
  });
  // A fresh browser opens the person's own project, never the sample listed first (T-5603).
  expect.soft(relative(page.url()), "sign-in lands on").toMatch(new RegExp(`^/projects/${PROJECT}/`));
  try {
    stage = "rows";
    // 1. Sign-in to the rows of TYPE in SPACE, through the navigation a person sees.
    w.begin();
    const switcher = page.getByRole("button", {
      name: "Projects",
      exact: true,
    });
    if (!page.url().includes(`/projects/${PROJECT}/`)) {
      await w.click(switcher);
      await w.click(
        page.getByRole("menuitem", { name: new RegExp(PROJECT, "i") }).first(),
      );
      await page.waitForURL(new RegExp(`/projects/${PROJECT}/`));
    }
    await w.hop(
      page
        .getByRole("navigation", { name: "Main navigation" })
        .getByRole("link", { name: "Context Spaces" }),
      heading(page, "Context Spaces"),
    );
    await w.hop(
      page.locator("main").getByRole("link", { name: SPACE_TITLE }).first(),
      heading(page, SPACE_TITLE),
    );
    const focusOnArrival = await focusIsHeading(page);
    await w.hop(
      typeButton(page),
      page.locator("main").getByRole("columnheader").first(),
    );
    w.done("sign-in to rows of a type", {
      note: `focus on h1 after route change: ${focusOnArrival}; title "${await page.title()}"`,
    });
    const rowsUrl = page.url();
    links.push({
      persona,
      task: "rows of a type (space)",
      url: relative(rowsUrl),
      expect: `type ${TYPE} selected`,
    });
    await pictures(page, persona, "rows");
    await axeCheck(page, persona, "space rows");

    w.begin();
    await page.reload();
    await expect(
      page.locator("main").getByRole("columnheader").first(),
    ).toBeVisible({ timeout: 60_000 });
    w.done("reload space rows", {
      kept:
        (await typeButton(page).getAttribute("aria-current")) ||
        page.url().includes(`type=${TYPE}`)
          ? "type"
          : "nothing",
    });

    stage = "one row";
    // 2. One row by its name.
    w.begin();
    const search = page.locator("main").getByRole("searchbox").first();
    const asked: string[] = [];
    const listen = async (r: import("@playwright/test").Response) => {
      const u = new URL(r.url());
      if (
        !u.pathname.endsWith("/entities") ||
        !u.searchParams.get("q")?.includes(ROW)
      )
        return;
      const body = await r.json().catch(() => null);
      asked.push(
        `${r.status()} ${Array.isArray(body) ? body.length : "?"} rows for q=${u.searchParams.get("q")} at ${u.pathname.split("/ngsi-ld/")[0]}`,
      );
    };
    page.on("response", listen);
    await w.type(search, ROW);
    // The search waits 400 ms before it asks (SpaceInside.tsx), then the grid reads again.
    await page.waitForTimeout(5000);
    const row = page.locator("main").getByText(new RegExp(ROW)).first();
    const found = await row.isVisible();
    const loaded = (
      await page
        .locator("main")
        .getByText(/ of \d+ loaded|^\d+ rows?$/)
        .first()
        .innerText()
        .catch(() => "")
    ).trim();
    await pictures(page, persona, "search");
    page.off("response", listen);
    let how = `the search found it (${asked.join("; ")})`;
    if (!found) {
      // The search found nothing: the persona scrolls the list instead.
      how = `the search found nothing (${loaded}; ${asked.join("; ") || "no query sent"}); cleared it and picked the row from the list`;
      await w.type(search, "");
      await expect(row).toBeVisible({ timeout: 30_000 });
    }
    await w.click(row);
    await expect(rowDialog(page)).toBeVisible();
    w.done("one row by its name", {
      note: `${how}; search in URL: ${page.url().includes(encodeURIComponent(ROW)) || page.url().includes(ROW)}`,
    });
    const rowUrl = page.url();
    links.push({
      persona,
      task: "one row (space)",
      url: relative(rowUrl),
      expect: "row details open",
    });
    await pictures(page, persona, "row");

    stage = "history";
    // 3. That row's history of one attribute.
    w.begin();
    const temporal: string[] = [];
    page.on("response", (r) => {
      if (r.url().includes("/temporal/entities/"))
        temporal.push(
          `${r.status()} ${new URL(r.url()).pathname.split("/ngsi-ld/")[0]}`,
        );
    });
    await w.click(rowDialog(page).getByRole("tab", { name: "History" }));
    await page.waitForTimeout(3000);
    const historyOf = async () =>
      (await rowDialog(page).locator("[role=tabpanel]").innerText())
        .replace(/\s+/g, " ")
        .slice(-140);
    const firstAttribute = await historyOf();
    const attributePick = rowDialog(page).getByRole("combobox").first();
    if (await attributePick.count()) {
      w.clicks += 1;
      const options = await attributePick.locator("option").evaluateAll((all) =>
        all.map((o) => ({
          value: (o as HTMLOptionElement).value,
          label: o.textContent ?? "",
        })),
      );
      const wanted = options.find((o) => o.label.includes(HISTORY_ATTRIBUTE));
      if (!wanted)
        throw new Error(
          `no ${HISTORY_ATTRIBUTE} among ${options.map((o) => o.label).join(", ")}`,
        );
      await attributePick.selectOption(wanted.value);
      await page.waitForTimeout(3000);
    }
    w.done("row history of one attribute", {
      note: `default attribute shows "${firstAttribute}"; ${HISTORY_ATTRIBUTE} shows "${await historyOf()}"; temporal answers ${temporal.join(", ")}`,
    });
    links.push({
      persona,
      task: "row history (space)",
      url: relative(page.url()),
      expect: "history tab of the row",
    });
    await pictures(page, persona, "history");

    stage = "back";
    // 4. Back to the space overview with the browser's Back.
    w.begin();
    await page.keyboard.press("Escape");
    await expect(rowDialog(page)).toBeHidden();
    const afterClose = relative(page.url());
    let backs = 0;
    const trail: string[] = [];
    while (
      backs < 8 &&
      relative(page.url()) !== `/projects/${PROJECT}/spaces/${SPACE}`
    ) {
      backs += 1;
      const t = Date.now();
      await page.goBack({ waitUntil: "commit" });
      await page.waitForTimeout(1500);
      trail.push(
        `${Date.now() - t - 1500} ms${(await rowDialog(page).count()) ? " row dialog open again" : ""} ${relative(page.url()).replace(/urn%3A[^&]+/, "<urn>")}`,
      );
    }
    w.done("Back to the space overview", {
      clicks: backs,
      note: `closed the row on ${afterClose.replace(/urn%3A[^&]+/, "<urn>")}; Back: ${trail.join(" → ")}`,
    });

    stage = "switch project";
    // 5. Switch project and come back.
    await page.goto(`${rowsUrl}`);
    await expect(
      page.locator("main").getByRole("columnheader").first(),
    ).toBeVisible({ timeout: 60_000 });
    w.begin();
    await w.click(switcher);
    await w.click(
      page
        .getByRole("menuitem", { name: new RegExp(OTHER_PROJECT, "i") })
        .first(),
    );
    await page.waitForURL(new RegExp(`/projects/${OTHER_PROJECT}`));
    await expect(heading(page, /.+/)).toBeVisible();
    const away = relative(page.url());
    await w.click(switcher);
    await w.click(
      page.getByRole("menuitem", { name: new RegExp(PROJECT, "i") }).first(),
    );
    await page.waitForURL(new RegExp(`/projects/${PROJECT}`));
    await expect(heading(page, /.+/)).toBeVisible();
    // The switch back reopens the space and type the person left (T-5603).
    expect.soft(page.url(), "switch project and come back").toContain(`type=${TYPE}`);
    w.done("switch project and come back", {
      kept: page.url().includes(`type=${TYPE}`)
        ? "space and type"
        : page.url().includes(`/spaces/${SPACE}`)
          ? "space"
          : "nothing",
      note: `away on ${away}`,
    });

    stage = "endpoint";
    // 6. The same type from the Endpoint page.
    w.begin();
    await w.hop(
      page
        .getByRole("navigation", { name: "Main navigation" })
        .getByRole("link", { name: "Endpoints" }),
      heading(page, "Endpoints"),
    );
    await w.hop(
      page
        .locator("main")
        .getByRole("link", {
          name: new RegExp(ENDPOINT.replace(/-/g, "[- ]"), "i"),
        })
        .first(),
      heading(page, /air quality/i),
    );
    const dataType = page
      .locator("main")
      .getByRole("combobox", { name: /type/i })
      .first();
    if (await dataType.count()) {
      w.clicks += 1;
      await dataType
        .selectOption({ label: TYPE })
        .catch(async () => dataType.selectOption(TYPE));
    }
    await expect(
      page.locator("main").getByRole("columnheader").first(),
    ).toBeVisible({ timeout: 60_000 });
    w.done("same type from the Endpoint page", {
      note: `title "${await page.title()}"`,
    });
    links.push({
      persona,
      task: "rows of a type (Endpoint)",
      url: relative(page.url()),
      expect: `type ${TYPE} rows`,
    });
    await pictures(page, persona, "endpoint");
    await axeCheck(page, persona, "endpoint data");
    w.begin();
    await page.reload();
    await expect(
      page.locator("main").getByRole("columnheader").first(),
    ).toBeVisible({ timeout: 60_000 });
    w.done("reload Endpoint data", {
      kept: page.url().includes(`dataType=${TYPE}`) ? "type" : "nothing",
    });

    stage = "explore";
    // 7. The same type from Explore.
    w.begin();
    await w.hop(
      page
        .getByRole("navigation", { name: "Main navigation" })
        .getByRole("link", { name: "Explore data" }),
      heading(page, "Explore data"),
    );
    await w.click(
      page
        .locator("main")
        .getByRole("button", { name: SPACE_TITLE, exact: true }),
    );
    w.clicks += 1;
    await page.locator("#explore-type").selectOption(TYPE);
    await expect(
      page.locator("main").getByRole("columnheader").first(),
    ).toBeVisible({ timeout: 60_000 });
    const endpointPicked = await page.locator("#explore-endpoint").inputValue();
    // The address names the Endpoint read, so the other persona opening the link reads
    // through the same one or is told why not (T-5604).
    const endpointInAddress = new URL(page.url()).searchParams.get("endpoint");
    w.done("same type from Explore", {
      note: `endpoint picked for the person: ${endpointPicked}; in the address: ${endpointInAddress ?? "none"}`,
    });
    links.push({
      persona,
      task: "rows of a type (Explore)",
      url: relative(page.url()),
      expect: `type ${TYPE} rows`,
    });
    await pictures(page, persona, "explore");
    await axeCheck(page, persona, "explore");
    w.begin();
    await page.reload();
    await page.waitForTimeout(4000);
    const typeAfter = await page
      .locator("#explore-type")
      .inputValue()
      .catch(() => "");
    w.done("reload Explore", {
      kept:
        typeAfter === TYPE
          ? "type"
          : page.url().includes("space=")
            ? "space only"
            : "nothing",
    });

    stage = "copies";
    // 8. The copy bar (WorkspaceBar) as the persona meets it: the list of copies.
    hops.push({ persona, ms: w.hops });
    w.begin();
    await page.goto(`/projects/${PROJECT}/workspaces?lang=en`);
    const drawn = await heading(page, /.+/)
      .waitFor({ state: "visible", timeout: 60_000 })
      .then(
        () => true,
        () => false,
      );
    w.done("copies list (WorkspaceBar entry)", {
      note: drawn
        ? `bar shown: ${await page.getByRole("region", { name: /copy/i }).count()}`
        : "the page drew nothing in 60 s (edge 429 or a failed boot)",
    });
    await pictures(page, persona, "copies");
  } catch (error) {
    hops.push({ persona, ms: w.hops });
    await pictures(page, persona, `stopped-${stage.replace(/ /g, "-")}`);
    throw new Error(`${stage}: ${plain(error)}`);
  } finally {
    await context.close();
  }
}

/** The terminal's colour codes Playwright puts into an error message. */
const COLOUR = new RegExp(`${String.fromCharCode(27)}\\[[0-9;]*m`, "g");

/** The first three lines of an error without the terminal's colour codes. */
const plain = (error: unknown) =>
  String(error).replace(COLOUR, "").split("\n").slice(0, 3).join(" ");

/** Each deep link opened cold: as the other persona, and signed out. */
async function deepLinks(browser: Browser): Promise<void> {
  for (const link of links) {
    const other = PERSONAS.find((p) => p.who !== link.persona) ?? PERSONAS[0];
    const t0 = Date.now();
    const { context, page } = await signIn(
      browser,
      other.cred,
      `${link.url}${link.url.includes("?") ? "&" : "?"}lang=en`,
    );
    await expect(page.locator("main").getByRole("columnheader").first())
      .toBeVisible({ timeout: 60_000 })
      .catch(() => undefined);
    if (link.expect.startsWith("row") || link.expect.startsWith("history")) {
      await expect(rowDialog(page))
        .toBeVisible({ timeout: 30_000 })
        .catch(() => undefined);
    }
    const rowOpen = await rowDialog(page).count();
    const historyTab = rowOpen
      ? await rowDialog(page)
          .getByRole("tab", { name: "History" })
          .getAttribute("aria-selected")
      : null;
    const typeShown =
      page.url().includes(TYPE) ||
      (await page
        .locator("#explore-type")
        .inputValue()
        .catch(() => "")) === TYPE;
    steps.push({
      persona: other.who,
      task: `deep link: ${link.task}`,
      clicks: 0,
      ms: Date.now() - t0,
      url: relative(page.url()),
      note: `expected ${link.expect}; type kept ${typeShown}; row dialog ${rowOpen > 0}; history tab selected ${historyTab}`,
    });
    console.log(
      `${other.who.padEnd(7)}| deep link ${link.task.padEnd(24)}| expected ${link.expect}; type ${typeShown}; row ${rowOpen > 0} | ${relative(page.url())}`,
    );
    await context.close();

    const anon = await browser.newContext();
    const cold = await anon.newPage();
    await cold.goto(`${link.url}`);
    await cold.waitForTimeout(4000);
    const body = await cold.locator("body").innerText();
    const leaked =
      body.includes(ROW) || (await cold.getByRole("columnheader").count()) > 0;
    steps.push({
      persona: "anonymous",
      task: `deep link: ${link.task}`,
      clicks: 0,
      ms: 0,
      url: relative(cold.url()),
      note: `row data shown: ${leaked}`,
    });
    expect(leaked, `signed out, ${link.url} shows no row data`).toBe(false);
    await anon.close();
  }
}

test("navigation critic: clicks and seconds per task and persona, deep links cold", async ({
  browser,
}) => {
  for (const { who, cred } of PERSONAS) {
    try {
      await walk(browser, who, cred);
    } catch (error) {
      steps.push({
        persona: who,
        task: "walk stopped",
        clicks: 0,
        ms: 0,
        url: "",
        note: plain(error),
      });
      console.log(`${who}: walk stopped: ${plain(error)}`);
    }
  }
  await deepLinks(browser);
  if (DIR)
    writeFileSync(
      join(DIR, "critic-nav.json"),
      JSON.stringify({ steps, links, axe, hops }, null, 2),
    );
  for (const h of hops) {
    const sorted = [...h.ms].sort((a, b) => a - b);
    console.log(
      `route changes ${h.persona}: ${h.ms.join(", ")} ms; p75 ${sorted[Math.ceil(sorted.length * 0.75) - 1]} ms`,
    );
  }
  for (const a of axe)
    console.log(
      `axe ${a.persona} ${a.page}: ${a.serious.join(", ") || "none serious"}`,
    );
  expect(
    axe.flatMap((a) => a.serious),
    "no serious or critical axe violation on the pages walked",
  ).toEqual([]);
});

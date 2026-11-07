/**
 * T-4160 — UX photo audit 4/7: spaces, the catalogue, explore (table, map, chart, export, an empty
 * result), knowledge, the assistant admin (an AssistantDeployment's form) and its chat, dashboards,
 * apps (one opened framed in the Portal), the glossary and the Assistant page.
 *
 * Each module's list, a detail, its create and edit forms as they open and after Save pressed
 * empty, at 375, 768 and 1440 px, in sk and en, as demo.steward and demo.viewer, saved by `shot`
 * (shoot.ts, T-4156) under `$SHOTS_DIR`, which then gets its contact sheet. Explore reads project
 * `$AUDIT_PROJECT` (default bbsk) with its real data: a type of `$AUDIT_SPACE` for the table, the
 * first of `$AUDIT_MAP_TYPES` in `$AUDIT_MAP_SPACE` whose model has a place for the map (project
 * `$AUDIT_MAP_PROJECT`), and a search nothing matches.
 *
 * Read-only by construction: every request the page sends to the Portal's API other than GET or
 * HEAD is aborted in the browser (and printed once), so a Save pressed on a form can only show its
 * messages. Let through are the POSTs that only read (a permission check, the access explorer, a
 * retention preview) and a single question to the knowledge assistant (steward, sk, 1440), which
 * answers and stores nothing. The Assistant page gets a question typed into its composer, never
 * sent, because a sent one starts an agent run. A state whose page met a 429 or a script that did
 * not load is retaken, never saved. The count printed at the end is read from the folder.
 *
 * The edge allows 300 requests a minute per address, shared by every agent on the server: run one
 * audit at a time, holding `tasks/.audit.lock` (mkdir) for the run.
 *
 *   SHOTS_DIR=/workspace/evidence/<agent>/T-4160 PORTAL_PASSWORD=… VIEWER_PASSWORD=… \
 *     npx playwright test --config playwright.live.config.ts audit-explore-apps
 */
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import type { Page, Request } from "@playwright/test";
import { STEWARD, VIEWER, journeyMarkerOnPortalOnly, portalReady, signIn } from "./portal";
import { shot, writeIndex } from "./shoot";
import type { Shot } from "./shoot";

const here = dirname(fileURLToPath(import.meta.url));
const LANGS = ["sk", "en"] as const;
type Lang = (typeof LANGS)[number];
type Who = "steward" | "viewer";
const WIDTHS = (process.env.AUDIT_WIDTHS ?? "375,768,1440")
  .split(",")
  .map(Number);
const PROJECT = process.env.AUDIT_PROJECT ?? "bbsk";
const P = `/projects/${PROJECT}`;
/** A space and a type of it with a place, so the map tab is offered (bbsk-registre on dev). */
const SPACE = process.env.AUDIT_SPACE ?? "bbsk-registre";
const TYPE = process.env.AUDIT_TYPE ?? "Hospital";
const DIR = process.env.SHOTS_DIR;

const locales = Object.fromEntries(
  LANGS.map((lang) => [
    lang,
    JSON.parse(
      readFileSync(join(here, `../../src/locales/${lang}.json`), "utf8"),
    ) as Record<string, unknown>,
  ]),
) as Record<Lang, Record<string, unknown>>;

/** A label as the Portal renders it in `lang`, from the same locale file. */
function words(lang: Lang, key: string): string {
  const value = key
    .split(".")
    .reduce<unknown>(
      (node, part) => (node as Record<string, unknown>)[part],
      locales[lang],
    );
  if (typeof value !== "string") throw new Error(`no ${lang} text at ${key}`);
  return value;
}

/** The routes of router.tsx this audit covers; the spec checks each was shot at least once. */
const ROUTES = [
  `${P}/spaces`,
  `${P}/spaces/new`,
  `${P}/spaces/$name`,
  `${P}/spaces/$name/edit`,
  `${P}/spaces/complete`,
  `${P}/spaces/upload`,
  "/catalogue",
  "/catalogue/$name",
  `${P}/explore`,
  `${P}/knowledge`,
  `${P}/knowledge/$source`,
  `${P}/knowledgesources/new`,
  `${P}/assistantdeployments/new`,
  `${P}/assistantdeployments/$name/edit`,
  `${P}/dashboards`,
  `${P}/dashboards/new`,
  `${P}/dashboards/$name`,
  `${P}/apps`,
  `${P}/apps/new`,
  `${P}/apps/$name`,
  `${P}/apps/$name/open`,
  "/glossary",
  `${P}/assistant`,
];

/** Requests the edge refused (429) or scripts that failed since the state began to load. */
const refused: string[] = [];
/** Requests in flight, the Portal's EventSource streams aside, which never close (portal.ts). */
let inFlight = 0;
/**
 * The network as idle as a signed-in page gets: no request in flight for half a second, or 4 s
 * at most, since the Portal polls and a page is never wholly quiet (22 s a state with 20).
 */
async function quiet(page: Page): Promise<void> {
  const until = Date.now() + 4_000;
  let still = 0;
  while (Date.now() < until && still < 5) {
    still = inFlight ? 0 : still + 1;
    await page.waitForTimeout(100);
  }
}
const ONLY = process.env.AUDIT_STATES?.split(",");
/** States that could not be reached, with the reason; printed at the end. */
const missed: string[] = [];
/** The routes that got at least one picture. */
const shotRoutes = new Set<string>();
/** How many person-and-width tests walked to the end in this worker. */
let walked = 0;

async function state(
  page: Page,
  what: Omit<Shot, "width">,
  reach: () => Promise<void>,
): Promise<void> {
  // `AUDIT_STATES=framed,map` retakes those states alone (a re-shoot after a spec fix).
  if (ONLY && !ONLY.includes(what.state)) return;
  try {
    // A picture taken after a 429 or a script that did not load is a white page, not the
    // state: the edge allows 300 requests a minute per address (T-4160 rules), so retake it.
    for (let attempt = 1; ; attempt++) {
      refused.length = 0;
      const reached = await reach().then(
        () => undefined,
        (error: unknown) => error,
      );
      await quiet(page);
      // Fonts, icons, tiles and lazy panels settle before the picture: nothing busy, then a beat.
      await expect(page.locator('[aria-busy="true"]'))
        .toHaveCount(0, { timeout: 15_000 })
        .catch(() => undefined);
      await page.waitForTimeout(1200);
      if (!refused.length && !reached) break;
      if (attempt === (refused.length ? 3 : 2))
        throw reached ?? new Error(`refused: ${refused.join(", ")}`);
      await page.waitForTimeout(refused.length ? 15_000 : 5_000);
    }
    await shot(page, what);
    shotRoutes.add(what.route);
  } catch (error) {
    if (process.env.AUDIT_DEBUG_DIR)
      await page
        .screenshot({
          path: join(
            process.env.AUDIT_DEBUG_DIR,
            `${what.area}-${what.state}-${what.who}-${what.lang}-${page.viewportSize()?.width}.png`,
          ),
        })
        .catch(() => undefined);
    const line = `${what.route} ${what.state} ${what.who} ${what.lang} ${page.viewportSize()?.width}: ${String(error).split("\n")[0]}`;
    missed.push(line);
    console.log(`missed ${line}`);
  } finally {
    await page.keyboard.press("Escape").catch(() => undefined);
    await page.keyboard.press("Escape").catch(() => undefined);
  }
}

async function open(page: Page, address: string, lang: Lang): Promise<void> {
  await page.goto(
    `${address}${address.includes("?") ? "&" : "?"}lang=${lang}`,
    // The live config sets no navigation timeout: a load that never ends held one test 20 minutes.
    { waitUntil: "load", timeout: 45_000 },
  );
  await expect(
    portalReady(page)
      .or(page.locator('main h1, [data-testid="form-page"]'))
      // At 375 px the navigation is in the DOM but folded away, and it comes first.
      .filter({ visible: true })
      .first(),
  ).toBeVisible({ timeout: 60_000 });
  await page
    .locator("main h1:visible")
    .first()
    .waitFor({ timeout: 30_000 })
    .catch(() => undefined);
}

/** The first item a list page links to under `prefix/`, its own pages (`new`, …) aside. */
async function firstItem(
  page: Page,
  prefix: string,
  skip: string[] = [],
): Promise<string> {
  await page
    .locator(`main a[href^="${prefix}/"]`)
    .first()
    .waitFor({ timeout: 20_000 })
    .catch(() => undefined);
  const found = await page.locator("main a[href]").evaluateAll(
    (links, [prefix, skip]) => {
      for (const link of links) {
        const path = new URL((link as HTMLAnchorElement).href).pathname;
        const rest = path.startsWith(`${prefix}/`)
          ? path.slice(prefix.length + 1)
          : "";
        const name = rest.split("/")[0];
        if (name && !(skip as readonly string[]).includes(name))
          return `${prefix}/${name}`;
      }
      return "";
    },
    [prefix, ["new", ...skip]] as const,
  );
  if (!found) throw new Error(`no item linked under ${prefix}`);
  return found;
}

/** A create or edit form as it opens, and after Save pressed on it as it stands. */
async function form(
  page: Page,
  base: Omit<Shot, "width" | "route" | "state">,
  route: string,
  address: string,
  lang: Lang,
  empty: boolean,
): Promise<void> {
  await state(page, { ...base, route, state: "form-open" }, () =>
    open(page, address, lang),
  );
  if (!empty) return;
  await state(
    page,
    { ...base, route, state: "form-validated-empty" },
    async () => {
      await open(page, address, lang);
      // The form's own Save: in the page, or in the docked panel the App generator opens in.
      const save = page
        .locator('form button[type="submit"]')
        .filter({ visible: true })
        .first();
      // A Save the form keeps disabled while it is empty is the state itself.
      await save.waitFor({ timeout: 5_000 });
      if (await save.isEnabled()) await save.click({ timeout: 10_000 });
      await page.waitForTimeout(800);
    },
  );
}

/** Names found on the list pages, kept per run so each width does not look them up again. */
const names = new Map<string, string>();
async function item(
  page: Page,
  list: string,
  lang: Lang,
  skip: string[] = [],
): Promise<string> {
  const known = names.get(list);
  if (known) return known;
  await open(page, list, lang);
  const found = await firstItem(page, list, skip);
  names.set(list, found);
  return found;
}

let asked = false;
/**
 * The map's project, space and types: the tab is offered only for a type whose model declares a
 * place (a `location` slot or a GeoProperty), so the first walk probes them once; when none
 * offers it, the map is reported missing once and not looked for again (2026-10-16: no seeded
 * model on dev declares one).
 */
const MAP_PROJECT = process.env.AUDIT_MAP_PROJECT ?? PROJECT;
const MAP_SPACE = process.env.AUDIT_MAP_SPACE ?? SPACE;
const MAP_TYPES = (
  process.env.AUDIT_MAP_TYPES ?? `${TYPE},Bridge,AdministrativeArea`
).split(",");
let mapOffered: boolean | undefined;
let mapType: string | undefined;
/** POSTs that only read: the endpoint's permission check, the access explorer, a retention preview. */
const READS = /\/(access\/check|access\/explore|retention\/preview)$/;
/** Writes the guard stopped, printed once each. */
const blocked = new Set<string>();

async function walk(page: Page, who: Who, lang: Lang): Promise<void> {
  const width = page.viewportSize()?.width ?? 0;
  const at = (area: string) => ({ area, who, lang }) as const;

  // Spaces.
  await state(
    page,
    { ...at("spaces"), route: `${P}/spaces`, state: "list" },
    () => open(page, `${P}/spaces`, lang),
  );
  await form(
    page,
    at("spaces"),
    `${P}/spaces/new`,
    `${P}/spaces/new`,
    lang,
    true,
  );
  await state(
    page,
    { ...at("spaces"), route: `${P}/spaces/$name`, state: "detail" },
    async () =>
      open(
        page,
        await item(page, `${P}/spaces`, lang, ["complete", "upload"]),
        lang,
      ),
  );
  await state(
    page,
    { ...at("spaces"), route: `${P}/spaces/$name/edit`, state: "form-open" },
    async () =>
      open(
        page,
        `${await item(page, `${P}/spaces`, lang, ["complete", "upload"])}/edit`,
        lang,
      ),
  );
  await state(
    page,
    { ...at("spaces"), route: `${P}/spaces/complete`, state: "page" },
    () => open(page, `${P}/spaces/complete`, lang),
  );
  await state(
    page,
    { ...at("spaces"), route: `${P}/spaces/upload`, state: "page" },
    () => open(page, `${P}/spaces/upload`, lang),
  );

  // The catalogue.
  await state(
    page,
    { ...at("catalogue"), route: "/catalogue", state: "list" },
    () => open(page, "/catalogue", lang),
  );
  await state(
    page,
    { ...at("catalogue"), route: "/catalogue/$name", state: "detail" },
    async () => open(page, await item(page, "/catalogue", lang), lang),
  );

  // Explore, with the project's real data.
  const explore = `${P}/explore`;
  const typed = `${explore}?space=${encodeURIComponent(SPACE)}&type=${encodeURIComponent(TYPE)}`;
  await state(page, { ...at("explore"), route: explore, state: "start" }, () =>
    open(page, explore, lang),
  );
  await state(
    page,
    { ...at("explore"), route: explore, state: "table" },
    async () => {
      await open(page, typed, lang);
      await expect(
        page.getByRole("tab", { name: words(lang, "explore.entities") }),
      ).toBeVisible({ timeout: 30_000 });
    },
  );
  if (mapOffered !== false)
    await state(
      page,
      {
        ...at("explore"),
        route: `/projects/${MAP_PROJECT}/explore`,
        state: "map",
      },
      async () => {
        const mapTab = page.getByRole("tab", {
          name: words(lang, "explore.map.tab"),
        });
        if (mapOffered === false) throw new Error("the map tab is not offered");
        for (const type of mapType ? [mapType] : MAP_TYPES) {
          await open(
            page,
            `/projects/${MAP_PROJECT}/explore?space=${encodeURIComponent(MAP_SPACE)}&type=${encodeURIComponent(type)}`,
            lang,
          );
          if (
            await mapTab.waitFor({ timeout: 15_000 }).then(
              () => true,
              () => false,
            )
          ) {
            mapType = type;
            break;
          }
        }
        mapOffered = mapType !== undefined;
        if (!mapOffered)
          throw new Error(
            `no type of ${MAP_SPACE} offers the map tab: ${MAP_TYPES.join(", ")}`,
          );
        await mapTab.click({ timeout: 5_000 });
        await page.waitForTimeout(2500);
      },
    );
  await state(
    page,
    { ...at("explore"), route: explore, state: "quality" },
    async () => {
      await open(page, typed, lang);
      await page
        .getByRole("tab", { name: words(lang, "explore.quality.tab") })
        .click();
    },
  );
  await state(
    page,
    { ...at("explore"), route: explore, state: "chart" },
    async () => {
      await open(page, typed, lang);
      await page
        .getByRole("button", {
          name: words(lang, "explore.chart.open"),
          exact: true,
        })
        .first()
        .click();
      await expect(page.getByRole("dialog")).toBeVisible();
    },
  );
  await state(
    page,
    { ...at("explore"), route: explore, state: "export" },
    async () => {
      await open(page, typed, lang);
      // The grid toolbar's Export menu, open on the Endpoint's formats (T-6107).
      const menu = page.locator("summary", { hasText: words(lang, "explore.export") }).first();
      await menu.scrollIntoViewIfNeeded();
      await menu.click();
      await expect(page.getByText(words(lang, "spaces.export.label")).first()).toBeVisible();
    },
  );
  await state(
    page,
    { ...at("explore"), route: explore, state: "empty-result" },
    async () => {
      // A valid query that matches nothing; a malformed one photographs the parser's refusal.
      await open(
        page,
        `${typed}&q=${encodeURIComponent('name=="qzxwv-nothing-is-called-this"')}`,
        lang,
      );
      await page.waitForTimeout(1500);
    },
  );

  // Knowledge, its sources and the assistant's admin form.
  await state(
    page,
    { ...at("knowledge"), route: `${P}/knowledge`, state: "list" },
    () => open(page, `${P}/knowledge`, lang),
  );
  await state(
    page,
    { ...at("knowledge"), route: `${P}/knowledge/$source`, state: "detail" },
    async () => open(page, await item(page, `${P}/knowledge`, lang), lang),
  );
  await form(
    page,
    at("knowledge"),
    `${P}/knowledgesources/new`,
    `${P}/knowledgesources/new`,
    lang,
    true,
  );
  await form(
    page,
    at("assistant-admin"),
    `${P}/assistantdeployments/new`,
    `${P}/assistantdeployments/new`,
    lang,
    true,
  );
  await state(
    page,
    {
      ...at("assistant-admin"),
      route: `${P}/assistantdeployments/$name/edit`,
      state: "form-open",
    },
    async () => {
      await open(page, `${P}/knowledge`, lang);
      const deployment = await firstItem(page, `${P}/assistantdeployments`);
      await open(page, `${deployment}/edit`, lang);
    },
  );
  await state(
    page,
    { ...at("assistant-admin"), route: `${P}/knowledge`, state: "chat-open" },
    async () => {
      await open(page, `${P}/knowledge`, lang);
      await page
        .getByRole("button", { name: words(lang, "knowledge.chat.ask") })
        .first()
        .click();
      await expect(page.getByRole("dialog")).toBeVisible();
    },
  );
  if (who === "steward" && lang === "sk" && width === 1440 && !asked) {
    asked = true;
    await state(
      page,
      {
        ...at("assistant-admin"),
        route: `${P}/knowledge`,
        state: "chat-answered",
      },
      async () => {
        await open(page, `${P}/knowledge`, lang);
        await page
          .getByRole("button", { name: words(lang, "knowledge.chat.ask") })
          .first()
          .click();
        const dialog = page.getByRole("dialog");
        await dialog
          .getByRole("textbox")
          .first()
          .fill("Kde nájdem zoznam nemocníc v kraji?");
        await dialog.getByRole("textbox").first().press("Enter");
        // One answer or the sentence of its refusal, whichever the service gives.
        await page.waitForTimeout(25_000);
      },
    );
  }

  // Dashboards and apps.
  for (const area of ["dashboards", "apps"] as const) {
    const list = `${P}/${area}`;
    await state(page, { ...at(area), route: list, state: "list" }, () =>
      open(page, list, lang),
    );
    await form(page, at(area), `${list}/new`, `${list}/new`, lang, true);
    await state(
      page,
      { ...at(area), route: `${list}/$name`, state: "detail" },
      async () => open(page, await item(page, list, lang), lang),
    );
  }
  await state(
    page,
    { ...at("apps"), route: `${P}/apps/$name/open`, state: "framed" },
    async () => {
      await open(page, `${await item(page, `${P}/apps`, lang)}/open`, lang);
      await page.locator("main iframe").first().waitFor({ timeout: 30_000 });
      await page.waitForTimeout(3000);
    },
  );

  // The glossary, and the Assistant page with a question typed, not sent. The component gallery
  // is built only in development (router.tsx `devRoutes`), so a deployed Portal answers it with 404.
  await state(
    page,
    { ...at("glossary"), route: "/glossary", state: "page" },
    () => open(page, "/glossary", lang),
  );
  await state(
    page,
    { ...at("assistant"), route: `${P}/assistant`, state: "page" },
    () => open(page, `${P}/assistant`, lang),
  );
  await state(
    page,
    { ...at("assistant"), route: `${P}/assistant`, state: "question-typed" },
    async () => {
      await open(page, `${P}/assistant`, lang);
      const bubble = page.getByRole("button", {
        name: words(lang, "assistant.open"),
      });
      if (await bubble.count()) await bubble.first().click();
      await page
        .locator("#assistant-empty-composer, #run-message")
        .first()
        .fill(
          lang === "sk"
            ? "Ktoré nemocnice má kraj?"
            : "Which hospitals does the region have?",
        );
    },
  );
}

// One test per person and width (20 to 35 minutes each): one person's three widths outran an
// hour, and `--grep "viewer at 768"` resumes a run that stopped. Not serial, so one test that
// fails leaves the next to run (the live config holds one worker).
test.setTimeout(2_700_000);

for (const [who, person] of [
  ["steward", STEWARD],
  ["viewer", VIEWER],
] as const)
  for (const width of WIDTHS)
    test(`spaces, catalogue, explore, knowledge, dashboards, apps, glossary and assistant as demo.${who} at ${width} px`, async ({
      browser,
    }) => {
      test.skip(!DIR, "SHOTS_DIR is not set");
      const { context, page } = await signIn(
        browser,
        person,
        `${P}/home?lang=en`,
      );
      // Read-only: the Portal's API sees GET and HEAD alone, but for the one question asked.
      await context.route("**/api/**", (route) => {
        const request = route.request();
        const method = request.method();
        const path = new URL(request.url()).pathname;
        const question = /\/knowledge\/deployments\/[^/]+\/chat$/.test(path);
        if (
          method === "GET" ||
          method === "HEAD" ||
          READS.test(path) ||
          (question && asked)
        )
          return route.continue();
        if (!blocked.has(`${method} ${path}`))
          console.log(`blocked ${method} ${path}`);
        blocked.add(`${method} ${path}`);
        return route.abort("blockedbyclient");
      });
      // Another origin (a basemap, a framed App's host) photographed blank with the journey marker
      // (worker-8, 2026-10-16). Registered last, so it runs first and falls through.
      await journeyMarkerOnPortalOnly(context, new URL(page.url()).host);
      const counted = (request: { resourceType(): string }) =>
        request.resourceType() !== "eventsource";
      page.on("request", (request) => {
        if (counted(request)) inFlight++;
      });
      const settled = (request: Request) => {
        if (counted(request)) inFlight = Math.max(0, inFlight - 1);
      };
      page.on("requestfinished", settled);
      page.on("requestfailed", settled);
      page.on("response", (response) => {
        if (response.status() === 429)
          refused.push(`429 ${new URL(response.url()).pathname}`);
      });
      page.on("requestfailed", (request) => {
        if (request.resourceType() === "script")
          refused.push(`script ${new URL(request.url()).pathname}`);
      });
      inFlight = 0;
      try {
        await page.setViewportSize({ width, height: width < 768 ? 812 : 900 });
        for (const lang of LANGS) await walk(page, who, lang);
        walked++;
      } finally {
        await context.close();
      }
    });

test.afterAll(() => {
  if (!DIR) return;
  const count = writeIndex(DIR);
  console.log(
    `UX photo audit, spaces to assistant: ${count} screenshots in ${DIR}`,
  );
  if (missed.length)
    console.log(`not reached (${missed.length}):\n${missed.join("\n")}`);
  // A detail route whose list holds nothing (a project with no dashboard) has nothing to show.
  const empty = new Set(
    missed
      .filter((line) => line.includes("no item linked"))
      .map((line) => line.split(" ")[0]),
  );
  if (empty.size)
    console.log(`lists with nothing to open:\n${[...empty].join("\n")}`);
  const unshot = ROUTES.filter(
    (route) => !shotRoutes.has(route) && !empty.has(route),
  );
  if (unshot.length)
    console.log(`routes with no picture:\n${unshot.join("\n")}`);
  // A re-shoot of some states, or of some people and widths (`--grep`), adds to the folder of a
  // full run; the full run is what is checked.
  if (ONLY || walked < 2 * WIDTHS.length) return;
  expect(unshot, "every route of the audit has at least one picture").toEqual(
    [],
  );
  expect(count).toBeGreaterThanOrEqual(90);
});

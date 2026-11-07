/**
 * T-4159 — UX photo audit 3/7: every form and list of pipelines (the studio canvas included),
 * flows, data sources, import, MQTT brokers, sync sources and data models. Per module: the list
 * filled and empty (the empty one from a project that has none), each create and edit form as it
 * opens and after its submit is pressed on the form as it opened, the flow wizard of every
 * blueprint, the row menus and what each of their entries opens, the model's views, and the
 * viewer's view of the lists and forms.
 *
 * Every state at 375, 768 and 1440 px, in sk and en, saved by `shot` (shoot.ts, T-4156) under
 * `$SHOTS_DIR`, which then gets its contact sheet. Read-only by construction: after sign-in every
 * request to the Portal's API that is not a GET is refused in the browser before it leaves, the
 * one read that is a POST (a pipeline's sample of its source) excepted, so a pressed submit, a
 * menu entry or a draft saved on open writes nothing. Each refusal is printed at the end.
 * Each state waits for the network to go quiet and is retaken after the edge's minute when any
 * request met a 429 or a script failed to load; a blank page fails the state instead of saving
 * white. Run one audit at a time on the server (`mkdir tasks/.audit.lock`).
 *
 * The modules of these folders in tests/controls.json are the checklist: each is named by the
 * state that renders it (`MODULES`), and the run fails when a module is missing from that map or
 * its state was not reached, and on fewer than 90 shots.
 *
 *   SHOTS_DIR=/workspace/evidence/<agent>/T-4159 PORTAL_PASSWORD=… VIEWER_PASSWORD=… \
 *     npx playwright test --config playwright.live.config.ts audit-forms-data
 */
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { STEWARD, VIEWER, signIn } from "./portal";
import { shot, writeIndex } from "./shoot";
import type { Shot } from "./shoot";

const here = dirname(fileURLToPath(import.meta.url));
const LANGS = ["sk", "en"] as const;
type Lang = (typeof LANGS)[number];
const WIDTHS = [375, 768, 1440];
const PROJECT = "helsinki";
const P = `/projects/${PROJECT}`;
const AREA = "forms-data";
const DIR = process.env.SHOTS_DIR;
const FOLDERS = ["pipelines", "flows", "datasources", "import", "mqttbrokers", "sync", "models"];
/** The sections of these modules that list resources at `/projects/{project}/{plural}`. */
const SECTIONS = ["pipelines", "datasources", "mqttbrokers", "syncsources"];
/** A POST that only reads: the studio's sample of what the pipeline's source answers now. */
const READS = [/\/ops\/jc_pipeline_sample_source$/];

const locales = Object.fromEntries(
  LANGS.map((lang) => [lang, JSON.parse(readFileSync(join(here, `../../src/locales/${lang}.json`), "utf8")) as Record<string, unknown>]),
) as Record<Lang, Record<string, unknown>>;

function words(lang: Lang, key: string): string {
  const value = key.split(".").reduce<unknown>((node, part) => (node as Record<string, unknown>)[part], locales[lang]);
  if (typeof value !== "string") throw new Error(`no ${lang} text at ${key}`);
  return value;
}

/**
 * Where each module with controls is seen: the `state` of a shot that renders it (T-2136's
 * inventory). A module added to these folders fails the run until it is named here.
 */
const MODULES: Record<string, string> = {
  "src/pages/datasources/DataSourcesPage.tsx": "datasources-list",
  "src/pages/datasources/TemplateGallery.tsx": "datasources-new",
  "src/pages/flows/Gallery.tsx": "flows-gallery",
  "src/pages/flows/Instantiate.tsx": "flows-set-up-1",
  "src/pages/import/ImportPage.tsx": "import",
  "src/pages/models/ModelsList.tsx": "models-list",
  "src/pages/models/ModelsPage.tsx": "models-list",
  "src/pages/models/ModelPage.tsx": "model",
  "src/pages/models/ModelViews.tsx": "model",
  "src/pages/models/MappingsEditor.tsx": "model",
  "src/pages/models/ShareModel.tsx": "model",
  "src/pages/models/LinkmlVisualEditor.tsx": "models-new-blank-tab-2",
  "src/pages/models/RelationshipEditor.tsx": "models-new-blank-tab-2",
  "src/pages/models/ModelSubsetPicker.tsx": "models-new-blank-tab-2",
  "src/pages/models/LinkmlGraphView.tsx": "models-new-blank-tab-5",
  "src/pages/models/DiagramEditing.tsx": "models-new-blank-tab-5",
  "src/pages/models/LinkmlPreviewPanel.tsx": "models-new-blank-tab-6",
  "src/pages/models/ModelFileDrop.tsx": "models-new-file",
  "src/pages/models/SmartDataModelsImport.tsx": "models-new-sdm",
  "src/pages/pipelines/PipelineTemplates.tsx": "pipelines-new",
  "src/pages/pipelines/RecipeGallery.tsx": "pipelines-new",
  "src/pages/pipelines/PipelineWorkbench.tsx": "pipelines-edit",
  "src/pages/pipelines/PipelineStudio.tsx": "pipelines-edit",
  "src/pages/pipelines/PipelineFlow.tsx": "pipelines-edit",
  "src/pages/pipelines/PipelineTest.tsx": "pipelines-edit",
  "src/pages/pipelines/DebugPanel.tsx": "pipelines-edit",
  "src/pages/pipelines/SlotUnit.tsx": "pipelines-edit",
  "src/pages/pipelines/OutputMapper.tsx": "pipelines-edit",
  "src/pages/pipelines/SpaceChanges.tsx": "pipelines-edit",
  "src/pages/pipelines/PipelineRuns.tsx": "pipelines-menu-1",
  "src/pages/pipelines/PipelineRejected.tsx": "pipelines-menu-2",
  "src/pages/sync/SyncSourcesPage.tsx": "syncsources-list",
};

const reached = new Set<string>();
const missed: string[] = [];
const refused = new Set<string>();
/**
 * Set by a 429 or a script that failed to load. The edge allows one address 300 requests a
 * minute and every agent on the server shares one; a page whose chunks were refused paints white
 * or half, so the state is retaken after the window instead of photographed (supervisor, T-4159).
 */
let limited = false;
/** When each request to the Portal left this browser, for `pace`. */
const sent: number[] = [];

/**
 * The walk loads pages faster than a person: before each state it waits until the last minute
 * holds fewer than 200 of its requests, a page's worth under the edge's 300 (T-4159 run 1 got
 * 989 refusals without this).
 */
async function pace(page: Page): Promise<void> {
  for (;;) {
    const now = Date.now();
    while (sent.length && sent[0] < now - 61_000) sent.shift();
    if (sent.length < 200) return;
    await page.waitForTimeout(sent[0] + 61_000 - now);
  }
}

/** Waits for the page a person looks at: a heading, nothing busy, then a beat for late panels. */
async function settle(page: Page): Promise<void> {
  // The Portal keeps a stream open, so the network rarely goes fully quiet: wait a little, not forever.
  await page.waitForLoadState("networkidle", { timeout: 5_000 }).catch(() => undefined);
  await page.locator("h1:visible").first().waitFor({ timeout: 30_000 }).catch(() => undefined);
  await expect(page.locator('[aria-busy="true"]')).toHaveCount(0, { timeout: 20_000 }).catch(() => undefined);
  await page.waitForTimeout(1500);
}

async function open(page: Page, address: string, lang: Lang): Promise<void> {
  await page.goto(`${address}${address.includes("?") ? "&" : "?"}lang=${lang}`, { waitUntil: "load" });
  await settle(page);
}

async function state(page: Page, what: Omit<Shot, "width" | "area">, reach: () => Promise<void>): Promise<void> {
  try {
    for (let attempt = 0; ; attempt++) {
      await pace(page);
      limited = false;
      // A control that never appears because its chunk was refused is the limit too, not the page.
      const failed = await reach().then(
        () => undefined,
        (error: unknown) => error,
      );
      if (failed && !limited) throw failed;
      if (!failed) await settle(page);
      if (!limited) break;
      if (attempt === 2) throw new Error("requests still refused by the edge after three tries");
      await page.keyboard.press("Escape").catch(() => undefined);
      await page.waitForTimeout(61_000);
    }
    // A submit scrolls to the first error; a full-page picture taken there paints the sticky
    // header over the middle of the form. Back at the top, the header is where a person sees it.
    await page.evaluate(() => window.scrollTo(0, 0));
    // A blank page (dev restarting during an apply) is no picture of the Portal.
    if (!(await page.locator("body").innerText()).trim()) throw new Error("the page is blank");
    await shot(page, { area: AREA, ...what });
    reached.add(what.state);
  } catch (error) {
    missed.push(`${what.route} ${what.state} ${what.who} ${what.lang} ${page.viewportSize()?.width}: ${String(error).split("\n")[0]}`);
  } finally {
    await page.keyboard.press("Escape").catch(() => undefined);
  }
}

/** The form's submit: the footer's Propose change (or Save), the last one visible. */
function submit(page: Page) {
  return page.locator('button[data-shortcut="save"]:visible, form button[type="submit"]:visible').last();
}

async function names(page: Page, plural: string, project = PROJECT): Promise<string[] | undefined> {
  const answer = await page.request.get(`/api/v1/projects/${project}/${plural}`);
  if (!answer.ok()) return undefined;
  return (((await answer.json()) as { items?: { metadata?: { name?: string } }[] }).items ?? []).map((item) => item.metadata?.name ?? "").filter(Boolean);
}

/** The first project the person can open whose list of `plural` is empty, for the empty state. */
async function emptyProject(page: Page, plural: string): Promise<string | undefined> {
  const answer = await page.request.get("/api/v1/projects");
  if (!answer.ok()) return undefined;
  const body = (await answer.json()) as { items?: { metadata?: { name?: string } }[] };
  for (const project of (body.items ?? []).map((item) => item.metadata?.name ?? "")) {
    if ((await names(page, plural, project))?.length === 0) return project;
  }
  return undefined;
}

interface Plan {
  filled: Record<string, string | undefined>;
  empty: Record<string, string | undefined>;
  model?: string;
}

async function plan(page: Page): Promise<Plan> {
  const filled: Plan["filled"] = {};
  const empty: Plan["empty"] = {};
  for (const plural of SECTIONS) {
    filled[plural] = (await names(page, plural))?.[0];
    empty[plural] = await emptyProject(page, plural);
  }
  // A model's address is its space's name; the list links to each one.
  await page.goto(`${P}/models?lang=en`, { waitUntil: "load" });
  await settle(page);
  const model = await page
    .locator(`main a[href^="${P}/models/"]`)
    .evaluateAll((links) => links.map((link) => link.getAttribute("href") ?? "").find((href) => /^\/projects\/[^/]+\/models\/[^/?]+$/.test(href)));
  return { filled, empty, model: model ?? undefined };
}

/** Each entry of the first row's menu, and what pressing it opens. */
async function rowMenu(page: Page, base: Omit<Shot, "width" | "area" | "route" | "state">, plural: string, lang: Lang) {
  const list = `${P}/${plural}`;
  // The row's menu is named after its label, which may be a title rather than `name`: the first row's.
  const prefix = words(lang, "rowActions.more").split("{name}")[0];
  const more = () => page.getByRole("button", { name: new RegExp(`^${prefix.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`) }).first();
  await state(page, { ...base, route: list, state: `${plural}-menu` }, async () => {
    await open(page, list, lang);
    await more().click();
    await expect(page.getByRole("menu")).toBeVisible();
  });
  let count = 0;
  for (let attempt = 0; ; attempt++) {
    try {
      await pace(page);
      limited = false;
      await open(page, list, lang);
      await more().click();
      count = await page.getByRole("menuitem").count();
      break;
    } catch (error) {
      if (!limited || attempt === 2) {
        missed.push(`${list} ${plural}-menu entries ${base.who} ${lang} ${page.viewportSize()?.width}: ${String(error).split("\n")[0]}`);
        break;
      }
      await page.waitForTimeout(61_000);
    }
  }
  await page.keyboard.press("Escape");
  for (let i = 0; i < count; i++) {
    await state(page, { ...base, route: list, state: `${plural}-menu-${i + 1}` }, async () => {
      await open(page, list, lang);
      await more().click();
      const entry = page.getByRole("menuitem").nth(i);
      // A refused entry is photographed hovered, where its reason shows, instead of pressed.
      if ((await entry.getAttribute("aria-disabled")) === "true" || (await entry.getAttribute("data-disabled")) !== null) await entry.hover();
      else await entry.click();
    });
  }
}

async function walkSteward(page: Page, lang: Lang, at: Plan): Promise<void> {
  const base = { who: "steward" as const, lang };

  for (const plural of SECTIONS) {
    const list = `${P}/${plural}`;
    await state(page, { ...base, route: list, state: `${plural}-list` }, () => open(page, list, lang));
    const empty = at.empty[plural];
    if (empty) {
      await state(page, { ...base, route: `/projects/${empty}/${plural}`, state: `${plural}-empty` }, () => open(page, `/projects/${empty}/${plural}`, lang));
    }
    await state(page, { ...base, route: `${list}/new`, state: `${plural}-new` }, () => open(page, `${list}/new`, lang));
    await state(page, { ...base, route: `${list}/new`, state: `${plural}-new-submitted` }, async () => {
      await open(page, `${list}/new`, lang);
      await submit(page).click();
    });
    const name = at.filled[plural];
    if (!name) continue;
    const edit = `${list}/${encodeURIComponent(name)}/edit`;
    await state(page, { ...base, route: `${list}/$name/edit`, state: `${plural}-edit` }, () => open(page, edit, lang));
    await rowMenu(page, base, plural, lang);
  }

  // The studio: each of the editor's views of one pipeline, the Flow canvas among them.
  const pipeline = at.filled.pipelines;
  if (pipeline) {
    const edit = `${P}/pipelines/${encodeURIComponent(pipeline)}/edit`;
    await open(page, edit, lang);
    const tabs = await page.locator("main [role=tab]:visible").count();
    for (let i = 0; i < tabs; i++) {
      await state(page, { ...base, route: `${P}/pipelines/$name/edit`, state: `pipelines-edit-tab-${i + 1}` }, async () => {
        await open(page, edit, lang);
        await page.locator("main [role=tab]:visible").nth(i).click();
      });
    }
  }

  // The ready-made set-ups on Data sources (the Flows page went, T-6096), then the wizard of each
  // blueprint as it opens and after its submit.
  await state(page, { ...base, route: `${P}/datasources`, state: "flows-gallery" }, () => open(page, `${P}/datasources`, lang));
  const setUp = page.getByRole("button", { name: words(lang, "flows.run"), exact: true });
  const blueprints = await setUp.count();
  for (let i = 0; i < blueprints; i++) {
    await state(page, { ...base, route: `${P}/datasources`, state: `flows-set-up-${i + 1}` }, async () => {
      await open(page, `${P}/datasources`, lang);
      await setUp.nth(i).click();
    });
    await state(page, { ...base, route: `${P}/datasources`, state: `flows-set-up-${i + 1}-submitted` }, async () => {
      await open(page, `${P}/datasources`, lang);
      await setUp.nth(i).click();
      await settle(page);
      await submit(page).click();
    });
  }

  await state(page, { ...base, route: `${P}/import`, state: "import" }, () => open(page, `${P}/import`, lang));

  // Models: the list, the three ways to start one (each view of the blank editor), one model.
  await state(page, { ...base, route: `${P}/models`, state: "models-list" }, () => open(page, `${P}/models`, lang));
  for (const start of ["file", "sdm"]) {
    await state(page, { ...base, route: `${P}/models?new=${start}`, state: `models-new-${start}` }, () => open(page, `${P}/models?new=${start}`, lang));
  }
  await open(page, `${P}/models?new=blank`, lang);
  const views = await page.locator("main [role=tab]:visible").count();
  for (let i = 0; i < views; i++) {
    await state(page, { ...base, route: `${P}/models?new=blank`, state: `models-new-blank-tab-${i + 1}` }, async () => {
      await open(page, `${P}/models?new=blank`, lang);
      await page.locator("main [role=tab]:visible").nth(i).click();
    });
  }
  if (at.model) await state(page, { ...base, route: `${P}/models/$name`, state: "model" }, () => open(page, at.model as string, lang));
}

/** The viewer: the same lists, forms and model, where every write control says why it is off. */
async function walkViewer(page: Page, lang: Lang, at: Plan): Promise<void> {
  const base = { who: "viewer" as const, lang };
  for (const plural of SECTIONS) {
    const list = `${P}/${plural}`;
    await state(page, { ...base, route: list, state: `${plural}-list` }, () => open(page, list, lang));
    await state(page, { ...base, route: `${list}/new`, state: `${plural}-new` }, () => open(page, `${list}/new`, lang));
    const name = at.filled[plural];
    if (name) {
      await state(page, { ...base, route: `${list}/$name/edit`, state: `${plural}-edit` }, () => open(page, `${list}/${encodeURIComponent(name)}/edit`, lang));
    }
  }
  await state(page, { ...base, route: `${P}/datasources`, state: "flows-gallery" }, () => open(page, `${P}/datasources`, lang));
  await state(page, { ...base, route: `${P}/import`, state: "import" }, () => open(page, `${P}/import`, lang));
  await state(page, { ...base, route: `${P}/models`, state: "models-list" }, () => open(page, `${P}/models`, lang));
  if (at.model) await state(page, { ...base, route: `${P}/models/$name`, state: "model" }, () => open(page, at.model as string, lang));
}

test.describe.configure({ mode: "serial" });
// One test per person and width, so a slow width cannot starve the rest of their hour; and a
// control that never becomes clickable costs ten seconds, not thirty.
test.setTimeout(3_600_000);
// A hung navigation fails its one state in 45 s, not the test's hour (T-4285: T-4160's viewer
// 768 waited 20 min in one page.goto).
test.use({ actionTimeout: 10_000, navigationTimeout: 45_000 });

for (const [who, person] of [
  ["steward", STEWARD],
  ["viewer", VIEWER],
] as const) {
  for (const width of WIDTHS) {
    test(`pipelines, flows, data sources, import, MQTT, sync and models as demo.${who} at ${width}`, async ({ browser }) => {
      test.skip(!DIR, "SHOTS_DIR is not set");
      const { context, page } = await signIn(browser, person, `${P}/spaces?lang=en`);
      page.on("request", () => {
        sent.push(Date.now());
      });
      page.on("response", (answer) => {
        if (answer.status() === 429) limited = true;
      });
      page.on("requestfailed", (request) => {
        if (request.resourceType() === "script" && request.failure()?.errorText !== "net::ERR_BLOCKED_BY_CLIENT") limited = true;
      });
      await page.route("**/api/v1/**", (route) => {
        const request = route.request();
        const url = new URL(request.url());
        // A dry run (the delete and save-as dialogs' preview of what would change) persists nothing.
        if (request.method() === "GET" || url.searchParams.get("dryRun") === "All" || READS.some((read) => read.test(url.pathname))) {
          return route.continue();
        }
        refused.add(`${request.method()} ${new URL(request.url()).pathname}`);
        return route.abort("blockedbyclient");
      });
      try {
        const at = await plan(page);
        await page.setViewportSize({ width, height: width < 768 ? 812 : 900 });
        for (const lang of LANGS) await (who === "steward" ? walkSteward : walkViewer)(page, lang, at);
      } finally {
        await context.close();
      }
    });
  }
}

test.afterAll(() => {
  if (!DIR) return;
  const count = writeIndex(DIR);
  const shots = readdirSync(DIR).filter((file) => file.startsWith(`${AREA}-`) && file.endsWith(".png")).length;
  console.log(`UX photo audit, data forms: ${shots} screenshots in ${DIR} (${count} in the folder)`);
  console.log(`writes refused in the browser (${refused.size}):\n${[...refused].sort().join("\n")}`);
  if (missed.length) console.log(`not reached (${missed.length}):\n${missed.join("\n")}`);
  const inventory = (JSON.parse(readFileSync(join(here, "../../tests/controls.json"), "utf8")) as { modules: Record<string, unknown> }).modules;
  const modules = Object.keys(inventory).filter((module) => FOLDERS.some((folder) => module.startsWith(`src/pages/${folder}/`)));
  expect(modules.filter((module) => !(module in MODULES)), "every module of these folders is named in MODULES").toEqual([]);
  expect(
    modules.filter((module) => !reached.has(MODULES[module])).map((module) => `${module} (${MODULES[module]})`),
    "every module's state is photographed",
  ).toEqual([]);
  expect(missed, "every state is reached").toEqual([]);
  expect(shots).toBeGreaterThanOrEqual(90);
});

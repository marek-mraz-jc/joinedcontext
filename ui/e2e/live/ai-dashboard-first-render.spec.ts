/**
 * T-6403 (AP-48, T-6384): the app-builder eval of the bike prompt, timed on dev. One prompt, one
 * Endpoint: `AI_DASHBOARD_RUNS` builder runs (default 1), each from Generate to the first map drawn
 * in the preview, then what the App is built from, what the run cost, and a deny case: the App asks
 * for a type its Endpoint does not serve.
 *
 * A run is paid, so this spec runs only with JC_LIVE_MODEL_RUN=1. `AI_DASHBOARD_DIR` receives
 * report.json and the 1440 + 400 px shots.
 *
 * `AI_DASHBOARD_VIDEO=<dir>` (with `-g recording`, T-6385): the slide 12 recording, one desktop and
 * one phone take in one Playwright run, clock in the frame: prompt, first dashboard, two edits asked
 * of the assistant. Each take writes <take>.webm, <take>.json (times, the empty count beside a
 * direct NGSI-LD count of the same minute, the conversation) and <take>-conversation.png.
 * `AI_DASHBOARD_FILM=1` ends a take at the first dashboard (T-6493): one paid run, no edits.
 *
 * Every wait is on the thing the step needs, never on the network going idle (T-2452).
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { FrameLocator, Page } from "@playwright/test";
import { shareEdge } from "./edgeShare";
import { STEWARD, ask, csrf, signIn } from "./portal";
import { shot } from "./shoot";

const PROJECT = process.env.AI_DASHBOARD_PROJECT ?? "helsinki";
const ENDPOINT = process.env.AI_DASHBOARD_ENDPOINT ?? "helsinki-bikes";
const RUNS = Number(process.env.AI_DASHBOARD_RUNS ?? "1");
const DIR = process.env.AI_DASHBOARD_DIR ?? "test-results/ai-dashboard";
const SUFFIX = Date.now().toString(36);
const PROMPT = "Map the shared bike stations, colour-coded by occupancy, and count the empty ones";
const MAP = "canvas.maplibregl-canvas";
/** Share of the canvas: dev's two T-6434 maps read 0.93 and 1.0 painted, a blank one 0. */
const PAINTED = 0.25;
/** A type no bike Endpoint serves: the App's grant lacks it. */
const DENIED_PROMPT = "List the air quality stations of the city with their PM10 values";
const VIDEO = process.env.AI_DASHBOARD_VIDEO;
const FILM = process.env.AI_DASHBOARD_FILM === "1";
const FILTER_LABEL = "Only empty stations";
const COLUMN_LABEL = "Free dock share";
const FILTER_EDIT = `On the first page, add a checkbox "${FILTER_LABEL}" above the map: when ticked, the page shows only stations with 0 available bikes.`;
const COLUMN_EDIT = `Add a computed column "${COLUMN_LABEL}" to the station table: freeSlotNumber / totalSlotNumber * 100, as a percentage.`;
const TAKES = [
  { take: "desktop", viewport: { width: 1440, height: 810 } },
  { take: "phone", viewport: { width: 400, height: 800 } },
];

test.setTimeout(1_200_000);
test.skip(process.env.JC_LIVE_MODEL_RUN !== "1", "a live model run is paid: JC_LIVE_MODEL_RUN=1 with the owner's go");
test.describe.configure({ mode: "serial" });

interface RunEvent {
  kind: string;
  payload: Record<string, unknown>;
}

interface Measured {
  name: string;
  run: string;
  /** Generate to the map painted in the preview, in seconds; null when it never painted. */
  firstRenderS: number | null;
  /** Generate to the run waiting for the person, tests included. */
  waitingS: number | null;
  costUsd: number;
  tokens: number;
  calls: number;
  models: string[];
  /** Seconds from the run's start per event kind's first sight, for where the minute goes. */
  phases: Record<string, number>;
  usesEntityGrid: boolean;
  formulaColumn: boolean;
  handTable: boolean;
  sampleLost: boolean;
  preview: string;
  note: string;
  /** Direct NGSI-LD counts of the minute the first render was shot, beside the App's own text. */
  direct: { slug: string; total: number | null; empty: number | null; noReading: number | null } | null;
  firstRenderText: string;
}

const results: Measured[] = [];

/** Every event of a run so far: the stream replays them, then goes quiet. */
async function runEvents(page: Page, id: string): Promise<RunEvent[]> {
  return page.evaluate(
    async ({ url, kinds }) => {
      const events: { kind: string; payload: unknown }[] = [];
      await new Promise<void>((resolve) => {
        const source = new EventSource(url);
        let quiet = setTimeout(resolve, 8_000);
        const take = (message: MessageEvent<string>) => {
          events.push({ kind: message.type, payload: JSON.parse(message.data) as unknown });
          clearTimeout(quiet);
          quiet = setTimeout(() => {
            source.close();
            resolve();
          }, 2_000);
        };
        for (const kind of kinds) source.addEventListener(kind, take);
      });
      return events;
    },
    { url: `/api/v1/projects/${PROJECT}/agent-runs/${id}/events`, kinds: ["status", "message", "thought", "tool", "commit", "preview", "tests", "usage", "app-build"] },
  ) as Promise<RunEvent[]>;
}

/** Start of each phase, read from the frames' own timestamps (RunTimeSpent reads the same). */
function phases(events: RunEvent[]): Record<string, number> {
  const stamp = (event: RunEvent) => Date.parse(String(event.payload.timestamp ?? ""));
  const stamped = events.filter((event) => !Number.isNaN(stamp(event)));
  const first = stamped[0] ? stamp(stamped[0]) : 0;
  const seen: Record<string, number> = {};
  for (const event of stamped) {
    const key = event.kind === "status" ? `status:${String(event.payload.status)}` : event.kind;
    seen[key] ??= (stamp(event) - first) / 1000;
  }
  return seen;
}

/** Starts one builder run through the form a person uses; answers the run's id and when Generate was pressed. */
async function generate(page: Page, name: string, prompt: string, label: string, pressing = (): Promise<void> => both(page, label, "prompt")): Promise<{ id: string; pressed: number }> {
  await page.goto(`/projects/${PROJECT}/apps?lang=en`, { waitUntil: "load" });
  await page.getByRole("button", { name: "Generate your own app" }).first().click();
  await page.getByLabel("Endpoint").first().selectOption(ENDPOINT);
  await page.getByLabel("What should the app do?").first().fill(prompt);
  await page.getByText("Details:", { exact: false }).first().click();
  await page.getByLabel("App name").fill(name);
  await pressing();
  // The body is read as the answer lands: the browser drops it once the page moves to the App (T-6441).
  const started = page
    .waitForResponse((answer) => answer.request().method() === "POST" && answer.url().endsWith(`/projects/${PROJECT}/agent-runs`))
    .then(async (answer) => ({ status: answer.status(), body: await answer.text() }));
  const pressed = Date.now();
  await page.evaluate((at) => sessionStorage.setItem("clock-start", String(at)), pressed);
  await page.getByRole("button", { name: "Generate the app" }).click();
  const answer = await started;
  expect(answer.status, answer.body).toBeLessThan(300);
  const { id } = JSON.parse(answer.body) as { id: string };
  await page.waitForURL(new RegExp(`/projects/${PROJECT}/apps/${name}`), { timeout: 60_000 });
  return { id, pressed };
}

/**
 * One shot at 1440 and 400 px, back at 1440 afterwards. At 400 px the open dock covers the page, so
 * a built preview is shot with the dock hidden and in view, as a person reaches it (T-6493).
 */
async function both(page: Page, label: string, state: string): Promise<void> {
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    const pane = page.locator("iframe").first();
    if (width === 400 && (await pane.count())) {
      await page.getByRole("button", { name: "Hide the assistant" }).click();
      await pane.scrollIntoViewIfNeeded();
    }
    await shot(page, { area: "ai-dashboard", route: `/projects/${PROJECT}/apps`, state: `${label}-${state}`, who: "steward", lang: "en" }, DIR);
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

/** Cancels the run, so the organization's two builder slots stay free for the next. */
async function cancel(page: Page, id: string): Promise<void> {
  const headers = { "x-csrf-token": await csrf(page.context()) };
  await page.request.post(`/api/v1/projects/${PROJECT}/agent-runs/${id}/cancel`, { headers });
}

/**
 * Share of the map's pixels that differ from its corner: none on a blank map. The App's map keeps
 * its drawing buffer (EntityMap), so a 2D copy reads what a person sees; the App needs no hook.
 */
async function paintedPixels(preview: FrameLocator): Promise<number> {
  return preview.locator(MAP).first().evaluate((canvas: HTMLCanvasElement) => {
    const copy = document.createElement("canvas");
    copy.width = canvas.width;
    copy.height = canvas.height;
    const context = copy.getContext("2d", { willReadFrequently: true });
    if (!context || !copy.width || !copy.height) return 0;
    context.drawImage(canvas, 0, 0);
    const { data } = context.getImageData(0, 0, copy.width, copy.height);
    let painted = 0;
    for (let i = 0; i < data.length; i += 4) {
      if (data[i] !== data[0] || data[i + 1] !== data[1] || data[i + 2] !== data[2]) painted += 1;
    }
    return painted / (copy.width * copy.height);
  });
}

/** The App's own modules: the preview serves them as `data:` URLs in its import map. */
function sourceOf(html: string): Record<string, string> {
  const map = /<script type="importmap">(.*?)<\/script>/s.exec(html)?.[1] ?? "{}";
  const imports = (JSON.parse(map) as { imports?: Record<string, string> }).imports ?? {};
  return Object.fromEntries(
    Object.entries(imports)
      .filter(([name, url]) => name.startsWith("@app/src/") && url.startsWith("data:text/javascript,"))
      .map(([name, url]) => [name, decodeURIComponent(url.slice("data:text/javascript,".length))]),
  );
}

for (let round = 1; round <= RUNS; round += 1) {
  test(`run ${round} of ${RUNS}: the bike prompt draws its map`, async ({ browser }) => {
    const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
    const name = `t6403-bikes-${SUFFIX}-${round}`;
    const label = `run${round}`;
    const { id, pressed } = await generate(page, name, PROMPT, label);
    const preview = page.frameLocator("iframe").first();
    // The map stands, its rows arrived and its dots are on the canvas: a canvas beside a `Loading`
    // line is not a dashboard yet, and neither is an empty one (T-6441).
    let firstRenderS: number | null = null;
    let note = "";
    let direct: Measured["direct"] = null;
    let firstRenderText = "";
    try {
      await expect(preview.locator(MAP)).toBeVisible({ timeout: 300_000 });
      // A basemap paints before the rows land (run 7 of T-6441): the template's tiles hold "…" until they do.
      await expect(preview.locator(".jc-loading, [aria-busy=true]")).toHaveCount(0, { timeout: 120_000 });
      await expect(preview.getByText("…", { exact: true })).toHaveCount(0, { timeout: 120_000 });
      await expect.poll(() => paintedPixels(preview), { timeout: 120_000 }).toBeGreaterThanOrEqual(PAINTED);
      firstRenderS = (Date.now() - pressed) / 1000;
      const { slug } = JSON.parse((await preview.locator("#jc-config").textContent()) ?? "{}") as { slug: string };
      direct = {
        slug,
        total: await directCount(page, slug),
        empty: await directCount(page, slug, "availableBikeNumber==0"),
        noReading: await directCount(page, slug, "!availableBikeNumber"),
      };
      firstRenderText = await preview.locator("body").innerText();
      await both(page, label, "first-render");
    } catch (error) {
      note = String(error).split("\n")[0] ?? "";
      await both(page, label, "no-render");
    }
    let waitingS: number | null = null;
    await expect(page.getByTestId("run-progress").filter({ hasText: "Waiting for you" }))
      .toBeVisible({ timeout: 300_000 })
      .then(() => {
        waitingS = (Date.now() - pressed) / 1000;
      })
      .catch(() => undefined);
    const events = await runEvents(page, id);
    const usage = events.filter((event) => event.kind === "usage").map((event) => event.payload);
    const frame = page.frames().find((each) => each !== page.mainFrame());
    const html = frame ? await frame.content() : "";
    writeFileSync(join(DIR, `${label}-events.json`), `${JSON.stringify(events, null, 1)}\n`);
    const modules = sourceOf(html);
    // What the model chose: the template's components are files of the App whether used or not.
    const written = Object.entries(modules).filter(([file]) => !file.includes("/components/"));
    const source = written.map(([, text]) => text).join("\n");
    writeFileSync(join(DIR, `${label}-source.json`), `${JSON.stringify(modules, null, 1)}\n`);
    results.push({
      name,
      run: id,
      firstRenderS,
      waitingS,
      costUsd: usage.reduce((sum, each) => sum + (typeof each.costUsd === "number" ? each.costUsd : 0), 0),
      tokens: usage.reduce((sum, each) => sum + Number(each.tokensThisStep ?? 0), 0),
      calls: usage.length,
      models: [...new Set(usage.map((each) => String(each.model ?? "")))],
      phases: phases(events),
      usesEntityGrid: /components\/TypeGrid|@joinedcontext\/sdk\/grid/.test(source),
      formulaColumn: /formula:/.test(source),
      handTable: /_jsxs?\("table"/.test(source),
      sampleLost: events.some((event) => event.kind === "thought" && String(event.payload.text).startsWith("No sample of")),
      preview: new URL(page.url()).pathname,
      note,
      direct,
      firstRenderText,
    });
    await cancel(page, id);
    await context.close();
    expect(firstRenderS, note).not.toBeNull();
  });
}

/** The running clock of the recording, in the top page only; `clockMark` adds what it reached. */
function clockOverlay(): void {
  if (window.top !== window) return;
  const clock = document.createElement("div");
  clock.setAttribute("aria-hidden", "true");
  clock.style.cssText =
    "position:fixed;left:50%;top:4px;transform:translateX(-50%);z-index:2147483647;pointer-events:none;display:none;" +
    "padding:6px 12px;border-radius:8px;background:rgba(18,18,18,.88);color:#fff;font:600 18px/1.3 ui-monospace,Menlo,Consolas,monospace;white-space:nowrap";
  const time = (ms: number) => {
    const s = Math.floor(ms / 1000);
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  };
  setInterval(() => {
    if (!clock.isConnected) document.documentElement.append(clock);
    const start = Number(sessionStorage.getItem("clock-start"));
    if (!start) return;
    const marks = JSON.parse(sessionStorage.getItem("clock-marks") ?? "[]") as [string, number][];
    clock.style.display = "block";
    clock.textContent = [time(Date.now() - start), ...marks.map(([what, at]) => `${what} ${time(at - start)}`)].join("  ·  ");
  }, 200);
}

async function clockMark(page: Page, what: string): Promise<number> {
  return page.evaluate((mark) => {
    const at = Date.now();
    const marks = JSON.parse(sessionStorage.getItem("clock-marks") ?? "[]") as [string, number][];
    sessionStorage.setItem("clock-marks", JSON.stringify([...marks, [mark, at]]));
    return at;
  }, what);
}

/** `ngsild-results-count` of a query through the App's own Endpoint. */
async function directCount(page: Page, slug: string, q?: string): Promise<number | null> {
  const query = new URLSearchParams({ type: "BikeHireDockingStation", limit: "0", count: "true", ...(q ? { q } : {}) });
  const answer = await page.request.get(`/api/endpoint/${slug}/ngsi-ld/v1/entities?${query.toString()}`);
  const count = answer.headers()["ngsild-results-count"];
  return count === undefined ? null : Number(count);
}

/** Asks for one edit and waits for the preview to show it. */
async function edit(page: Page, words: string, shown: string): Promise<void> {
  await ask(page, words);
  await expect(page.frameLocator("iframe").first().getByText(shown, { exact: false }).first()).toBeAttached({ timeout: 300_000 });
}

/** The conversation at its whole height: a tall window, so nothing in it scrolls. */
async function conversationShot(page: Page, path: string, width: number): Promise<string> {
  const chat = page.locator("#run-chat");
  const height = await chat.evaluate((element) => Math.max(...[element, ...element.querySelectorAll("*")].map((each) => each.scrollHeight)));
  await page.setViewportSize({ width, height: Math.min(height + 320, 7_000) });
  await page.waitForTimeout(500);
  await chat.screenshot({ path });
  return chat.innerText();
}

for (const { take, viewport } of TAKES) {
  test(`recording ${take}: prompt to first dashboard, then a filter and a computed column`, async ({ browser }) => {
    test.skip(!VIDEO, "the slide 12 recording: AI_DASHBOARD_VIDEO=<dir>");
    const dir = VIDEO ?? "";
    mkdirSync(dir, { recursive: true });
    // Signed in off camera: no sign-in page, password or token reaches the recording.
    const signedIn = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
    const storageState = await signedIn.context.storageState();
    await signedIn.context.close();
    const context = await browser.newContext({ viewport, recordVideo: { dir, size: viewport }, storageState });
    await shareEdge(context);
    await context.addInitScript(clockOverlay);
    const page = await context.newPage();
    const opened = Date.now();
    const name = `city-bikes-${SUFFIX.slice(-4)}${take === "desktop" ? "d" : "p"}`;
    const report: Record<string, unknown> = { take, name, viewport, opened };
    const video = page.video();
    let run = "";
    try {
      const { id, pressed } = await generate(page, name, PROMPT, take, async () => {
        await page.getByLabel("What should the app do?").first().scrollIntoViewIfNeeded();
        await page.waitForTimeout(1_500);
      });
      run = id;
      Object.assign(report, { run, pressed });
      const previewPane = page.locator("iframe").first();
      const preview = page.frameLocator("iframe").first();
      await expect(preview.locator(MAP)).toBeVisible({ timeout: 300_000 });
      await previewPane.scrollIntoViewIfNeeded();
      await expect(preview.locator(".jc-loading, [aria-busy=true]")).toHaveCount(0, { timeout: 120_000 });
      await expect(preview.getByText("…", { exact: true })).toHaveCount(0, { timeout: 120_000 });
      await expect.poll(() => paintedPixels(preview), { timeout: 120_000 }).toBeGreaterThanOrEqual(PAINTED);
      const rendered = await clockMark(page, "first dashboard");
      const config = JSON.parse((await preview.locator("#jc-config").textContent()) ?? "{}") as { slug: string };
      Object.assign(report, {
        slug: config.slug,
        firstRenderS: (rendered - pressed) / 1000,
        direct: {
          at: new Date().toISOString(),
          total: await directCount(page, config.slug),
          empty: await directCount(page, config.slug, "availableBikeNumber==0"),
          noReading: await directCount(page, config.slug, "!availableBikeNumber"),
        },
        firstRenderText: await preview.locator("body").innerText(),
      });
      await page.screenshot({ path: join(dir, `${take}-first-render.png`) });
      if (FILM) {
        await page.waitForTimeout(5_000);
        return;
      }
      await expect(page.getByTestId("run-progress").filter({ hasText: "Waiting for you" })).toBeVisible({ timeout: 420_000 });
      report.waitingS = (Date.now() - pressed) / 1000;
      await edit(page, FILTER_EDIT, FILTER_LABEL);
      report.filterS = ((await clockMark(page, "filter")) - pressed) / 1000;
      await previewPane.scrollIntoViewIfNeeded();
      await page.waitForTimeout(2_000);
      await preview.getByLabel(FILTER_LABEL).first().check({ timeout: 10_000 }).catch((error: unknown) => {
        report.filterClick = String(error).split("\n")[0];
      });
      await page.waitForTimeout(3_000);
      report.filteredText = await preview.locator("body").innerText();
      await page.screenshot({ path: join(dir, `${take}-filter.png`) });
      await edit(page, COLUMN_EDIT, COLUMN_LABEL);
      report.columnS = ((await clockMark(page, "computed column")) - pressed) / 1000;
      await previewPane.scrollIntoViewIfNeeded();
      await page.waitForTimeout(4_000);
      report.finalText = await preview.locator("body").innerText();
      await page.screenshot({ path: join(dir, `${take}-column.png`) });
      report.closed = Date.now();
      report.conversation = await conversationShot(page, join(dir, `${take}-conversation.png`), viewport.width);
    } finally {
      writeFileSync(join(dir, `${take}.json`), `${JSON.stringify(report, null, 1)}\n`);
      if (run) await cancel(page, run);
      await context.close();
      await video?.saveAs(join(dir, `${take}.webm`));
    }
  });
}

test("assistant: the same prompt from the Assistant page, timed to its first map", async ({ browser }) => {
  test.skip(process.env.AI_DASHBOARD_ASSISTANT !== "1", "one paid conversation, on its own: AI_DASHBOARD_ASSISTANT=1");
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/assistant?lang=en`);
  await both(page, "assistant", "prompt");
  const pressed = Date.now();
  await ask(page, PROMPT);
  // The dock answers in words, opens a form or starts a build: a map may stand anywhere, or nowhere.
  const trail: { s: number; url: string }[] = [];
  let drawn: number | null = null;
  let said = "";
  while (Date.now() - pressed < 240_000 && drawn === null) {
    await page.waitForTimeout(3_000);
    const s = (Date.now() - pressed) / 1000;
    if (trail.at(-1)?.url !== page.url()) trail.push({ s, url: page.url() });
    for (const frame of page.frames()) {
      if ((await frame.locator("canvas").count().catch(() => 0)) > 0) drawn = s;
    }
    said = await page.getByRole("list", { name: "Conversation" }).first().innerText({ timeout: 1_000 }).catch(() => said);
    if (/navigat|Open|propos|cannot|can't|endpoint/i.test(said) && s > 60 && (await page.getByRole("button", { name: "Send" }).first().isEnabled().catch(() => false))) break;
  }
  await both(page, "assistant", drawn === null ? "no-map" : "first-render");
  writeFileSync(join(DIR, "assistant.json"), `${JSON.stringify({ firstRenderS: drawn, trail, said: said.slice(0, 3_000) }, null, 1)}\n`);
  await context.close();
});

test("deny: the App asks for a type its Endpoint does not serve, and no row of it reaches the preview", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  const { id } = await generate(page, `t6403-denied-${SUFFIX}`, DENIED_PROMPT, "deny");
  const preview = page.frameLocator("iframe").first();
  await expect(page.getByTestId("run-progress").filter({ hasText: "Waiting for you" })).toBeVisible({ timeout: 600_000 });
  await both(page, "deny", "first-render");
  const reads: { status: number; rows: number }[] = [];
  const frame = page.frames().find((each) => each !== page.mainFrame());
  const config = JSON.parse((await frame?.locator("#jc-config").textContent()) ?? "{}") as { slug: string };
  const door = await page.request.get(`/api/endpoint/${config.slug}/ngsi-ld/v1/entities?type=AirQualityObserved&limit=1`);
  const body = (await door.json().catch(() => null)) as unknown;
  reads.push({ status: door.status(), rows: Array.isArray(body) ? body.length : 0 });
  const events = await runEvents(page, id);
  const text = await preview.locator("body").innerText();
  writeFileSync(join(DIR, "deny.json"), `${JSON.stringify({ run: id, reads, events, text: text.slice(0, 2_000) }, null, 1)}\n`);
  await cancel(page, id);
  await context.close();
  expect(reads[0]?.rows, "the Endpoint hands out no row of a type it does not serve").toBe(0);
  expect(door.status(), "the door refuses or answers empty, never a 5xx").toBeLessThan(500);
});

test.afterAll(() => {
  mkdirSync(DIR, { recursive: true });
  writeFileSync(join(DIR, "report.json"), `${JSON.stringify(results, null, 2)}\n`);
});

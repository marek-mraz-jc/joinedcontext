import { readFileSync } from "node:fs";
import { extname, resolve } from "node:path";
import { test, expect, type FrameLocator, type Page, type Route } from "@playwright/test";

// A temporal chart's toolbar (T-7490) in jc slug mode, served as slug_chart_host.spec.ts serves it:
// the chart host and the renderer sandboxed to opaque origins under jc's CSP, every address
// answered here. The temporal answers are praha-pocasi's (jc-prod Endpoint fudnrvyz…, three of its
// stations, relativeHumidity), every bucket as the Endpoint answered it:
// - raw and hourly averages: read 2026-11-21 15:15Z, 168 buckets per station;
// - daily maxima per UTC day: read 2026-11-21 ~16:40Z, its 2026-11-19 bucket from the same read
//   repeated 2026-11-22 08:25Z (the other whole days still stored answer the same).
// The source measured nothing from 2026-11-19 00:00Z to 2026-11-20 06:50Z: the raw answer has no
// reading there, the aggregates carry the last one over that time (a plateau), and the chart draws
// both as answered and says that an aggregate may carry a value.
//   node <portal ui>/node_modules/@playwright/test/cli.js test -c crates/dataroom/web/e2e temporal_chart_controls
// SHOTS=<dir> also writes the 1440 and 400 shots of each state.

const SITE = resolve(__dirname, "../../../../vendor/viz");
const FIXTURES = resolve(__dirname, "fixtures");
const fixture = (name: string) => JSON.parse(readFileSync(resolve(FIXTURES, name), "utf8"));
const JC = "https://jc.city.test";
const VIEW = "view-01a1c5e4-c153-712b-887a-282522e42099";
const ENDPOINT = "fudnrvyz5x6iugzs5dudektnpnlvud54";
const NOW = new Date("2026-11-21T15:10:00Z");

const SANDBOX = "sandbox allow-scripts allow-popups allow-popups-to-escape-sandbox allow-downloads";
const TAIL = `base-uri 'none'; form-action 'none'; ${SANDBOX}`;
const CSP = {
  host: `default-src 'none'; script-src ${JC}; style-src ${JC}; img-src ${JC} data:; connect-src ${JC} ${JC}; frame-src ${JC}; ${TAIL}; frame-ancestors *`,
  render: `default-src 'none'; script-src ${JC}; style-src ${JC} 'unsafe-inline'; img-src ${JC} data:; connect-src 'none'; ${TAIL}`,
};
// A Slovak reader in Prague: native date fields and the axis in their own format and time.
// A date field takes its format from the browser's own language, not the context's.
test.use({ locale: "sk-SK", timezoneId: "Europe/Prague", launchOptions: { args: ["--lang=sk-SK"], env: { ...process.env, LANG: "sk_SK.UTF-8", LANGUAGE: "sk" } } });

const META = `<meta name="jc-viz" content="${JSON.stringify({ api: JC, tiles: "", catalogue: `${JC}/opendata` }).replace(/"/g, "&quot;")}">\n`;

// praha-pocasi's humidity view as the catalogue serves it (2026-11-21).
const SPEC = {
  $schema: "https://vega.github.io/schema/vega-lite/v5.json",
  data: { name: "source" },
  mark: { type: "line", point: true },
  encoding: {
    x: { field: "observedAt", type: "temporal", title: null },
    y: { field: "relativeHumidity", type: "quantitative", scale: { zero: false } },
    color: { field: "name", type: "nominal", title: null },
    tooltip: [
      { field: "observedAt", type: "temporal" },
      { field: "relativeHumidity", type: "quantitative", format: ".1f" },
      { field: "name", type: "nominal" },
    ],
  },
};
const VIEW_JSON = {
  success: true,
  result: {
    id: VIEW,
    package_id: "praha-pocasi",
    view_type: "jc_visualization",
    title: "Relatívna vlhkosť",
    description: "Vlhkosť vzduchu na staniciach ČHMÚ.",
    endpoint: ENDPOINT,
    source: JSON.stringify({ source: "temporal", type: "WeatherObserved", attrs: ["relativeHumidity"], last: "P7D" }),
    spec: JSON.stringify(SPEC),
  },
};

type Mode = "normal" | "empty" | "error" | "cut" | "slow";
// The buckets a cut answer brings before the page's limit: up to 2026-11-20 00:00Z.
const CUT = 129;
const TYPES: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css" };

// The temporal answer for a request: the recorded one of its format, its value key the method asked.
function answer(url: URL): unknown {
  const method = url.searchParams.get("aggrMethods");
  if (!method) return fixture("praha-humidity-raw.json");
  const daily = url.searchParams.get("aggrPeriodDuration") === "P1D";
  type Entity = { id: string; relativeHumidity: { avg?: unknown[]; max?: unknown[] } };
  const body = fixture(daily ? "praha-humidity-daily-max.json" : "praha-humidity-hourly.json") as Entity[];
  return body.map((e) => {
    const attr = e.relativeHumidity;
    const pairs = attr.avg ?? attr.max;
    return { ...e, relativeHumidity: { type: "Property", [method]: pairs } };
  });
}

async function serve(page: Page, state: { mode: Mode; asked: URL[] }, datasetPage: string) {
  await page.route("**/*", async (route: Route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.origin !== JC) return route.fulfill({ status: 404, body: "" });
    const opaque = request.headers()["origin"] === "null" ? { "access-control-allow-origin": "null", vary: "Origin" } : {};
    if (url.pathname === "/opendata/api/3/action/resource_view_show" && url.searchParams.get("id") === VIEW) {
      return route.fulfill({ status: 200, headers: opaque, contentType: "application/json", body: JSON.stringify(VIEW_JSON) });
    }
    if (url.pathname === "/opendata/dataset/praha-pocasi") {
      return route.fulfill({ status: 200, contentType: "text/html; charset=utf-8", body: datasetPage });
    }
    if (url.pathname === `/api/endpoint/${ENDPOINT}/ngsi-ld/v1/entities`) {
      return route.fulfill({ status: 200, headers: opaque, contentType: "application/json", body: JSON.stringify(fixture("praha-names.json")) });
    }
    if (url.pathname === `/api/endpoint/${ENDPOINT}/ngsi-ld/v1/temporal/entities`) {
      state.asked.push(url);
      if (state.mode === "error") return route.fulfill({ status: 503, headers: opaque, contentType: "application/json", body: "{}" });
      if (state.mode === "empty") return route.fulfill({ status: 200, headers: opaque, contentType: "application/json", body: "[]" });
      if (state.mode === "slow") await new Promise((r) => setTimeout(r, 4000));
      let body = JSON.stringify(answer(url));
      if (state.mode === "cut") {
        // A 206 with the oldest CUT buckets whose next window is the same address: the host stops
        // there and says that the newer ones are missing.
        body = JSON.stringify(JSON.parse(body, (key, value) => (key === "avg" ? value.slice(0, CUT) : value)));
        const link = `<${url.href}>; rel="intervalafter"`;
        return route.fulfill({ status: 206, headers: { ...opaque, link, "access-control-expose-headers": "Link" }, contentType: "application/json", body });
      }
      return route.fulfill({ status: 200, headers: opaque, contentType: "application/json", body });
    }
    if (url.pathname.startsWith("/embed/")) {
      const name = url.pathname === "/embed/" ? "index.html" : url.pathname.slice("/embed/".length);
      if (!/^(vendor\/)?[a-z0-9.-]+$/.test(name)) return route.fulfill({ status: 404, body: "" });
      let body: Buffer | string = readFileSync(resolve(SITE, name.startsWith("vendor/") ? name : `site/${name}`));
      if (name === "index.html") body = body.toString().replace("<head>\n", `<head>\n${META}`);
      const csp = name === "render.html" ? CSP.render : name === "index.html" ? CSP.host : "default-src 'none'; frame-ancestors 'none'";
      return route.fulfill({ status: 200, headers: { "content-security-policy": csp, ...opaque }, contentType: TYPES[extname(name)] ?? "application/octet-stream", body });
    }
    return route.fulfill({ status: 404, body: "" });
  });
}

// The catalogue's frame of the chart host: sandboxed, without allow-same-origin, sized by the
// host's jc-viz-height message as the catalogue sizes it.
async function open(page: Page, state: { mode: Mode; asked: URL[] }, hash = "") {
  await page.clock.setFixedTime(NOW);
  await serve(page, state, `<!doctype html><html lang="sk"><body style="margin:0;padding:16px;font:16px system-ui">
<h1>Počasie Praha</h1><iframe title="Relatívna vlhkosť" src="${JC}/embed/#${VIEW}&inline&lang=sk${hash}" sandbox="allow-scripts allow-downloads"
 style="width:100%;height:900px;border:0"></iframe>
<script>addEventListener("message", (e) => { if (e.data && e.data.type === "jc-viz-height") document.querySelector("iframe").style.height = Math.min(e.data.height, 3000) + "px"; });</script></body></html>`);
  await page.goto(`${JC}/opendata/dataset/praha-pocasi`);
  const host = page.frameLocator('iframe[title="Relatívna vlhkosť"]');
  return { host, chart: host.frameLocator("iframe#frame") };
}

// How many separate strokes each station's line has in the chart over the strip.
const strokes = (chart: FrameLocator) =>
  chart.locator("g.concat_0_group g.mark-line path").evaluateAll((paths) => paths.map((p) => (p.getAttribute("d") ?? "").split("M").length - 1));

// Picks from the stations' list, opened and closed as a reader does.
const pick = async (host: FrameLocator, values: string | string[]) => {
  await host.locator("#c-stations-picker summary").click();
  await host.locator("#c-stations").selectOption(values);
  await host.locator("#c-stations-picker summary").click();
};

const shot = async (page: Page, name: string, width: number) => {
  // The catalogue frame takes the host's new height by message: let it land before the shot.
  if (process.env.SHOTS) await page.waitForTimeout(500);
  if (process.env.SHOTS) await page.screenshot({ path: `${process.env.SHOTS}/temporal-${name}-${width}.png`, fullPage: true });
};

for (const width of [1440, 400]) {
  test.describe(`${width} px`, () => {
  test.use({ hasTouch: width < 480 });
  test(`a reader switches range, granularity, method and stations and zooms with the brush (${width})`, async ({ page }) => {
    const errors: string[] = [];
    page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
    await page.setViewportSize({ width, height: 900 });
    const state = { mode: "normal" as Mode, asked: [] as URL[] };
    const { host, chart } = await open(page, state);

    // Normal: the view's own 7 days of raw readings, three stations, the y axis in percent.
    await expect(chart.locator("g.mark-line path")).toHaveCount(6, { timeout: 20_000 });
    expect(state.asked[0].searchParams.get("timerel")).toBe("after");
    expect(state.asked[0].searchParams.get("format")).toBe("temporalValues");
    await expect(chart.locator("text", { hasText: "Relatívna vlhkosť (%)" }).first()).toBeVisible();
    await expect(host.getByLabel("Obdobie")).toHaveValue("P7D");
    await expect(host.getByLabel("Výpočet")).toBeHidden();
    await expect(host.locator("#c-stations option:checked")).toHaveText(["Všetky stanice"]);
    await expect(host.locator("#controls-note")).toContainText("Medzera v čiare: v tom čase nie sú merania");
    // Readings have none from 19. 11. 00:00Z to 20. 11. 06:50Z: each line breaks there.
    for (const count of await strokes(chart)) expect(count).toBeGreaterThan(1);
    await shot(page, "normal", width);

    // Hourly, the same days: drawn as answered, the held value included, and said so in one line.
    await host.getByLabel("Podrobnosť").selectOption("PT1H");
    await expect(host.locator("#data-note.carried")).toContainText("prenesená cez čas bez meraní");
    await expect(host.locator("#data-note")).toContainText("„Merania“");
    await expect.poll(() => strokes(chart)).toEqual([1, 1, 1]);
    await expect(host.locator("#axis-note")).toContainText("v miestnom čase");
    await shot(page, "hourly", width);

    // 24 h hourly maxima: one read, the rows swapped into the running view.
    await host.getByLabel("Obdobie").selectOption("PT24H");
    await host.getByLabel("Podrobnosť").selectOption("PT1H");
    await host.getByLabel("Výpočet").selectOption("max");
    await expect.poll(() => state.asked.at(-1)?.searchParams.get("aggrMethods")).toBe("max");
    const last = state.asked.at(-1)!;
    expect(Object.fromEntries(last.searchParams)).toMatchObject({ timerel: "after", timeAt: "2026-11-20T15:00:00Z", format: "aggregatedValues", aggrPeriodDuration: "PT1H" });

    // 31 days raw becomes hourly and says so.
    await host.getByLabel("Podrobnosť").selectOption("raw");
    await host.getByLabel("Obdobie").selectOption("P31D");
    await expect(host.locator("#controls-note")).toContainText("po hodinách");
    expect(state.asked.at(-1)!.searchParams.get("aggrPeriodDuration")).toBe("PT1H");
    await expect(host.getByLabel("Podrobnosť")).toHaveValue("PT1H");

    // Custom range over 31 days: consecutive daily windows `between`, the note names the switch.
    await host.getByLabel("Obdobie").selectOption("custom");
    const before = state.asked.length;
    await host.getByLabel("Od", { exact: true }).fill("2026-09-01");
    await host.getByLabel("Do", { exact: true }).fill("2026-11-20");
    await expect(host.locator("#controls-note")).toContainText("po dňoch");
    const walk = state.asked.slice(before).filter((u) => u.searchParams.get("timeAt")?.startsWith("2026-09-01") || u.searchParams.get("endTimeAt") === "2026-11-21T00:00:00Z");
    expect(walk.length).toBeGreaterThanOrEqual(2);
    for (const u of walk) expect(u.searchParams.get("timerel")).toBe("between");
    // The walk's earlier windows came back empty: the chart says where the data starts.
    await expect(host.locator("#controls-note")).toContainText("Pred 14.\u00a011.\u00a02026 v tomto období nie sú uložené údaje");
    await expect(host.getByLabel("Podrobnosť")).toHaveValue("P1D");
    // Daily buckets are UTC days: the axis says that in place of local time.
    await expect(host.locator("#axis-note")).toContainText("Dni sú v UTC");
    await expect(host.locator("#axis-note")).not.toContainText("miestnom");
    await expect(host.locator("#data-note.carried")).toContainText("poslednú hodnotu spred výpadku");
    await expect(host.locator("#c-from-hint")).toContainText("1. nov. 2026 (UTC)");
    // A point a day and no line through a day that may be carried.
    await expect(chart.locator("g.concat_0_group g.mark-line path")).toHaveCount(0);
    await expect(chart.locator("g.concat_0_group g.mark-symbol.role-mark path[d]:not([d=''])")).toHaveCount(21);
    // No tick label runs out of the chart.
    expect(await chart.locator("svg.marks g.role-axis-label text").evaluateAll((labels) => labels.filter((l) => {
      const svg = (l as SVGTextElement).ownerSVGElement!.getBoundingClientRect();
      return getComputedStyle(l).opacity !== "0" && l.getBoundingClientRect().right > svg.right;
    }).map((l) => l.textContent))).toEqual([]);
    await expect(chart.locator("g.concat_0_group g.role-axis-label text").first()).toHaveText(/^\d+\. \d+\.$/);
    await shot(page, "daily-walk", width);

    // Back to 7 days, then two stations: the third line goes, no new read.
    await host.getByLabel("Obdobie").selectOption("P7D");
    await host.getByLabel("Podrobnosť").selectOption("raw");
    await expect(chart.locator("g.mark-line path")).toHaveCount(6, { timeout: 10_000 });
    const reads = state.asked.length;
    await pick(host, ["urn:ngsi-ld:WeatherObserved:hel.fi:praha-mesto:chmi-0-20000-0-11518", "urn:ngsi-ld:WeatherObserved:hel.fi:praha-mesto:chmi-0-20000-0-11519"]);
    await expect(chart.locator("g.mark-line path")).toHaveCount(4);
    // The closed list names both picks.
    await expect(host.locator("#c-stations-picker summary")).toHaveText(/^\S+, \S+ \(2\)$/);
    expect(state.asked.length).toBe(reads);
    await shot(page, "two-stations", width);

    // The brush on the strip zooms the chart: the detail's first x label moves forward.
    const strip = chart.locator("g.concat_1_group path.background, g[class*='concat_1'] .background").first();
    const box = await strip.boundingBox();
    expect(box).not.toBeNull();
    const firstLabel = () => chart.locator("g.concat_0_group g.role-axis-label text, g[class*='concat_0'] .role-axis-label text").first().textContent();
    const unzoomed = await firstLabel();
    const reset = host.getByRole("button", { name: "Zobraziť celé obdobie" });
    await expect(reset).toBeHidden();
    await page.mouse.move(box!.x + box!.width * 0.55, box!.y + box!.height / 2);
    await page.mouse.down();
    await page.mouse.move(box!.x + box!.width * 0.85, box!.y + box!.height / 2, { steps: 8 });
    await page.mouse.up();
    await expect.poll(firstLabel).not.toBe(unzoomed);
    await shot(page, "zoomed", width);
    // The reset button draws the whole range again.
    await reset.click();
    await expect.poll(firstLabel).toBe(unzoomed);
    await expect(reset).toBeHidden();
    expect(errors).toEqual([]);
  });

  test(`empty, error, loading and cut answers each say so (${width})`, async ({ page }) => {
    const errors: string[] = [];
    page.on("console", (m) => { if (m.type() === "error" && !/503/.test(m.text())) errors.push(m.text()); });
    await page.setViewportSize({ width, height: 900 });
    const state = { mode: "empty" as Mode, asked: [] as URL[] };
    const { host, chart } = await open(page, state);
    await expect(host.locator("#status")).toContainText("žiadne údaje", { timeout: 20_000 });
    // The stations stay to change with no answer to read them from, and the pick is kept.
    const stations = host.locator("#c-stations");
    await expect(stations.locator("option")).toHaveText(["Všetky stanice", "Praha, Karlov", "Praha, Libuš", "Praha, Ruzyně"]);
    await shot(page, "empty", width);
    const karlov = "urn:ngsi-ld:WeatherObserved:hel.fi:praha-mesto:chmi-0-20000-0-11519";
    await pick(host, karlov);

    state.mode = "slow";
    await host.getByLabel("Obdobie").selectOption("PT24H");
    await expect(host.locator("#status")).toContainText("Načítavam");
    await expect(host.locator("#c-stations-picker summary")).toBeVisible();
    await shot(page, "loading", width);
    await expect(host.locator("#status")).toBeHidden({ timeout: 10_000 });
    await expect(stations).toHaveValues([karlov]);
    await pick(host, "");

    state.mode = "cut";
    await host.getByLabel("Obdobie").selectOption("P31D");
    await expect(host.locator("#status")).toContainText("neúplný", { timeout: 10_000 });
    await expect(host.locator("#controls-note")).toContainText("Pred 14.\u00a011.\u00a02026 v tomto období nie sú uložené údaje");
    await expect(host.locator("#status")).toContainText("novšie údaje");
    await expect(host.locator("#status")).not.toContainText("ďalšie stanice");
    await expect(host.locator("#status")).toContainText(/od 14\.\u00a011\.\u00a02026\u00a0\d\d:\d\d do 20\.\u00a011\.\u00a02026\u00a0\d\d:\d\d/);
    // The axis runs on to the end of the range picked, past the last bucket read.
    await expect(chart.locator("g.concat_0_group g.role-axis-label text").filter({ hasText: "21. 11." })).toHaveCount(1);
    await expect(chart.locator("g.concat_0_group text", { hasText: "nenačítané" })).toHaveCount(1);
    await shot(page, "cut", width);

    state.mode = "error";
    await host.getByLabel("Obdobie").selectOption("P7D");
    await expect(host.locator("#status")).toContainText("HTTP 503", { timeout: 20_000 });
    await expect(host.locator("#c-stations-picker summary")).toBeVisible();
    await shot(page, "error", width);

    // The reader retries once the Endpoint answers again.
    state.mode = "normal";
    await host.getByRole("button", { name: "Skúsiť znova" }).click();
    await expect(host.locator("#status")).toBeHidden({ timeout: 10_000 });
    await expect(host.getByRole("button", { name: "Skúsiť znova" })).toBeHidden();

    // A change over a drawn chart keeps it, dimmed, until the new rows land.
    state.mode = "slow";
    await host.getByLabel("Obdobie").selectOption("PT24H");
    await expect(host.locator("iframe#frame[aria-busy=true]")).toBeVisible();
    await expect(host.locator("#status")).toContainText("24 hodín");
    // The note of the chart still drawn stays with it.
    await expect(host.locator("#data-note.carried")).toBeVisible();
    await shot(page, "reloading", width);

    // The choice survives in the fragment: a reload keeps 7 days and the view's default granularity.
    const src = await page.locator("iframe").getAttribute("src");
    expect(src).toContain(VIEW);
    expect(errors).toEqual([]);
  });

  test(`the first read shows every control while it loads (${width})`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const { host } = await open(page, { mode: "slow", asked: [] });
    await expect(host.locator("#status")).toContainText("Načítavam", { timeout: 20_000 });
    await expect(host.locator("#c-stations option")).toHaveCount(4);
    await shot(page, "first-loading", width);
  });
  });
}

test("a link's fragment restores the toolbar and a bad value is dropped", async ({ page }) => {
  const state = { mode: "normal" as Mode, asked: [] as URL[] };
  const { host } = await open(page, state, "&r=PT24H&g=P1D&m=min&s=urn:ngsi-ld:WeatherObserved:hel.fi:praha-mesto:chmi-0-20000-0-11520&x=javascript:1");
  await expect(host.getByLabel("Obdobie")).toHaveValue("PT24H", { timeout: 20_000 });
  await expect(host.getByLabel("Podrobnosť")).toHaveValue("P1D");
  await expect(host.getByLabel("Výpočet")).toHaveValue("min");
  await expect.poll(() => state.asked.length).toBeGreaterThan(0);
  expect(Object.fromEntries(state.asked[0].searchParams)).toMatchObject({ aggrMethods: "min", aggrPeriodDuration: "P1D", timeAt: "2026-11-20T00:00:00Z" });
  await expect(host.locator("#c-stations-picker summary")).toHaveText("Libuš");
  await expect(host.locator("#c-stations option:checked")).toHaveText(["Praha, Libuš"]);
});

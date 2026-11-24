// T-7591: the host page jc serves (vendor/viz/site/host.js) in a DOM: `main` reading a view and
// drawing it, the temporal toolbar and each change of it, and `#show` for the assistant's widget.
// jsdom is the Portal UI's devDependency: run `pnpm install` in crates/portal/ui first.
//   node --test vendor/viz/tests/
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

process.env.TZ = "UTC";
const here = path.dirname(fileURLToPath(import.meta.url));
const { JSDOM } = createRequire(path.join(here, "../../../crates/portal/ui/package.json"))("jsdom");
const SITE = path.join(here, "../site");
const SCRIPTS = ["refuse.js", "granularity.js", "host.js"].map((name) => {
  const file = path.join(SITE, name);
  return { file, source: fs.readFileSync(file, "utf8") };
});
const INDEX = fs.readFileSync(path.join(SITE, "index.html"), "utf8").replace(/<script[^>]*><\/script>\n?/g, "");
const VIEW_ID = "6f1c2b9e-0d0a-4c35-9a51-3a3f8f6c1d20";
const SLUG = "3iasem46w7d7pxxypwdfdzheyaodv3qn";
const VL = "https://vega.github.io/schema/vega-lite/v5.json";
const HOUR = 3600e3;
const DAY = 24 * HOUR;
const HANG = Symbol("hang");

const flush = async (rounds = 12) => {
  for (let i = 0; i < rounds; i += 1) await new Promise((done) => setImmediate(done));
};

// A fetch answer as the page reads it: JSON by json(), a body by its reader, Content-Length.
function answer(body, status = 200, headers = {}) {
  const text = typeof body === "string" ? body : JSON.stringify(body);
  const bytes = new TextEncoder().encode(text);
  return {
    ok: status >= 200 && status < 300, status,
    headers: { get: (name) => (name in headers ? headers[name] : name === "Content-Length" ? String(bytes.length) : null) },
    json: async () => JSON.parse(text),
    body: { getReader() { let done = false; return { read: async () => (done ? { done: true } : (done = true, { done: false, value: bytes })), cancel() {} }; } }
  };
}

const SPEC = {
  $schema: VL, data: { name: "source" }, mark: "line",
  encoding: {
    x: { field: "observedAt", type: "temporal" }, y: { field: "no2", type: "quantitative" }, color: { field: "name", type: "nominal" },
    tooltip: [{ field: "no2" }, { field: "observedAt", type: "temporal" }, { field: "name" }, null, { field: "x", title: "own" }]
  }
};
const TEMPORAL = { source: "temporal", type: "AirQualityObserved", attrs: ["no2", "relativeHumidity"], last: "P7D" };
const view = (source, more = {}) => ({
  view_type: "jc_visualization", spec: JSON.stringify(more.spec || SPEC), source: JSON.stringify(source), endpoint: SLUG, package_id: "ovzdusie",
  titles: JSON.stringify({ sk: "Oxid dusičitý", en: "Nitrogen dioxide" }), descriptions: JSON.stringify({ en: "Hourly NO2" }), ...more.view
});

// Stations, each a list of [hours ago, value]; the broker's answer in the format the URL asks.
function broker(stations, attr = "no2") {
  return (url) => {
    const aggregated = /aggrMethods=([a-z]+)/.exec(url);
    const asked = new URL(url).searchParams.get("attrs").split(",");
    return answer(Object.entries(stations).map(([id, points]) => {
      const entity = { id, type: "AirQualityObserved" };
      asked.forEach((a) => {
        const pairs = points.map(([ago, v]) => [a === attr ? v : v / 100, new Date(Date.now() - ago * HOUR).toISOString()]);
        entity[a] = aggregated ? { type: "Property", [aggregated[1]]: pairs.map((p) => [p[0], p[1], p[1]]) } : { type: "Property", values: pairs };
      });
      return entity;
    }));
  };
}
const NAMES = [{ id: "urn:a", name: "Banská Bystrica, Nám. SNP" }, { id: "urn:b", name: "Banská Bystrica, Zvolenská" }];

function page(o = {}) {
  let html = INDEX;
  if (o.meta !== undefined) html = html.replace("</head>", `<meta name="jc-viz" content='${o.meta}'>\n</head>`);
  const dom = new JSDOM(html, { url: (o.at || "https://viz.example.sk/") + (o.hash === undefined ? `#${VIEW_ID}&lang=en` : o.hash), runScripts: "outside-only", pretendToBeVisual: true });
  const win = dom.window;
  const doc = win.document;
  const toParent = [];
  const parent = { postMessage: (message, origin) => toParent.push({ message: JSON.parse(JSON.stringify(message)), origin }) };
  if (!o.top) Object.defineProperty(win, "parent", { configurable: true, get: () => parent });
  if (!o.noObserver) win.ResizeObserver = class { constructor(f) { this.f = f; } observe() {} };
  if (o.coarse) win.matchMedia = () => ({ matches: true });
  // Time is the test's: a timer runs when the test fires it.
  const timers = [];
  win.setTimeout = (f, ms) => timers.push({ f, ms });
  win.clearTimeout = (id) => { if (timers[id - 1]) timers[id - 1] = null; };
  const posts = [];
  const frames = new WeakMap();
  Object.defineProperty(win.HTMLIFrameElement.prototype, "contentWindow", {
    configurable: true,
    get() {
      if (!frames.has(this)) frames.set(this, { postMessage: (message, origin) => posts.push({ frame: this, message: JSON.parse(JSON.stringify(message)), origin }) });
      return frames.get(this);
    }
  });
  const calls = [];
  win.fetch = (url, options = {}) => {
    calls.push({ url, options });
    let r;
    try { r = o.fetch(url, options, calls); } catch (error) { return Promise.reject(error); }
    if (r === HANG) {
      return new Promise((_, failed) => options.signal.addEventListener("abort", () => failed(new win.DOMException("The operation was aborted.", "AbortError"))));
    }
    return Promise.resolve(r);
  };
  const ctx = dom.getInternalVMContext();
  SCRIPTS.forEach(({ file, source }) => new vm.Script(source, { filename: file }).runInContext(ctx));
  const $ = (s) => doc.querySelector(s);
  return {
    win, doc, $, calls, posts, toParent, parent, timers,
    frame: () => doc.getElementById("frame"),
    status: () => doc.getElementById("status"),
    renders: () => posts.filter((p) => p.message.type === "jc-viz-render"),
    temporals: () => calls.filter((c) => c.url.includes("/temporal/")),
    message(data, origin, source) {
      const event = new win.Event("message");
      Object.defineProperties(event, { data: { value: data }, origin: { value: origin }, source: { value: source } });
      win.dispatchEvent(event);
    },
    ready() { this.message({ type: "jc-viz-ready" }, "null", this.frame().contentWindow); },
    fire(ms) {
      timers.forEach((t, i) => {
        if (t && t.ms === ms) {
          timers[i] = null;
          t.f();
        }
      });
    },
    pick(id, value) {
      const el = doc.getElementById(id);
      el.value = value;
      el.dispatchEvent(new win.Event("change"));
    }
  };
}

// The catalogue's view, the broker's answers and the stations' names.
const routes = (v, temporal, names = NAMES) => (url, options, calls) => {
  if (url.includes("/resource_view_show")) return typeof v === "function" ? v(url) : answer({ result: v });
  if (url.includes("/temporal/")) return temporal(url, options, calls);
  if (url.includes("/ngsi-ld/v1/entities?")) return typeof names === "function" ? names(url) : answer(names);
  if (url.includes("/odata/v4/")) return answer({ value: [{ id: "urn:a", name: null, no2: 11 }] });
  throw new Error("unexpected " + url);
};

test("a chart of a view: its title, description, catalogue link and rows go to the renderer once it is ready", async () => {
  const chat = page({ hash: `#${VIEW_ID}&inline&lang=sk`, fetch: routes(view({ source: "odata", entitySet: "AirQualityObserved" })) });
  assert.equal(chat.frame().getAttribute("src"), "render.html");
  assert.ok(chat.doc.body.classList.contains("inline"));
  await flush();
  const asked = chat.calls[0];
  assert.equal(asked.url, `https://opendata.example.sk/api/3/action/resource_view_show?id=${VIEW_ID}`);
  assert.equal(asked.options.credentials, "omit");
  assert.equal(chat.doc.title, "Oxid dusičitý");
  assert.equal(chat.$("#title").textContent, "Oxid dusičitý");
  assert.equal(chat.$("#description").textContent, "Hourly NO2", "no Slovak description: the first it has");
  assert.equal(chat.$("#table").textContent, "Dáta a tabuľka v katalógu otvorených dát");
  assert.equal(chat.$("#table").href, "https://opendata.example.sk/dataset/ovzdusie");
  assert.equal(chat.renders().length, 0, "the renderer has not said it is ready");
  chat.message({ type: "jc-viz-ready" }, "https://evil.example", chat.frame().contentWindow);
  chat.message({ type: "jc-viz-ready" }, "null", {});
  await flush();
  assert.equal(chat.renders().length, 0, "only our frame's own messages count");
  chat.ready();
  await flush();
  const render = chat.renders()[0];
  assert.equal(render.origin, "*");
  assert.equal(render.message.lang, "sk");
  assert.deepEqual(render.message.data.rows, [{ id: "urn:a", name: "Banská Bystrica, Nám. SNP", no2: 11 }]);
  chat.message({ type: "jc-viz-height", height: 5000 }, "null", chat.frame().contentWindow);
  assert.equal(chat.frame().style.height, "2000px");
  chat.message({ type: "jc-viz-height", height: 1 }, "null", chat.frame().contentWindow);
  assert.equal(chat.frame().style.height, "120px");
  assert.deepEqual(chat.toParent.at(-1), { message: { type: "jc-viz-height", height: 0 }, origin: "*" });
  let zoomed = null;
  chat.doc.addEventListener("jc-viz-zoom", (event) => { zoomed = event.detail; });
  chat.message({ type: "jc-viz-zoom", zoomed: true }, "null", chat.frame().contentWindow);
  assert.equal(zoomed, true);
  chat.message(null, "null", chat.frame().contentWindow);
  chat.message({ type: "jc-viz-error" }, "null", chat.frame().contentWindow);
  assert.equal(chat.status().textContent, "The chart could not be drawn.");
  assert.equal(chat.frame().hidden, true);
  chat.message({ type: "jc-viz-error", message: "Bad spec" }, "null", chat.frame().contentWindow);
  assert.equal(chat.status().textContent, "Bad spec");
  const before = chat.toParent.length;
  chat.win.dispatchEvent(new chat.win.Event("resize"));
  assert.equal(chat.toParent.length, before + 1);
});

test("a cut read says so over its chart, and a page of its own posts no height", async () => {
  const many = (url) => answer({ value: [{ id: "urn:a", no2: 1 }], "@odata.nextLink": "https://evil.example/next" });
  const off = page({ top: true, noObserver: true, fetch: (url) => (url.includes("/odata/") ? many(url) : routes(view({ source: "odata", entitySet: "AirQualityObserved" }))(url)) });
  await flush();
  assert.equal(off.status().textContent, "the next page is not on the Endpoint");
  assert.equal(off.toParent.length, 0);
  const partial = { source: "temporal", type: "AirQualityObserved", attrs: ["no2"], last: "P7D", controls: false };
  const cut = page({ fetch: routes(view(partial), (url) => {
    const a = broker({ "urn:a": [[30, 5], [20, 6]] })(url);
    return { ...a, status: 206 };
  }) });
  await flush();
  cut.ready();
  await flush();
  assert.match(cut.status().textContent, /^Partial chart, not loaded: newer data\./);
  assert.equal(cut.$(".controls"), null, "an author's hidden toolbar stays hidden");
  assert.equal(cut.renders().length, 1);
});

test("a page that names no view, no catalogue or an unpublished or refused chart says why", async () => {
  const none = page({ hash: "", fetch: () => { throw new Error("no read"); } });
  assert.equal(none.status().textContent, "This page draws a chart of the open-data catalogue; the address names none.");
  assert.equal(none.status().hidden, false);
  const noCatalogue = page({ meta: '{"api":"https://api.example.sk"}', fetch: () => { throw new Error("no read"); } });
  assert.equal(noCatalogue.status().textContent, "This page names no open-data catalogue to draw a chart from.");
  assert.equal(noCatalogue.calls.length, 0);
  const unpublished = page({ fetch: () => answer({}, 404) });
  await flush();
  assert.equal(unpublished.status().textContent, "The chart is not published.");
  const other = page({ fetch: () => answer({ result: { view_type: "image_view" } }) });
  await flush();
  assert.equal(other.status().textContent, "The chart is not published.");
  const refused = page({ fetch: routes(view(TEMPORAL, { spec: { $schema: VL, data: { url: "https://evil.example/x.json" }, mark: "line" } })) });
  await flush();
  assert.match(refused.status().textContent, /^The chart was refused: \/data/);
  const meta = page({ meta: '{"api":"https://api.example.sk","catalogue":"https://data.example.sk"}', fetch: routes(view({ source: "odata", entitySet: "AirQualityObserved" }, { view: { titles: "null", title: "Plain", descriptions: "null", description: "Words", package_id: undefined } })) });
  await flush();
  assert.equal(meta.calls[0].url.startsWith("https://data.example.sk/api/3/"), true);
  assert.equal(meta.$("#title").textContent, "Plain");
  assert.equal(meta.$("#description").textContent, "Words");
  assert.equal(meta.$("#table").href, "https://data.example.sk/dataset/");
});

test("a refused read names its status in the reader's language, and a read past 15 s says so", async () => {
  const refused = page({ fetch: routes(view({ source: "odata", entitySet: "AirQualityObserved" }), null, () => answer([])) });
  refused.calls.length = 0;
  const slow = page({ fetch: (url) => (url.includes("/resource_view_show") ? HANG : null) });
  slow.fire(15000);
  await flush();
  assert.equal(slow.status().textContent, "The data took longer than 15 seconds.");
  const gone = page({ hash: `#${VIEW_ID}&lang=sk`, fetch: (url) => (url.includes("/odata/") ? answer({}, 403) : routes(view({ source: "odata", entitySet: "AirQualityObserved" }))(url)) });
  await flush();
  assert.equal(gone.status().textContent, "Údaje tohto grafu teraz nie sú dostupné. (HTTP 403)");
  const thrown = page({ fetch: (url) => { throw "plain words"; } });
  await flush();
  assert.equal(thrown.status().textContent, "plain words");
});

test("a temporal chart: toolbar, station list, notes, the interactive spec, then zoom, stations, resize and a new measure", async () => {
  const points = Array.from({ length: 70 }, (_, i) => [i + 1, 20 + (i % 5)]);
  const chat = page({ fetch: routes(view(TEMPORAL), broker({ "urn:a": points, "urn:b": [[61, 40], [60, 41], [59, 42], [5, 43]] })) });
  await flush();
  const form = chat.$("form.controls");
  assert.equal(form.getAttribute("aria-label"), "Chart settings");
  const submit = new chat.win.Event("submit", { cancelable: true });
  form.dispatchEvent(submit);
  assert.equal(submit.defaultPrevented, true, "Enter in a field never leaves the page");
  assert.deepEqual(Array.from(chat.$("#c-range").options).map((o) => o.value), ["PT24H", "P7D", "P31D", "P1Y", "P10Y", "whole", "custom"]);
  assert.equal(chat.$("#c-range").value, "P7D");
  assert.equal(chat.$("#c-range").getAttribute("aria-describedby"), "c-range-hint");
  assert.equal(chat.$("#c-method").parentNode.hidden, true, "readings have no method");
  assert.equal(chat.$("#c-from").parentNode.hidden, true);
  assert.deepEqual(Array.from(chat.$("#c-attr").options).map((o) => o.textContent), ["All", "NO₂", "Relative humidity"]);
  assert.equal(chat.frame().getAttribute("aria-busy"), "true");
  assert.equal(chat.$(".skeleton").hidden, false);
  const temporal = chat.temporals()[0].url;
  assert.match(temporal, /^https:\/\/api\.example\.sk\/api\/endpoint\/3iasem46w7d7pxxypwdfdzheyaodv3qn\/ngsi-ld\/v1\/temporal\/entities\?/);
  assert.match(temporal, /attrs=no2%2CrelativeHumidity/);
  // The names come beside the first window, so the list is there before the rows land.
  assert.deepEqual(Array.from(chat.$("#c-stations").options).map((o) => o.textContent), ["All stations", "Banská Bystrica, Nám. SNP", "Banská Bystrica, Zvolenská"]);
  assert.equal(chat.$("#c-stations-picker summary").textContent, "All stations (2)");
  assert.equal(chat.$("#c-stations-hint").textContent, "E.g. Nám. SNP, Zvolenská.");
  chat.ready();
  await flush();
  assert.equal(chat.status().hidden, true);
  assert.equal(chat.frame().hasAttribute("aria-busy"), false);
  assert.equal(form.getAttribute("aria-busy"), "false");
  const render = chat.renders()[0].message;
  assert.deepEqual(render.spec, SPEC, "two measures: the authored spec as it is");
  assert.equal(render.data.rows.length, (70 + 4) * 2);
  assert.equal(chat.$("#axis-note").textContent, "Times on the axis are local (UTC).");
  assert.match(chat.$("#controls-note").textContent, /^Data available from \w{3}\s\d+,\s\d{4}\.$/, "the readings start inside the week");
  chat.message({ type: "jc-viz-zoom", zoomed: true }, "null", chat.frame().contentWindow);
  const reset = chat.$(".reset");
  assert.equal(reset.hidden, false);
  reset.click();
  assert.equal(chat.renders().length, 2, "drawn again unzoomed");
  assert.equal(reset.hidden, true);
  const stations = chat.$("#c-stations");
  stations.options[2].selected = true;
  stations.options[0].selected = false;
  stations.dispatchEvent(new chat.win.Event("change"));
  assert.equal(chat.win.location.hash, `#${VIEW_ID}&lang=en&s=urn%3Ab`);
  assert.equal(chat.$("#c-stations-picker summary").textContent, "Zvolenská");
  assert.equal(chat.renders().length, 3);
  assert.deepEqual([...new Set(chat.renders()[2].message.data.rows.map((r) => r.id))], ["urn:b"]);
  stations.options[0].selected = true;
  stations.dispatchEvent(new chat.win.Event("change"));
  assert.equal(chat.$("#c-stations-picker summary").textContent, "All stations (2)", "All beside a pick clears it");
  chat.win.dispatchEvent(new chat.win.Event("resize"));
  chat.fire(200);
  assert.equal(chat.posts.at(-1).message.type, "jc-viz-rows", "the same drawing takes the rows alone");
  chat.pick("c-attr", "relativeHumidity");
  await flush();
  assert.match(chat.temporals()[1].url, /attrs=relativeHumidity&/);
  assert.equal(chat.win.location.hash, `#${VIEW_ID}&lang=en&a=relativeHumidity`);
  const humid = chat.renders().at(-1).message;
  assert.equal(humid.spec.vconcat.length, 2, "one measure: the chart and its brush strip");
  assert.equal(humid.data.rows.length, 70 + 4 + 1, "a gap row breaks b's line");
  assert.ok(humid.data.rows.every((r) => r.relativeHumidity === null || r.relativeHumidity >= 20), "a fraction is drawn as percent");
  assert.match(chat.$("#controls-note").textContent, / A gap in a line: no readings at that time\.$/);
  assert.match(chat.$("#axis-note").textContent, /^Times on the axis are local \(UTC\)\. Click or tap a station in the legend/);
  assert.match(JSON.stringify(humid.spec), /Relative humidity \(%\)/);
});

test("an aggregated range says a value may be carried and offers the readings of the last week", async () => {
  const source = { ...TEMPORAL, attrs: ["no2"], aggrMethod: "max", aggrPeriod: "PT1H" };
  const chat = page({ hash: `#${VIEW_ID}&lang=en&r=P1Y&m=bogus&x&=1&s=%E0%A4%A`, fetch: routes(view(source), broker({ "urn:a": [[24 * 40, 10], [24 * 20, 12], [24 * 10, 11], [24 * 5, 13]] })) });
  await flush();
  assert.equal(chat.$("#c-method").value, "max");
  assert.equal(chat.$("#c-method").parentNode.hidden, false);
  chat.ready();
  await flush();
  const url = chat.temporals()[0].url;
  assert.match(url, /timerel=between/);
  assert.match(url, /aggrPeriodDuration=P1D/);
  assert.equal(chat.$("#c-gran").value, "P1D", "the toolbar shows what is drawn");
  assert.equal(Array.from(chat.$("#c-gran").options).find((o) => o.value === "PT1H").disabled, true);
  assert.equal(chat.$("#controls-note").textContent, "Ranges longer than 31 days are shown daily. Data available from " + new Date(Date.now() - 40 * DAY).toLocaleDateString("en", { day: "numeric", month: "short", year: "numeric", timeZone: "UTC" }).replace(/ /g, " ") + ".");
  assert.match(chat.$("#data-note").textContent, /^A point or a flat line may be no measurement/);
  assert.equal(chat.$("#data-note").hidden, false);
  assert.match(chat.$("#axis-note").textContent, /^Days are in UTC/);
  const toReadings = chat.$("#data-note button");
  assert.equal(toReadings.textContent, "Show measured times (\"Readings\") of the last 7 days");
  toReadings.click();
  await flush();
  assert.equal(chat.$("#c-range").value, "P7D");
  assert.equal(chat.$("#c-gran").value, "raw");
  assert.match(chat.temporals()[1].url, /format=temporalValues/);
  assert.equal(chat.win.location.hash, `#${VIEW_ID}&lang=en&x&g=raw`);
});

test("a custom range checks its days, a month pick is no choice for a short range, and an empty answer says what to try", async () => {
  const chat = page({ hash: `#${VIEW_ID}&lang=en&r=custom&from=2026-01-01`, fetch: routes(view({ ...TEMPORAL, attrs: ["no2"] }), () => answer([])) });
  await flush();
  assert.equal(chat.$("#c-range").value, "P7D", "a custom range without its end is the view's own");
  chat.ready();
  await flush();
  assert.equal(chat.status().textContent, "The chart has no data for this range. Try a longer range or another granularity.");
  assert.equal(chat.status().className, "note blank");
  assert.equal(chat.frame().hidden, true);
  assert.equal(chat.$("#c-stations").parentNode.parentNode.hidden, false, "no answer yet: the list stays");
  chat.pick("c-range", "custom");
  assert.equal(chat.$("#c-from").parentNode.hidden, false);
  const reads = chat.temporals().length;
  chat.$("#c-to").value = "2026-02-30";
  chat.pick("c-from", "2026-01-10");
  assert.equal(chat.temporals().length, reads, "an impossible day asks nothing");
  chat.$("#c-to").value = "2026-01-05";
  chat.pick("c-from", "2026-01-10");
  await flush();
  assert.equal(chat.status().textContent, "From must be on or before To and not in the future.");
  assert.equal(chat.$(".retry:not(.reset)").hidden, true);
  chat.pick("c-to", "2026-01-20");
  await flush();
  assert.match(chat.temporals().at(-1).url, /timerel=between&timeAt=2026-01-10T00%3A00%3A00Z&endTimeAt=2026-01-21T00%3A00%3A00Z/);
  assert.equal(chat.win.location.hash, `#${VIEW_ID}&lang=en&r=custom&from=2026-01-10&to=2026-01-20`);
  chat.pick("c-gran", "P1M");
  await flush();
  chat.pick("c-range", "P31D");
  await flush();
  assert.match(chat.temporals().at(-1).url, /aggrPeriodDuration=PT1H/, "months are no pick for 31 days: its finest period is read");
  chat.pick("c-method", "min");
  await flush();
  assert.equal(chat.$("#c-method").value, "min");
});

test("a refused read offers Try again, which reads again; a slow one says 15 seconds", async () => {
  let n = 0;
  const chat = page({ fetch: routes(view({ ...TEMPORAL, attrs: ["no2"] }), (url) => (++n === 1 ? answer("no", 503) : broker({ "urn:a": [[2, 1], [1, 2]] })(url)), () => answer("no", 500)) });
  await flush();
  assert.equal(chat.status().textContent, "The data of this chart is not available now. (HTTP 503)");
  assert.equal(chat.status().className, "blank");
  const retry = chat.$(".retry:not(.reset)");
  assert.equal(retry.hidden, false);
  assert.equal(chat.$("#c-stations").options.length, 1, "names refused: only All stations");
  retry.click();
  chat.ready();
  await flush();
  assert.equal(chat.renders().length, 1);
  assert.equal(chat.status().hidden, true);
  const slow = page({ fetch: routes(view({ ...TEMPORAL, attrs: ["no2"] }), () => HANG, () => HANG) });
  await flush();
  assert.equal(slow.status().className, "loading");
  assert.equal(slow.status().textContent, "Loading data: 7 days…");
  assert.equal(slow.frame().hidden, true, "the first rows keep the chart's place taken");
  slow.fire(15000);
  await flush();
  assert.equal(slow.status().textContent, "The data did not load within 15 seconds.");
  // A change while a read runs aborts it: only the newest read lands.
  const raced = page({ fetch: routes(view({ ...TEMPORAL, attrs: ["no2"] }), (url, options, calls) => (calls.filter((c) => c.url.includes("/temporal/")).length === 1 ? HANG : broker({ "urn:a": [[2, 1], [1, 2]] })(url))) });
  await flush();
  raced.pick("c-range", "PT24H");
  raced.ready();
  await flush();
  assert.equal(raced.renders().length, 1);
  assert.equal(raced.status().hidden, true);
  const broken = page({ fetch: routes(view({ ...TEMPORAL, attrs: ["no2"], type: "../x" }), () => answer([])) });
  await flush();
  assert.match(broken.status().textContent, /./);
});

test("all available is read again from where the data starts, by the granularity of what it covers", async () => {
  const chat = page({ hash: `#${VIEW_ID}&lang=sk&r=whole`, fetch: routes(view({ ...TEMPORAL, attrs: ["no2"] }), broker({ "urn:a": [[24 * 10, 1], [24 * 5, 2], [24, 3]] })) });
  await flush();
  chat.ready();
  await flush();
  const reads = chat.temporals().map((c) => c.url);
  assert.equal(reads.length, 2);
  assert.match(reads[0], /timeAt=1000-01-01T00%3A00%3A00Z/);
  assert.match(reads[0], /aggrPeriodDuration=P1Y/);
  assert.match(reads[1], /aggrPeriodDuration=P1D/);
  assert.equal(chat.$("#c-gran").value, "P1D");
  assert.match(chat.status().textContent, /|/);
});

test("ten years of monthly averages name their years; five stations get a plot each, on a phone one column", async () => {
  const stations = Object.fromEntries(["a", "b", "c", "d", "e"].map((s, i) => ["urn:" + s, [[24 * 365 * 3, 200000 + i], [24 * 400, 10 + i], [24 * 30, 12 + i]]]));
  const names = Object.keys(stations).map((id) => ({ id, name: "Mesto, " + id.slice(4).toUpperCase() }));
  const chat = page({ hash: `#${VIEW_ID}&lang=en&r=P10Y`, fetch: routes(view({ ...TEMPORAL, attrs: ["no2"], aggrMethod: "avg", aggrPeriod: "P1D" }), broker(stations), names) });
  Object.defineProperty(chat.frame(), "clientWidth", { configurable: true, value: 400 });
  await flush();
  chat.ready();
  await flush();
  const spec = chat.renders()[0].message.spec;
  assert.match(JSON.stringify(spec.vconcat[0].title), /Monthly averages, 20\d\d-20\d\d/);
  assert.equal(spec.vconcat[0].columns, 1);
  assert.equal(spec.vconcat[0].facet.field, "name");
  assert.match(chat.$("#axis-note").textContent, /^Months are in UTC/);
  assert.equal(chat.$("#c-stations-hint").textContent, "E.g. A, B.");
  assert.equal(chat.$("#c-stations-picker summary").textContent, "All stations (5)");
  const list = chat.$("#c-stations");
  list.options[1].selected = true;
  list.options[2].selected = true;
  list.options[0].selected = false;
  list.dispatchEvent(new chat.win.Event("change"));
  assert.equal(chat.$("#c-stations-picker summary").textContent, "A, B (2)");
  assert.equal(chat.$("#c-stations-picker summary").title, "Mesto, A; Mesto, B");
});

test("a touch screen says to tap each station, and a station of the hash the answer lacks is dropped", async () => {
  const chat = page({ coarse: true, hash: `#${VIEW_ID}&lang=en&s=urn:a,urn:zzz`, fetch: routes(view({ ...TEMPORAL, attrs: ["no2"] }), broker({ "urn:a": [[3, 1], [2, 2], [1, 3]], "urn:b": [[3, 4], [2, 5]] }), [{ id: "urn:a", name: "Alpha" }, { id: "urn:b", name: "Beta" }]) });
  await flush();
  assert.equal(chat.$("#c-stations-picker small").textContent, "For more stations tap each.");
  chat.ready();
  await flush();
  assert.deepEqual([...new Set(chat.renders()[0].message.data.rows.map((r) => r.id))], ["urn:a"]);
  assert.equal(chat.$("#c-stations-hint").textContent, "E.g. Alpha, Beta.");
  chat.pick("c-range", "PT24H");
  await flush();
  chat.pick("c-attr" in {} ? "c-attr" : "c-gran", "PT1H");
  await flush();
  assert.equal(chat.$("#c-method").parentNode.hidden, false);
});

test("#show: a parent on our hosts hands a chart or a map, heights and errors go back to its origin alone", async () => {
  const shown = page({ hash: "#show", fetch: () => { throw new Error("no read in #show"); } });
  assert.ok(shown.doc.body.classList.contains("show"));
  const parent = shown.parent;
  const chart = { $schema: VL, data: { name: "source" }, mark: "bar" };
  shown.message({ type: "jc-viz-show", spec: chart }, "https://evil.example", parent);
  shown.message({ type: "other" }, "https://www.example.sk", parent);
  shown.message({ type: "jc-viz-show", spec: chart }, "https://www.example.sk", {});
  assert.equal(shown.toParent.length, 0);
  shown.message({ type: "jc-viz-show", spec: chart, rows: [{ a: 1 }], lang: "sk", theme: "dark" }, "https://www.example.sk", parent);
  assert.equal(shown.frame().getAttribute("src"), "render.html");
  assert.equal(shown.doc.documentElement.dataset.theme, "dark");
  assert.equal(shown.doc.documentElement.lang, "sk");
  assert.equal(shown.posts.length, 0, "the renderer has not said it is ready");
  shown.message({ type: "jc-viz-ready" }, "https://www.example.sk", shown.frame().contentWindow);
  shown.ready();
  assert.deepEqual(shown.posts[0].message, { type: "jc-viz-render", spec: chart, data: { format: "json", rows: [{ a: 1 }] }, lang: "sk", theme: "dark" });
  assert.equal(shown.posts[0].origin, "*");
  shown.ready();
  assert.equal(shown.posts.length, 1, "sent once");
  shown.message({ type: "jc-viz-height", height: 50 }, "null", shown.frame().contentWindow);
  assert.equal(shown.frame().style.height, "120px");
  assert.deepEqual(shown.toParent.at(-1), { message: { type: "jc-viz-height", height: 0 }, origin: "https://www.example.sk" });
  shown.message(undefined, "null", shown.frame().contentWindow);
  shown.message({ type: "jc-viz-error" }, "null", shown.frame().contentWindow);
  assert.deepEqual(shown.toParent.at(-1).message, { type: "jc-viz-error", message: "The chart could not be drawn." });
  shown.win.dispatchEvent(new shown.win.Event("resize"));
  assert.equal(shown.toParent.at(-1).message.type, "jc-viz-height");
  const plain = page({ hash: "#show", fetch: () => null });
  plain.message({ type: "jc-viz-show", spec: chart, lang: "Slovak" }, "https://example.sk", plain.parent);
  plain.ready();
  assert.equal(plain.posts[0].message.lang, "en");
  assert.equal(plain.posts[0].message.theme, "light");
  assert.deepEqual(plain.posts[0].message.data.rows, []);
});

test("#show: a map goes to the map page in this origin, and a refused spec is said to the parent", async () => {
  const shown = page({ hash: "#show", fetch: () => null });
  const map = { $schema: "urn:joinedcontext:map:v1", title: "Stanice", label: "name", popup: [{ field: "name", label: "Názov" }], data: { type: "FeatureCollection", features: [{ type: "Feature", geometry: { type: "Point", coordinates: [19.1, 48.7] }, properties: { name: "A" } }] } };
  shown.message({ type: "jc-viz-show", spec: map }, "https://www.example.sk", shown.parent);
  const frame = shown.$("#map");
  assert.equal(shown.$("#frame"), null, "the map page takes the renderer's place");
  assert.equal(frame.title, "Stanice");
  assert.equal(frame.getAttribute("src"), "map.html");
  assert.equal(shown.$("#title").textContent, "Stanice");
  shown.message({ type: "jc-viz-ready" }, "null", frame.contentWindow);
  assert.equal(shown.posts.length, 0, "the map page speaks from this origin");
  shown.message({ type: "jc-viz-ready" }, "https://viz.example.sk", frame.contentWindow);
  assert.deepEqual(shown.posts[0].message, { type: "jc-viz-render", spec: map, lang: "en", theme: "light" });
  assert.equal(shown.posts[0].origin, "https://viz.example.sk");
  shown.message({ type: "jc-viz-height", height: 400 }, "https://viz.example.sk", frame.contentWindow);
  assert.equal(frame.style.height, "400px");
  const untitled = page({ hash: "#show", fetch: () => null });
  untitled.message({ type: "jc-viz-show", spec: { ...map, title: undefined } }, "https://www.example.sk", untitled.parent);
  assert.equal(untitled.$("#map").title, "Map");
  const refused = page({ hash: "#show", fetch: () => null });
  refused.message({ type: "jc-viz-show", spec: { $schema: VL, data: { url: "https://evil.example/x" }, mark: "bar" } }, "https://www.example.sk", refused.parent);
  assert.match(refused.toParent.at(-1).message.message, /^The chart was refused: /);
  assert.equal(refused.toParent.at(-1).origin, "https://www.example.sk");
  assert.equal(refused.frame().hidden, true);
});

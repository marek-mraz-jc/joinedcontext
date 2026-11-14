// The chart MCP App view (T-6866, crates/shared/jc-chart/src/chart_view.html), drawn by the chart
// library it inlines (T-6887): the six marks, a hostile title and hostile points. The library sees a
// spec of numbers and constants only: no title or category value reaches its SVG, no expression,
// signal, param, URL or link is built from input, and a library that fails shows the answer's PNG.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { JSDOM, VirtualConsole } from "jsdom";
import { describe, expect, it, vi } from "vitest";

// The page as jc_chart::view_page writes it: the theme (T-6886), then the three library files.
const SRC = resolve(__dirname, "../../../shared/jc-chart/src");
const VENDOR = resolve(__dirname, "../../../../vendor/viz/vendor");
const THEME = readFileSync(resolve(SRC, "theme.json"), "utf8").trimEnd();
const LIBRARY: [string, string][] = [
  ["__JC_VEGA__", "vega-6.4.0.min.js"],
  ["__JC_VEGA_LITE__", "vega-lite-6.5.0.min.js"],
  ["__JC_VEGA_INTERPRETER__", "vega-interpreter-2.3.2.js"],
];
const SOURCE = readFileSync(resolve(SRC, "chart_view.html"), "utf8").replace("__JC_THEME__", THEME);
const RAW = LIBRARY.reduce((page, [mark, file]) => page.replace(mark, () => readFileSync(resolve(VENDOR, file), "utf8")), SOURCE);
const PAGE = RAW.replace("__PORTAL_ORIGIN__", JSON.stringify("https://portal.example.org"));
// The library that fails to load: its first script throws.
const BROKEN = SOURCE.replace("__JC_VEGA__", 'throw new Error("no library");')
  .replace("__JC_VEGA_LITE__", "")
  .replace("__JC_VEGA_INTERPRETER__", "")
  .replace("__PORTAL_ORIGIN__", "null");
const HOSTILE = "</script><img src=x onerror=alert(1)>";
const EXPRESSION = "datum.x + '}' + (alert(1))";
const LINK = "javascript:alert(1)";
const INPUT = [HOSTILE, EXPRESSION, LINK, "PM10 at Kallio", "Kallio", "urn:ngsi-ld:A:1", "station"];
const THEMED = JSON.parse(THEME) as Record<"light" | "dark", { background: string; range: { category: string[] } }>;
// What a spec may never hold (T-6887): Vega's ways to run text as code or to reach out.
const FORBIDDEN = new Set(["expr", "signal", "signals", "params", "url", "href", "labelExpr", "transform", "calculate", "filter"]);
const PNG = "iVBORw0KGgo=";

type Result = Record<string, unknown>;

function open(page: string = PAGE) {
  // Script errors of the page; jsdom's missing canvas is not one (Vega measures text without it).
  const errors: string[] = [];
  const console = new VirtualConsole();
  console.on("jsdomError", (e) => { if (!e.message.startsWith("Not implemented")) errors.push(e.message); });
  const dom = new JSDOM(page, {
    runScripts: "dangerously",
    pretendToBeVisual: true,
    virtualConsole: console,
    // Vega-Lite uses structuredClone, which every browser has and jsdom does not.
    beforeParse: (window) => Object.assign(window, { structuredClone }),
  });
  const win = dom.window as unknown as Window & typeof globalThis & { vegaLite?: { compile: (spec: unknown) => unknown } };
  const alert = vi.fn();
  win.alert = alert;
  const fetch = vi.fn(() => Promise.reject(new Error("no network")));
  win.fetch = fetch as unknown as typeof win.fetch;
  // Every Vega-Lite spec the page compiles.
  const specs: unknown[] = [];
  const lib = win.vegaLite;
  if (lib) {
    const compile = lib.compile.bind(lib);
    lib.compile = (spec: unknown) => {
      specs.push(JSON.parse(JSON.stringify(spec)));
      return compile(spec);
    };
  }
  // Every message the page sends its host (its parent is itself in a test window).
  const sent: { method?: string }[] = [];
  win.postMessage = ((message: { method?: string }) => { sent.push(message); }) as typeof win.postMessage;
  const post = (data: unknown, source: unknown = win) =>
    win.dispatchEvent(new win.MessageEvent("message", { data, source: source as Window }));
  post({ jsonrpc: "2.0", id: 1, result: { hostContext: { theme: "light" } } });
  return { win, doc: win.document, alert, fetch, specs, sent, post, errors };
}

// The library draws asynchronously: wait until every chart drew or the picture stands in.
async function settled(doc: Document) {
  for (let i = 0; i < 200; i++) {
    const plots = doc.querySelectorAll(".plot").length;
    const drawn = doc.querySelectorAll(".plot .canvas svg").length;
    if ((plots && drawn === plots) || doc.querySelector(".fallback")) return;
    await new Promise((r) => setTimeout(r, 10));
  }
  throw new Error("the chart never drew");
}

function result(mark: string, points: unknown[], extra: Result = {}): Result {
  return {
    content: [{ type: "image", data: PNG, mimeType: "image/png" }],
    structuredContent: {
      rows: points.length,
      sources: [{ endpoint: "air" }],
      categories: { x: [{ label: "#1", value: HOSTILE }, { label: "#2", value: "Kallio" }], color: [{ label: "#1", value: "urn:ngsi-ld:A:1" }] },
      untrusted: true,
      ...extra,
    },
    _meta: {
      "joinedcontext.com/chartSpec": {
        mark,
        title: "PM10 at Kallio",
        width: 800,
        height: 450,
        x: { scale: mark === "bar" || mark === "rule" ? "nominal" : "temporal", title: "time" },
        y: mark === "arc" ? null : { scale: "quantitative", title: "PM10" },
        color: { scale: "nominal", title: "station" },
        theta: mark === "arc" ? { scale: "quantitative", title: "PM10" } : null,
      },
      "joinedcontext.com/chartPoints": points,
      "joinedcontext.com/pointsTruncated": false,
    },
  };
}

// The library's SVG: no link, no script, no event handler, no markup in a value, and none of the
// answer's text: every title and category value stays in the page's own HTML.
function checkSvg(doc: Document) {
  const svgs = Array.from(doc.querySelectorAll("svg"));
  expect(svgs.length).toBeGreaterThan(0);
  for (const svg of svgs) {
    expect(svg.closest(".canvas")).not.toBeNull();
    for (const node of Array.from(svg.querySelectorAll("*")).concat(svg)) {
      expect(["a", "script", "foreignObject", "image", "use", "iframe", "style"]).not.toContain(node.localName);
      for (const attr of Array.from(node.attributes)) {
        expect(attr.name.startsWith("on") || /href/i.test(attr.name), `${node.localName} ${attr.name}`).toBe(false);
        expect(/[<>]|javascript:/i.test(attr.value), `${node.localName} ${attr.name}="${attr.value}"`).toBe(false);
        if (attr.value.includes("url(")) expect(attr.value, attr.name).toMatch(/^url\(#[A-Za-z0-9_-]+\)$/);
      }
    }
    for (const text of INPUT) expect(svg.outerHTML).not.toContain(text);
  }
  return svgs[0];
}

// No key of a spec is one of Vega's code or reach-out members, and no value is the answer's text.
function checkSpec(spec: unknown, path = "spec") {
  if (Array.isArray(spec)) spec.forEach((v, i) => checkSpec(v, `${path}[${i}]`));
  else if (spec && typeof spec === "object") {
    for (const [k, v] of Object.entries(spec)) {
      expect(FORBIDDEN.has(k), `${path}.${k}`).toBe(false);
      checkSpec(v, `${path}.${k}`);
    }
  } else if (typeof spec === "string") {
    for (const text of INPUT) expect(spec.includes(text), `${path} = ${spec}`).toBe(false);
  }
}

function attrsHoldNoMarkup(doc: Document) {
  for (const node of Array.from(doc.body.querySelectorAll("*"))) {
    for (const attr of Array.from(node.attributes)) {
      expect(attr.value.includes("<") || attr.name.startsWith("on"), `${attr.name}`).toBe(false);
    }
  }
}

// A whole Dashboard's answer (T-6867): one tile per view, in order.
function dashboard(views: Result[], tiles: Result[]): Result {
  return {
    content: [{ type: "image", data: "", mimeType: "image/png" }],
    structuredContent: { dashboard: "air-quality", widgets: tiles, rows: 5, sources: [], untrusted: true },
    _meta: { "joinedcontext.com/dashboard": views },
  };
}

const TIMED = [
  { x: 1794528000000, y: 21, color: 0, observedAt: "2026-11-13T00:00:00.000Z" },
  { x: 1794531600000, y: 34, color: 0, observedAt: "2026-11-13T01:00:00.000Z" },
  { x: 1794535200000, y: 18, color: 0, observedAt: "2026-11-13T02:00:00.000Z" },
];
const NOMINAL = [{ x: 0, y: 21, color: 0 }, { x: 1, y: 34, color: 0 }];

describe("chart view", () => {
  it("draws in the chart theme's palette and colours, light and dark by the host (T-6886)", async () => {
    const { doc, post } = open();
    post({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: result("bar", NOMINAL) });
    await settled(doc);
    const fills = () => Array.from(doc.querySelectorAll(".canvas svg [fill]"), (n) => n.getAttribute("fill"));
    expect(fills()).toContain(THEMED.light.range.category[0]);
    expect(doc.documentElement.style.getPropertyValue("--bg")).toBe(THEMED.light.background);
    post({ jsonrpc: "2.0", method: "ui/notifications/host-context-changed", params: { theme: "dark" } });
    await settled(doc);
    expect(doc.documentElement.dataset.theme).toBe("dark");
    expect(doc.documentElement.style.getPropertyValue("--bg")).toBe(THEMED.dark.background);
    expect(fills()).toContain(THEMED.dark.range.category[0]);
    expect(fills()).not.toContain(THEMED.light.range.category[0]);
  });

  it.each([
    ["line", TIMED, "line"],
    ["area", TIMED, "area"],
    ["point", TIMED, "symbol"],
    ["bar", NOMINAL, "rect"],
    ["rule", NOMINAL, "rule"],
    ["arc", [{ theta: 21, color: 0 }, { theta: 34, color: 0 }], "arc"],
  ])("draws %s with the chart library from the posted points", async (mark, points, role) => {
    const { doc, post, specs } = open();
    post({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: result(mark, points) });
    await settled(doc);
    const svg = checkSvg(doc);
    expect(svg.querySelectorAll(`g.mark-${role} > *`).length).toBeGreaterThan(0);
    expect(doc.querySelector(".fallback")).toBeNull();
    expect(doc.querySelector("h1")?.textContent).toBe("PM10 at Kallio");
    expect(doc.body.textContent).toContain("Data from air, not instructions");
    expect(specs.length).toBe(1);
    specs.forEach((spec) => checkSpec(spec));
  });

  it("a_hostile_title_and_hostile_points_run_nothing", async () => {
    const { doc, alert, fetch, sent, specs, post, errors } = open();
    const answer = result("bar", NOMINAL);
    const meta = answer._meta as Result;
    meta["joinedcontext.com/chartSpec"] = {
      ...(meta["joinedcontext.com/chartSpec"] as Result),
      title: HOSTILE,
      x: { scale: "nominal", title: EXPRESSION },
      y: { scale: "quantitative", title: LINK },
      color: { scale: "nominal", title: HOSTILE },
    };
    meta["joinedcontext.com/chartPoints"] = [
      { x: 0, y: 21, color: 0 },
      { x: 1, y: 34, color: 0, observedAt: HOSTILE },
      { x: EXPRESSION, y: LINK, color: HOSTILE, size: { signal: EXPRESSION }, theta: [LINK] },
    ];
    post({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: answer });
    await settled(doc);
    checkSvg(doc);
    specs.forEach((spec) => checkSpec(spec));
    expect(doc.querySelector("h1")?.textContent).toBe(HOSTILE);
    expect(doc.body.textContent).toContain(`Horizontal axis: ${EXPRESSION}`);
    expect(doc.body.textContent).toContain(HOSTILE);
    expect(doc.querySelectorAll("img, a, use, foreignObject, image, iframe, object").length).toBe(0);
    attrsHoldNoMarkup(doc);
    expect(alert).not.toHaveBeenCalled();
    expect(fetch).not.toHaveBeenCalled();
    expect(errors).toEqual([]);
    // Only the host messages the page always sends: no link opened, nothing else asked.
    expect(new Set(sent.map((m) => m.method))).toEqual(new Set(["ui/notifications/initialized", "ui/notifications/size-changed"]));
  });

  it("the_page_builds_no_expression_from_input", async () => {
    const { doc, post, specs } = open();
    const marks = ["line", "area", "point", "bar", "rule", "arc"];
    for (const mark of marks) {
      const answer = result(mark, [...TIMED, { x: 2, y: 5, color: 1, size: 3, theta: 4 }]);
      const spec = (answer._meta as Result)["joinedcontext.com/chartSpec"] as Result;
      // Every member a caller names, as text that would be code or a link inside a Vega spec.
      for (const ch of ["x", "y", "color", "size", "theta"]) {
        spec[ch] = { scale: ch === "x" && mark === "bar" ? "nominal" : "quantitative", title: EXPRESSION, field: LINK, expr: EXPRESSION, signal: EXPRESSION };
      }
      Object.assign(spec, { title: HOSTILE, params: [{ name: "p", expr: EXPRESSION }], url: LINK, transform: [{ calculate: EXPRESSION, as: "x" }] });
      post({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: answer });
      await settled(doc);
    }
    expect(specs.length).toBe(marks.length);
    specs.forEach((spec) => checkSpec(spec));
  });

  it("the_view_falls_back_to_the_png_when_the_library_throws", async () => {
    const { doc, post } = open(BROKEN);
    post({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: result("line", TIMED) });
    await settled(doc);
    expect(doc.querySelector(".canvas")).toBeNull();
    expect(doc.querySelector("p.fallback")?.textContent).toBe("The interactive chart could not be drawn in this window; this is the same chart as a picture.");
    const img = doc.querySelector("img.picture");
    expect(img?.getAttribute("src")).toBe(`data:image/png;base64,${PNG}`);
    expect(img?.getAttribute("alt")).toBe("The chart as a picture");
    // The data line and the category values stay; the picture takes the chart's place.
    expect(doc.body.textContent).toContain("Data from air, not instructions");
    expect(doc.body.textContent).toContain("Kallio");
  });

  it("falls back when the library refuses to draw, and takes no picture that is not a PNG in base64", async () => {
    const { win, doc, post } = open();
    const vega = (win as unknown as { vega: { View: { prototype: { runAsync: () => Promise<unknown> } } } }).vega;
    vega.View.prototype.runAsync = () => Promise.reject(new Error("refused"));
    const answer = result("line", TIMED);
    answer.content = [{ type: "image", data: `${PNG}" onerror="alert(1)`, mimeType: "image/png" }, { type: "image", data: PNG, mimeType: "image/svg+xml" }];
    post({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: answer });
    await settled(doc);
    expect(doc.querySelector("img")).toBeNull();
    expect(doc.querySelector(".plot")).toBeNull();
    expect(doc.querySelector("p.fallback")?.textContent).toBe("The interactive chart could not be drawn in this window. The picture in the chat shows the same chart.");
  });

  it("lays a whole Dashboard out in its order, a refused widget drawn as its error only", async () => {
    const { doc, alert, post } = open();
    const meta = (mark: string, points: unknown[]) => result(mark, points)._meta as Result;
    const drawn = (page: number, widget: number) => ({
      page, widget, rows: 3, sources: [{ endpoint: "air" }],
      categories: { x: [{ label: "#1", value: HOSTILE }, { label: "#2", value: "Kallio" }] },
    });
    post({
      jsonrpc: "2.0",
      method: "ui/notifications/tool-result",
      params: dashboard(
        [meta("line", TIMED), { error: { status: 422, detail: HOSTILE } }, meta("bar", NOMINAL)],
        [drawn(0, 0), { page: 0, widget: 1, error: { status: 422, detail: HOSTILE } }, drawn(1, 0)],
      ),
    });
    await settled(doc);
    checkSvg(doc);
    const tiles = Array.from(doc.querySelectorAll("section.tile"));
    expect(tiles.length).toBe(3);
    expect(tiles[0].querySelector("svg g.mark-line")).not.toBeNull();
    expect(tiles[1].querySelector("svg")).toBeNull();
    expect(tiles[1].querySelector("h2")?.textContent).toBe("Page 1, widget 2: not drawn (422)");
    expect(tiles[1].querySelector("p.refused")?.textContent).toBe(HOSTILE);
    expect(tiles[2].querySelector("svg g.mark-rect")).not.toBeNull();
    // One tooltip per chart, each plot labelled by its own title.
    expect(doc.querySelectorAll(".tip").length).toBe(2);
    expect(tiles[2].querySelector(".plot")?.getAttribute("aria-labelledby")).toBe("chart-title-2");
    expect(doc.getElementById("chart-title-2")?.textContent).toBe("PM10 at Kallio");
    expect(doc.body.textContent).toContain("2 of 3 widgets drawn");
    expect(doc.querySelectorAll("img, a, use, foreignObject, image, iframe, object").length).toBe(0);
    attrsHoldNoMarkup(doc);
    expect(alert).not.toHaveBeenCalled();
  });

  it("reads the values of a point by keyboard, as text", async () => {
    const { win, doc, post } = open();
    post({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: result("bar", NOMINAL) });
    await settled(doc);
    const plot = doc.querySelector(".plot") as HTMLElement;
    expect(plot.tabIndex).toBe(0);
    expect(plot.getAttribute("role")).toBe("img");
    plot.dispatchEvent(new win.KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    const tip = doc.querySelector(".tip") as HTMLElement;
    expect(tip.hidden).toBe(false);
    expect(tip.textContent).toBe(`time: #1 ${HOSTILE}\nPM10: 21\nstation: #1 urn:ngsi-ld:A:1`);
    expect(tip.children.length).toBe(0);
    expect(doc.querySelector("p.sr-only")?.textContent).toBe(tip.textContent);
    plot.dispatchEvent(new win.KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(tip.hidden).toBe(true);
  });

  it("reads a malformed point as no value, never as markup or a link", async () => {
    const { doc, alert, post } = open();
    const bad = { x: "1794528000000", y: "<b>34</b>", color: "javascript:alert(1)", size: { valueOf: 3 }, observedAt: "<img src=x>" };
    post({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: result("line", [bad, ...TIMED]) });
    await settled(doc);
    const svg = checkSvg(doc);
    expect(svg.outerHTML).not.toContain("<b>");
    expect(doc.querySelectorAll("img, a, use, foreignObject, image").length).toBe(0);
    // The three good points draw one line; the bad one has no x or y and is left out.
    expect(svg.querySelectorAll("g.mark-line path").length).toBe(1);
    attrsHoldNoMarkup(doc);
    expect(alert).not.toHaveBeenCalled();
  });

  it("says what to do next for a Dashboard without widgets", () => {
    const { doc, post } = open();
    post({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: dashboard([], []) });
    expect(doc.querySelector("svg")).toBeNull();
    expect(doc.querySelector("h1")?.textContent).toBe("Dashboard air-quality");
    expect(doc.body.textContent).toContain("no widgets yet");
  });

  it("draws nothing for a refusal and says why", () => {
    const { doc, post } = open();
    post({
      jsonrpc: "2.0",
      method: "ui/notifications/tool-result",
      params: { isError: true, content: [{ type: "text", text: "no Endpoint other-air in project p" }] },
    });
    expect(doc.querySelector("svg")).toBeNull();
    expect(doc.querySelector("h1")?.textContent).toBe("The chart was refused");
    expect(doc.body.textContent).toContain("no Endpoint other-air in project p");
  });

  it("ignores a message from anyone but its parent, or not JSON-RPC 2.0", () => {
    const { doc, post } = open();
    post({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: result("line", TIMED) }, null);
    post({ method: "ui/notifications/tool-result", params: result("line", TIMED) });
    expect(doc.querySelector("svg")).toBeNull();
  });

  it("links only to the Portal origin, once tool-input names the project", () => {
    const { doc, post } = open();
    post({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: result("line", TIMED) });
    expect(doc.querySelector("a")).toBeNull();
    post({ jsonrpc: "2.0", method: "ui/notifications/tool-input", params: { arguments: { project: "bratislava" } } });
    expect(doc.querySelector("a")?.getAttribute("href")).toBe("https://portal.example.org/projects/bratislava/endpoints/air");
  });

  it("draws no link when the server wrote no origin (the gateway's door, T-6882)", async () => {
    const { doc, post } = open(RAW.replace("__PORTAL_ORIGIN__", "null"));
    post({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: result("line", TIMED) });
    post({ jsonrpc: "2.0", method: "ui/notifications/tool-input", params: { arguments: { project: "bratislava" } } });
    await settled(doc);
    expect(doc.querySelector("svg")).not.toBeNull();
    expect(doc.querySelector("a")).toBeNull();
  });

  it("keeps the Portal's truncation line on its door and says Not complete on the gateway's", () => {
    const cut = (page?: string) => {
      const { doc, post } = open(page);
      const answer = result("line", TIMED, { rows: 9000 });
      (answer._meta as Result)["joinedcontext.com/pointsTruncated"] = true;
      post({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: answer });
      return Array.from(doc.querySelectorAll("p"), (p) => p.textContent).join("\n");
    };
    expect(cut()).toContain("Shows the first 3 points of 9,000 rows; the picture in the chat draws up to 20,000.");
    const raw = RAW;
    const gateway = cut(raw.replace("__PORTAL_ORIGIN__", "null"));
    expect(gateway).toContain("Not complete: the read has more rows than this chart draws (3 points shown).");
    expect(gateway).not.toContain("Shows the first");
  });
});

// The chart MCP App view (T-6866, crates/portal/src/mcp/chart_view.html): its draw over the six
// marks and a hostile point. Only the allowed SVG elements appear, no attribute value comes from
// the data but a number, and a category value is text, never markup.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { JSDOM } from "jsdom";
import { describe, expect, it, vi } from "vitest";

const PAGE = readFileSync(resolve(__dirname, "../../src/mcp/chart_view.html"), "utf8").replace(
  "__PORTAL_ORIGIN__",
  JSON.stringify("https://portal.example.org"),
);
const HOSTILE = "</script><img src=x onerror=alert(1)>";
const ELEMENTS = new Set(["svg", "g", "path", "rect", "circle", "line", "text", "title"]);
const CONSTANTS = new Set([
  "grid", "axis", "tick", "axis-title", "focus", "none", "middle", "end", "img", "chart-title", "#ffffff",
  "#2563eb", "#ea580c", "#16a34a", "#dc2626", "#9333ea", "#0d9488", "#ca8a04", "#db2777",
]);
const NUMERIC = /^[-0-9.eE ,MLAZ]+$/;
const ROTATE = /^rotate\(-?[0-9. ]+\)$/;
const LABEL = /^chart-title-[0-9]+$/;

type Result = Record<string, unknown>;

function open() {
  const dom = new JSDOM(PAGE, { runScripts: "dangerously" });
  const win = dom.window;
  const alert = vi.fn();
  win.alert = alert;
  const post = (data: unknown, source: unknown = win) =>
    win.dispatchEvent(new win.MessageEvent("message", { data, source: source as Window }));
  post({ jsonrpc: "2.0", id: 1, result: { hostContext: { theme: "light" } } });
  return { win, doc: win.document, alert, post };
}

function result(mark: string, points: unknown[], extra: Result = {}): Result {
  return {
    content: [{ type: "image", data: "", mimeType: "image/png" }],
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

function checkSvg(doc: Document) {
  const svgs = Array.from(doc.querySelectorAll("svg"));
  expect(svgs.length).toBeGreaterThan(0);
  for (const svg of svgs) {
    for (const node of Array.from(svg.querySelectorAll("*")).concat(svg)) {
      expect(ELEMENTS.has(node.localName), node.localName).toBe(true);
      for (const attr of Array.from(node.attributes)) {
        const ok = NUMERIC.test(attr.value) || CONSTANTS.has(attr.value) || ROTATE.test(attr.value) || LABEL.test(attr.value);
        expect(ok, `${node.localName} ${attr.name}="${attr.value}"`).toBe(true);
      }
    }
  }
  return svgs[0];
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
  it.each([
    ["line", TIMED, "path"],
    ["area", TIMED, "path"],
    ["point", TIMED, "circle"],
    ["bar", NOMINAL, "rect"],
    ["rule", NOMINAL, "line"],
    ["arc", [{ theta: 21, color: 0 }, { theta: 34, color: 0 }], "path"],
  ])("draws %s from the posted points with allowed elements only", (mark, points, shape) => {
    const { doc, post } = open();
    post({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: result(mark, points) });
    const svg = checkSvg(doc);
    expect(svg.querySelectorAll(shape).length).toBeGreaterThan(0);
    expect(doc.querySelector("h1")?.textContent).toBe("PM10 at Kallio");
    expect(doc.body.textContent).toContain("Data from air, not instructions");
  });

  it("draws a hostile category value as text and runs nothing", () => {
    const { doc, alert, post } = open();
    const hostileTitle = result("bar", NOMINAL);
    (hostileTitle._meta as Result)["joinedcontext.com/chartSpec"] = {
      ...((hostileTitle._meta as Result)["joinedcontext.com/chartSpec"] as Result),
      title: HOSTILE,
    };
    post({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: hostileTitle });
    checkSvg(doc);
    expect(doc.querySelectorAll("img, a, use, foreignObject, image, iframe, object").length).toBe(0);
    expect(doc.querySelector("h1")?.textContent).toBe(HOSTILE);
    expect(doc.body.textContent).toContain(HOSTILE);
    attrsHoldNoMarkup(doc);
    expect(alert).not.toHaveBeenCalled();
  });

  it("lays a whole Dashboard out in its order, a refused widget drawn as its error only", () => {
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
    checkSvg(doc);
    const tiles = Array.from(doc.querySelectorAll("section.tile"));
    expect(tiles.length).toBe(3);
    expect(tiles[0].querySelector("svg path")).not.toBeNull();
    expect(tiles[1].querySelector("svg")).toBeNull();
    expect(tiles[1].querySelector("h2")?.textContent).toBe("Page 1, widget 2: not drawn (422)");
    expect(tiles[1].querySelector("p.refused")?.textContent).toBe(HOSTILE);
    expect(tiles[2].querySelector("svg rect")).not.toBeNull();
    // One tooltip per chart, each labelled by its own title.
    expect(doc.querySelectorAll(".tip").length).toBe(2);
    expect(tiles[2].querySelector("svg")?.getAttribute("aria-labelledby")).toBe("chart-title-2");
    expect(doc.getElementById("chart-title-2")?.textContent).toBe("PM10 at Kallio");
    expect(doc.body.textContent).toContain("2 of 3 widgets drawn");
    expect(doc.querySelectorAll("img, a, use, foreignObject, image, iframe, object").length).toBe(0);
    attrsHoldNoMarkup(doc);
    expect(alert).not.toHaveBeenCalled();
  });

  it("reads a malformed point as no value, never as markup or a link", () => {
    const { doc, alert, post } = open();
    const bad = { x: "1794528000000", y: "<b>34</b>", color: "javascript:alert(1)", size: { valueOf: 3 }, observedAt: "<img src=x>" };
    post({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: result("line", [bad, ...TIMED]) });
    const svg = checkSvg(doc);
    expect(svg.outerHTML).not.toContain("javascript");
    expect(svg.outerHTML).not.toContain("<b>");
    expect(doc.querySelectorAll("img, a, use, foreignObject, image").length).toBe(0);
    // The three good points draw one line; the bad one has no x or y and is left out.
    expect(svg.querySelectorAll("path").length).toBe(1);
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
});

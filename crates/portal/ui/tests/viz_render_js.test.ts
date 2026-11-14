/**
 * The open-data chart renderer (vendor/viz/site/render.js, csv.js) draws under a CSP without
 * 'unsafe-eval' (T-6865): no step from a `csv` source's text to the drawn view may build code with
 * `Function`. jc-prod's Banská Bystrica PM10 dashboard showed the CSP refusal instead of its chart,
 * because vega's CSV reader (d3-dsv) builds its rows with `new Function`.
 */
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import * as vega from "vega";
import { expressionInterpreter } from "vega-interpreter";
import * as vegaLite from "vega-lite";
import { afterEach, describe, expect, it } from "vitest";

const site = resolve(dirname(fileURLToPath(import.meta.url)), "../../../../vendor/viz/site");
const source = (name: string) => readFileSync(resolve(site, name), "utf8");

function loadCsv(): (text: string) => Record<string, string>[] {
  const module = { exports: {} as (text: string) => Record<string, string>[] };
  new Function("module", "self", source("csv.js"))(module, {});
  return module.exports;
}
const csvRecords = loadCsv();

// file.csv of jc-prod's Endpoint mluyob4nz52lok3ssk7pgn5vwt (type AirQualityObserved, 2026-11-14):
// its header and first row as served, two more stations in the same shape.
const CSV =
  "id,type,dateObserved.value,location.value.type,location.value.coordinates[0],location.value.coordinates[1],name.languageMap.en,name.languageMap.sk,observedAt.value,pm10.value,pm10.unitCode,pm10.observedAt\n" +
  'urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:eea-SK0263A,AirQualityObserved,2026-11-14T07:00:00Z,Point,19.115268,48.733256,"Station SK0263A, urban background","Stanica SK0263A, mestské pozadie",,9.5909,GQ,2026-11-14T07:00:00Z\n' +
  'urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:eea-SK0004A,AirQualityObserved,2026-11-14T07:00:00Z,Point,19.155,48.7356,"Station SK0004A, ""Štefánikovo nábrežie"" traffic","Stanica SK0004A, doprava",,21.4,GQ,2026-11-14T07:00:00Z\r\n' +
  "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:eea-SK0118A,AirQualityObserved,2026-11-14T07:00:00Z,Point,19.1408,48.7417,Station SK0118A,Stanica SK0118A,,14,GQ,2026-11-14T07:00:00Z\n";

// The dashboard widget's spec as jc-prod's catalogue serves it (view-01a1d01f-…).
const SPEC = {
  $schema: "https://vega.github.io/schema/vega-lite/v5.json",
  data: { name: "source" },
  mark: { type: "bar", tooltip: true },
  encoding: {
    y: { field: "name\\.languageMap\\.sk", type: "nominal", sort: "-x" },
    x: { field: "pm10\\.value", type: "quantitative" },
    tooltip: [{ field: "pm10\\.observedAt", type: "temporal", timeUnit: "yearmonthdatehoursminutes" }],
  },
};

// What the CSP does to a page without 'unsafe-eval': every Function construction throws.
const realFunction = globalThis.Function;
function forbidFunction() {
  globalThis.Function = function () {
    throw new EvalError("Function() is refused (no 'unsafe-eval')");
  } as unknown as FunctionConstructor;
}
afterEach(() => {
  globalThis.Function = realFunction;
});

describe("csv.js", () => {
  it("reads quoted fields, doubled quotes and CRLF into header-keyed strings", () => {
    const rows = csvRecords(CSV);
    expect(rows).toHaveLength(3);
    expect(rows[0]["name.languageMap.sk"]).toBe("Stanica SK0263A, mestské pozadie");
    expect(rows[1]["name.languageMap.en"]).toBe('Station SK0004A, "Štefánikovo nábrežie" traffic');
    expect(rows[1]["pm10.observedAt"]).toBe("2026-11-14T07:00:00Z");
    expect(rows[2]["pm10.value"]).toBe("14");
    expect(rows[0]["observedAt.value"]).toBe("");
  });

  it("gives no record for an empty text, a header alone or blank lines, and fills short rows", () => {
    expect(csvRecords("")).toEqual([]);
    expect(csvRecords("a,b\n")).toEqual([]);
    expect(csvRecords("a,b\n\n1,2\n\n")).toEqual([{ a: "1", b: "2" }]);
    expect(csvRecords("a,b\n1")).toEqual([{ a: "1", b: "" }]);
    expect(csvRecords("a,b\n1,")).toEqual([{ a: "1", b: "" }]);
    expect(csvRecords('a,b\n"x\ny",2')).toEqual([{ a: "x\ny", b: "2" }]);
  });
});

// render.js in a stand-in frame (T-6886): the spec it hands vega-embed, for the host's scheme and width.
function drawn(message: Record<string, unknown>, width = 1440) {
  const self: Record<string, unknown> = { jcRefuse: () => [], jcCsv: csvRecords };
  new Function("self", source("theme.js"))(self);
  const listeners: ((event: unknown) => void)[] = [];
  const parent = { postMessage: () => undefined };
  const win = { parent, innerWidth: width, addEventListener: (_: string, f: (event: unknown) => void) => listeners.push(f) };
  const doc = {
    getElementById: () => ({}),
    documentElement: { dataset: {} as Record<string, string>, lang: "" },
    body: { style: {} as Record<string, string>, getBoundingClientRect: () => ({ height: 100 }) },
  };
  const location = { origin: "https://viz.dev.example.org", hostname: "viz.dev.example.org" };
  let spec: Record<string, unknown> = {};
  const vegaEmbed = (_el: unknown, given: Record<string, unknown>) => {
    spec = given;
    return Promise.resolve();
  };
  class ResizeObserver {
    observe() {}
  }
  new Function("window", "document", "location", "self", "vega", "vegaEmbed", "ResizeObserver", source("render.js"))(
    win, doc, location, self, vega, vegaEmbed, ResizeObserver,
  );
  listeners.forEach((f) => f({ source: parent, origin: location.origin, data: { type: "jc-viz-render", data: { format: "json", rows: [] }, ...message } }));
  return { spec: () => spec as { config: Record<string, Record<string, unknown>> }, doc, theme: self.jcTheme as Record<string, Record<string, unknown>> };
}

describe("render.js draws in the chart theme (T-6886)", () => {
  const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

  it("draws a spec in the light theme unless the host says dark, the spec's own config over it", async () => {
    const light = drawn({ spec: { ...SPEC, config: { axis: { labelColor: "#000000" } } } });
    await tick();
    expect(light.spec().config.range).toEqual(light.theme.light.range);
    expect(light.spec().config.font).toBe(light.theme.light.font);
    expect(light.spec().config.axis.labelColor).toBe("#000000");
    expect(light.spec().config.axis.gridColor).toBe((light.theme.light.axis as Record<string, unknown>).gridColor);
    expect(light.doc.documentElement.dataset.theme).toBe("light");
    expect(light.doc.body.style.backgroundColor).toBe(light.theme.light.background);

    const dark = drawn({ spec: SPEC, theme: "dark" });
    await tick();
    expect(dark.spec().config.range).toEqual(dark.theme.dark.range);
    expect(dark.doc.documentElement.dataset.theme).toBe("dark");
    expect(dark.doc.body.style.backgroundColor).toBe(dark.theme.dark.background);
    // Anything but "dark" is light.
    const odd = drawn({ spec: SPEC, theme: "<script>" });
    await tick();
    expect(odd.doc.documentElement.dataset.theme).toBe("light");
  });

  it("keeps the theme on a phone and puts the legend under the chart", async () => {
    const phone = drawn({ spec: SPEC, theme: "dark" }, 390);
    await tick();
    const config = phone.spec().config;
    expect(config.range).toEqual(phone.theme.dark.range);
    expect(config.legend.orient).toBe("bottom");
    expect(config.legend.labelColor).toBe((phone.theme.dark.legend as Record<string, unknown>).labelColor);
    expect(config.title.limit).toBe(390 - 48);
    expect(config.title.color).toBe((phone.theme.dark.title as Record<string, unknown>).color);
    expect(config.axisY.labelLimit).toBe(130);
  });

  it("the theme passes the renderer's own refusal list", () => {
    const self: Record<string, unknown> = {};
    new Function("self", source("refuse.js"))(self);
    new Function("self", source("theme.js"))(self);
    const refuse = self.jcRefuse as (spec: unknown, domain: string) => unknown[];
    for (const mode of ["light", "dark"]) {
      const spec = { ...SPEC, config: (self.jcTheme as Record<string, unknown>)[mode] };
      expect(refuse(spec, "dev.example.org")).toEqual([]);
    }
  });
});

describe("render.js under a CSP without 'unsafe-eval'", () => {
  it("vega's own CSV reader needs Function: the path the renderer must not take", () => {
    forbidFunction();
    expect(() => vega.read(CSV, { type: "csv", parse: "auto" })).toThrow(EvalError);
  });

  it("reads a csv source through csv.js and draws it with the interpreter, never Function", async () => {
    const js = source("render.js");
    expect(js).toContain("vega.read(self.jcCsv(String(data.text || \"\")), { type: \"json\", parse: \"auto\" })");
    expect(js).not.toMatch(/type: "csv"/);
    expect(js).toMatch(/ast: true,\s*expr: vega\.expressionInterpreter,/);
    const html = source("render.html");
    expect(html.indexOf('src="csv.js"')).toBeGreaterThan(0);
    expect(html.indexOf('src="csv.js"')).toBeLessThan(html.indexOf('src="render.js"'));

    const compiled = vegaLite.compile({ ...SPEC, datasets: { source: [] } } as vegaLite.TopLevelSpec).spec;
    forbidFunction();
    // vega.read takes the records as render.js passes them; its types name only a string.
    const rows = vega.read(csvRecords(CSV) as unknown as string, { type: "json", parse: "auto" }) as Record<string, unknown>[];
    expect(rows[1]["pm10.value"]).toBe(21.4);
    expect(rows[1]["pm10.observedAt"]).toBe(Date.parse("2026-11-14T07:00:00Z"));
    compiled.data = (compiled.data ?? []).map((d) => (d.name === "source" ? { ...d, values: rows } : d));
    const view = new vega.View(vega.parse(compiled, undefined, { ast: true }), {
      expr: expressionInterpreter,
      renderer: "none",
    });
    await view.runAsync();
    type Node = { marktype?: string; items?: Node[] };
    const root = (view.scenegraph() as unknown as { root: Node }).root;
    const marks = root.items?.[0].items?.find((i) => i.marktype === "rect");
    expect(marks?.items).toHaveLength(3);
    view.finalize();
  });
});

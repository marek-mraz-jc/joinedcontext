/**
 * The chart host's toolbar of a temporal chart (vendor/viz/site/host.js, T-7490): range, custom
 * from/to, granularity, method, attribute and stations become one or more reads under the view's
 * Endpoint, and the fragment carries the reader's choice. Every value a link brings is checked
 * against an allow-list before it reaches a URL.
 */
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import * as vegaLite from "vega-lite";
import { describe, expect, it } from "vitest";

const site = resolve(dirname(fileURLToPath(import.meta.url)), "../../../../vendor/viz/site");

type State = { range: string; from: string | null; to: string | null; gran: string; method: string; attr: string | null; stations: string[] };
type Plan = { attrs: string[]; method: string | null; gran: string; forced: string | null; capped: number; urls: string[] };
type Host = {
  defaults: (source: unknown) => State;
  parseState: (hash: string, source: unknown) => State;
  stateHash: (state: State, source: unknown) => string;
  temporalPlan: (source: unknown, state: State, endpoint: string, api: string, now: number) => Plan;
  measure: (attr: string, unitCode: string | null, lang: string, values: unknown[]) => { title: string; percent: boolean };
  interactive: (spec: unknown, opts: Record<string, unknown>) => Record<string, unknown>;
  gapped: (rows: Record<string, unknown>[], attr: string, factor?: number) => Record<string, unknown>[];
};
const module = { exports: {} as Host };
new Function("module", "self", readFileSync(resolve(site, "host.js"), "utf8"))(module, {});
const host = module.exports;

const API = "https://api.smartcity.example.org";
const SLUG = "fudnrvyz5x6iugzs5dudektnpnlvud54";
const NOW = Date.parse("2026-11-21T16:00:00Z");
// praha-pocasi's temperature view as the catalogue serves it (2026-11-21).
const SOURCE = { source: "temporal", type: "WeatherObserved", attrs: ["temperature"], last: "P7D" };
const base = `${API}/api/endpoint/${SLUG}/ngsi-ld/v1/temporal/entities?`;
const params = (url: string) => Object.fromEntries(new URL(url).searchParams);

describe("the toolbar's reads", () => {
  it("reads a preset range `after` now minus the range, raw", () => {
    const plan = host.temporalPlan(SOURCE, host.defaults(SOURCE), SLUG, API, NOW);
    expect(plan.urls).toEqual([
      `${base}type=WeatherObserved&attrs=temperature&timerel=after&timeAt=2026-11-14T16%3A00%3A00Z&format=temporalValues&firstN=1000&limit=1000`,
    ]);
    expect(plan.forced).toBeNull();
  });

  it("reads a custom range `between` its first day and the end of its last", () => {
    const state = { ...host.defaults(SOURCE), range: "custom", from: "2026-11-01", to: "2026-11-03" };
    const [url] = host.temporalPlan(SOURCE, state, SLUG, API, NOW).urls;
    expect(params(url)).toMatchObject({ timerel: "between", timeAt: "2026-11-01T00:00:00Z", endTimeAt: "2026-11-04T00:00:00Z", format: "temporalValues" });
  });

  it("asks hourly and daily aggregates with the method picked", () => {
    for (const [gran, method] of [["PT1H", "avg"], ["P1D", "max"], ["PT1H", "min"]]) {
      const plan = host.temporalPlan(SOURCE, { ...host.defaults(SOURCE), range: "PT24H", gran, method }, SLUG, API, NOW);
      expect(params(plan.urls[0])).toEqual({
        type: "WeatherObserved", attrs: "temperature", timerel: "after", timeAt: gran === "P1D" ? "2026-11-20T00:00:00Z" : "2026-11-20T16:00:00Z",
        format: "aggregatedValues", aggrMethods: method, aggrPeriodDuration: gran, limit: "1000",
      });
    }
  });

  it("starts an aggregated read on a whole hour or UTC day, within the gateway's 31 days", () => {
    const now = Date.parse("2026-11-21T15:10:00Z");
    const at = (range: string, gran: string) =>
      params(host.temporalPlan(SOURCE, { ...host.defaults(SOURCE), range, gran, method: "avg" }, SLUG, API, now).urls[0]).timeAt;
    expect(at("PT24H", "PT1H")).toBe("2026-11-20T15:00:00Z");
    expect(at("PT24H", "P1D")).toBe("2026-11-20T00:00:00Z");
    expect(at("P7D", "P1D")).toBe("2026-11-14T00:00:00Z");
    // A whole first day would make the window 31 days and 15 hours: it starts on the next day.
    expect(at("P31D", "P1D")).toBe("2026-10-22T00:00:00Z");
    expect(at("P31D", "PT1H")).toBe("2026-10-21T16:00:00Z");
  });

  it("turns raw readings past seven days hourly and says so", () => {
    const plan = host.temporalPlan(SOURCE, { ...host.defaults(SOURCE), range: "P31D" }, SLUG, API, NOW);
    expect(plan.forced).toBe("hourly");
    expect(params(plan.urls[0])).toMatchObject({ format: "aggregatedValues", aggrMethods: "avg", aggrPeriodDuration: "PT1H" });
    const week = host.temporalPlan(SOURCE, { ...host.defaults(SOURCE), range: "P7D" }, SLUG, API, NOW);
    expect(week.forced).toBeNull();
  });

  it("walks a window over 31 days as consecutive daily windows of at most 31 days", () => {
    const state = { ...host.defaults(SOURCE), range: "custom", from: "2026-09-01", to: "2026-11-20" };
    const plan = host.temporalPlan(SOURCE, state, SLUG, API, NOW);
    expect(plan.forced).toBe("daily");
    expect(plan.capped).toBe(0);
    const windows = plan.urls.map(params);
    expect(windows.map((w) => [w.timeAt, w.endTimeAt])).toEqual([
      ["2026-09-01T00:00:00Z", "2026-10-02T00:00:00Z"],
      ["2026-10-02T00:00:00Z", "2026-11-02T00:00:00Z"],
      ["2026-11-02T00:00:00Z", "2026-11-21T00:00:00Z"],
    ]);
    for (const w of windows) {
      expect(w).toMatchObject({ timerel: "between", format: "aggregatedValues", aggrPeriodDuration: "P1D" });
      expect(Date.parse(w.endTimeAt) - Date.parse(w.timeAt)).toBeLessThanOrEqual(31 * 86400000);
    }
  });

  it("caps the walk at its newest twelve windows and names the days shown", () => {
    const state = { ...host.defaults(SOURCE), range: "custom", from: "2024-01-01", to: "2026-11-20" };
    const plan = host.temporalPlan(SOURCE, state, SLUG, API, NOW);
    expect(plan.urls).toHaveLength(12);
    // Eleven whole 31-day windows and the newest, shorter one (counted from 2024-01-01).
    expect(plan.capped).toBe(342);
    expect(params(plan.urls[11]).endTimeAt).toBe("2026-11-21T00:00:00Z");
  });

  it("refuses dates out of order, in the future or not a day", () => {
    for (const [from, to] of [["2026-11-05", "2026-11-01"], ["2026-12-01", "2026-12-02"], ["2026-02-30", "2026-03-01"]]) {
      expect(() => host.temporalPlan(SOURCE, { ...host.defaults(SOURCE), range: "custom", from, to }, SLUG, API, NOW)).toThrow("dates");
    }
  });

  it("reads only the attribute picked, and only one the view lists", () => {
    const source = { ...SOURCE, attrs: ["temperature", "relativeHumidity"] };
    const state = host.parseState("#view-01a1c5e4-c153-712b-887a-282522e42033&a=relativeHumidity", source);
    expect(params(host.temporalPlan(source, state, SLUG, API, NOW).urls[0]).attrs).toBe("relativeHumidity");
    expect(host.parseState("#v&a=pm10", source).attr).toBeNull();
  });
});

describe("the fragment", () => {
  const id = "view-01a1c5e4-c153-712b-887a-282522e42033";
  it("round-trips every control, the view's defaults left out", () => {
    const state: State = {
      range: "custom", from: "2026-11-01", to: "2026-11-07", gran: "P1D", method: "max", attr: null,
      stations: ["urn:ngsi-ld:WeatherObserved:hel.fi:praha-mesto:chmi-0-20000-0-11518", "urn:ngsi-ld:WeatherObserved:x:2"],
    };
    const hash = host.stateHash(state, SOURCE);
    expect(hash).toBe("r=custom&from=2026-11-01&to=2026-11-07&g=P1D&m=max&s=urn%3Angsi-ld%3AWeatherObserved%3Ahel.fi%3Apraha-mesto%3Achmi-0-20000-0-11518,urn%3Angsi-ld%3AWeatherObserved%3Ax%3A2");
    expect(host.parseState(`#${id}&inline&lang=sk&${hash}`, SOURCE)).toEqual(state);
    expect(host.stateHash(host.defaults(SOURCE), SOURCE)).toBe("");
  });

  it("drops a value that is not on the allow-list, never repairs it", () => {
    const state = host.parseState(`#${id}&r=P90D&g=PT5M&m=eval&from=x&s=javascript:alert(1),urn:ngsi-ld:A:1&a=../x`, SOURCE);
    expect(state).toEqual({ ...host.defaults(SOURCE), stations: ["urn:ngsi-ld:A:1"] });
    // A custom range without both days is the view's own range.
    expect(host.parseState(`#${id}&r=custom&from=2026-11-01`, SOURCE).range).toBe("P7D");
  });

  it("takes nothing from the link when the author hid the toolbar (T-7491)", () => {
    const source = { ...SOURCE, controls: false };
    const hash = `#${id}&r=P31D&g=P1D&m=max&s=urn:ngsi-ld:A:1`;
    expect(host.parseState(hash, source)).toEqual(host.defaults(source));
  });

  it("keeps an authored range, period and method the toolbar does not list", () => {
    const source = { ...SOURCE, last: "P2D", aggrMethod: "sum", aggrPeriod: "PT6H" };
    expect(host.defaults(source)).toMatchObject({ range: "P2D", gran: "PT6H", method: "sum" });
    expect(host.parseState(`#${id}&r=P2D&g=PT6H&m=sum`, source)).toMatchObject({ range: "P2D", gran: "PT6H", method: "sum" });
  });
});

describe("the interactive chart", () => {
  // praha-pocasi's relativeHumidity view (2026-11-21).
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

  it("names the measure and its unit, a model fraction as percent", () => {
    expect(host.measure("relativeHumidity", null, "sk", [0.74, 0.59])).toMatchObject({ title: "Relatívna vlhkosť (%)", percent: true });
    expect(host.measure("relativeHumidity", null, "en", [74, 59])).toMatchObject({ percent: false });
    expect(host.measure("temperature", null, "en", [])).toMatchObject({ title: "Temperature (°C)", percent: false });
    expect(host.measure("noiseLevel", "DB", "en", [])).toMatchObject({ title: "Noise Level (dB)" });
    expect(host.measure("availableSpots", null, "en", [])).toMatchObject({ title: "Available Spots" });
  });

  it("draws the chart at full width with a brush strip and a legend that toggles a station, and compiles", () => {
    const measure = host.measure("relativeHumidity", null, "sk", [0.7]);
    const spec = host.interactive(SPEC, { attr: "relativeHumidity", measure, lang: "sk", width: 1200 });
    type Unit = { width: number; params: { name: string; bind?: string }[]; encoding: { x: { scale: { domain: unknown } }; y: { title: string }; tooltip: { title: string }[] } };
    const [detail, strip] = spec.vconcat as Unit[];
    expect(detail.width).toBe(1200);
    expect(detail.encoding.y.title).toBe("Relatívna vlhkosť (%)");
    expect(detail.encoding.x.scale.domain).toEqual({ param: "brush" });
    expect(detail.params[0]).toMatchObject({ name: "pick", bind: "legend" });
    expect(detail.encoding.tooltip[1].title).toBe("Relatívna vlhkosť (%)");
    expect(detail.encoding.tooltip[0].title).toBe("Čas");
    expect(strip.params[0]).toEqual({ name: "brush", select: { type: "interval", encodings: ["x"] } });
    const vega = vegaLite.compile(spec as unknown as vegaLite.TopLevelSpec).spec;
    expect(vega.data?.some((d) => d.name === "source")).toBe(true);
    // The authored spec is left as it was.
    expect((SPEC.encoding.y as Record<string, unknown>).title).toBeUndefined();
  });

  it("breaks a series' line at a gap over three steps, and only there", () => {
    const row = (name: string, at: string, v: number) => ({ id: name, name, observedAt: at, t: v });
    const rows = host.gapped([
      row("A", "2026-11-18T10:00:00Z", 1), row("A", "2026-11-18T11:00:00Z", 2), row("A", "2026-11-18T12:00:00Z", 3),
      row("A", "2026-11-20T06:00:00Z", 4), row("B", "2026-11-18T10:00:00Z", 5), row("B", "2026-11-18T11:00:00Z", 6),
    ], "t");
    expect(rows.map((r) => [r.name, r.observedAt, r.t])).toEqual([
      ["A", "2026-11-18T10:00:00Z", 1], ["A", "2026-11-18T11:00:00Z", 2], ["A", "2026-11-18T12:00:00Z", 3],
      ["A", "2026-11-18T13:00:00Z", null], ["A", "2026-11-20T06:00:00Z", 4],
      ["B", "2026-11-18T10:00:00Z", 5], ["B", "2026-11-18T11:00:00Z", 6],
    ]);
  });

  it("breaks an aggregated line at one missing bucket", () => {
    const days = ["2026-11-16", "2026-11-17", "2026-11-18", "2026-11-20"].map((d, i) => ({ name: "A", observedAt: `${d}T00:00:00Z`, t: i }));
    expect(host.gapped(days, "t").some((r) => r.t === null)).toBe(false);
    expect(host.gapped(days, "t", 1.5).map((r) => [r.observedAt, r.t])).toEqual([
      ["2026-11-16T00:00:00Z", 0], ["2026-11-17T00:00:00Z", 1], ["2026-11-18T00:00:00Z", 2],
      ["2026-11-19T00:00:00Z", null], ["2026-11-20T00:00:00Z", 3],
    ]);
  });

  it("swaps the authored attribute's fields for the one picked", () => {
    const spec = host.interactive(SPEC, { attr: "temperature", measure: host.measure("temperature", null, "en", []), lang: "en", width: 600, from: "relativeHumidity", to: "temperature" });
    expect(JSON.stringify(spec)).not.toContain("relativeHumidity");
  });

  it("leaves a composed spec composed, at the container's width", () => {
    const layered = { $schema: SPEC.$schema, data: SPEC.data, layer: [{ mark: "line", encoding: SPEC.encoding }] };
    const spec = host.interactive(layered, { attr: "relativeHumidity", measure: host.measure("relativeHumidity", null, "en", []), lang: "en", width: 600 });
    expect(spec.vconcat).toBeUndefined();
    expect(spec.width).toBe("container");
  });
});

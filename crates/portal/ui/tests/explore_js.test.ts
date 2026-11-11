/**
 * The catalogue explorer's own script (vendor/viz/site/explore.js, brief 132): its pure helpers,
 * loaded as the deployment's node suite loads them (CommonJS `module.exports`), without its page.
 */
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";

const file = resolve(dirname(fileURLToPath(import.meta.url)), "../../../../vendor/viz/site/explore.js");
type Feature = { type: "Feature"; geometry: { type: string; coordinates: unknown } | null; properties?: object };
interface Explore {
  boundsOf(features: Feature[]): [[number, number], [number, number]] | null;
  inBox(features: Feature[], box: number[]): number;
  viewPlan(held: Feature[] | null, lastUrl: string | null, url: string, box: number[]): { local?: number; skip?: boolean; read?: string };
  countLine(t: object, c: { inView: number; total: number; none: number; tiled?: boolean }): string;
  stacks(points: Feature[]): Feature[];
  mapStart(maplibre: { Map: new (o: object) => FakeMap }, options: object, fail: (kind: string) => void): FakeMap | null;
  TEXT: Record<string, object>;
}
function load(): Explore {
  const module = { exports: {} as Explore };
  new Function("module", "self", readFileSync(file, "utf8"))(module, {});
  return module.exports;
}
const X = load();

// MapLibre's Map as far as mapStart touches it: events by name, `remove` counted.
class FakeMap {
  handlers: Record<string, Array<(e?: object) => void>> = {};
  removed = 0;
  on(name: string, f: (e?: object) => void) { (this.handlers[name] ||= []).push(f); }
  once(name: string, f: (e?: object) => void) { this.on(name, f); }
  fire(name: string, e?: object) { (this.handlers[name] || []).forEach((f) => f(e)); }
  remove() { this.removed++; }
}
const start = (Map: new (o: object) => FakeMap) => {
  const kinds: string[] = [];
  const map = X.mapStart({ Map }, { container: "map" }, (k) => kinds.push(k));
  return { map, kinds };
};
const point = (lon: number, lat: number): Feature => ({ type: "Feature", geometry: { type: "Point", coordinates: [lon, lat] } });
const none: Feature = { type: "Feature", geometry: null };

describe("explore.js map view", () => {
  it("fits every located row, not the first page", () => {
    // Bratislava, Zvolen, Poprad: the box spans all three.
    expect(X.boundsOf([point(17.1, 48.14), none, point(19.12, 48.57), point(20.3, 49.06)])).toEqual([[17.1, 48.14], [20.3, 49.06]]);
    const poly: Feature = { type: "Feature", geometry: { type: "Polygon", coordinates: [[[14.3, 50], [14.5, 50], [14.5, 50.1], [14.3, 50]]] } };
    expect(X.boundsOf([poly])).toEqual([[14.3, 50], [14.5, 50.1]]);
  });

  it("gives one point as a box of no size, and nothing for rows without a position", () => {
    expect(X.boundsOf([point(17.1, 48.14)])).toEqual([[17.1, 48.14], [17.1, 48.14]]);
    expect(X.boundsOf([none, none])).toBeNull();
    expect(X.boundsOf([])).toBeNull();
  });

  it("counts within the view as the broker's within does", () => {
    const box = [17, 48, 17.5, 48.5];
    const across: Feature = { type: "Feature", geometry: { type: "LineString", coordinates: [[17.1, 48.1], [18, 48.1]] } };
    expect(X.inBox([point(17.1, 48.14), point(19, 48.5), none, across], box)).toBe(1);
  });

  it("reads the same view and query once", () => {
    const url = "https://api.example/ngsi-ld/v1/entities?type=Event&georel=within";
    const box = [17, 48, 17.5, 48.5];
    expect(X.viewPlan(null, null, url, box)).toEqual({ read: url });
    expect(X.viewPlan(null, url, url, box)).toEqual({ skip: true });
    expect(X.viewPlan(null, url, url + "&q=a", box)).toEqual({ read: url + "&q=a" });
    // Every row of the query held: the view is counted, nothing is read.
    expect(X.viewPlan([point(17.1, 48.14), point(19, 48.5)], null, url, box)).toEqual({ local: 1 });
  });

  it("says how many rows have no position", () => {
    const t = { ...X.TEXT.en, locale: "en-GB" };
    expect(X.countLine(t, { inView: 29, total: 38, none: 9 })).toBe("On the map 29 of 38 · 9 without a location");
  });

  it("says how many rows share a dot", () => {
    // 29 events at two venues: each dot carries its venue's count.
    const at = (lon: number, lat: number, n: number) => Array.from({ length: n }, () => point(lon, lat));
    const out = X.stacks([...at(17.1, 48.14, 20), ...at(17.11, 48.15, 9), point(19, 48.5)]);
    expect(out.map((p) => (p.properties as { jcStack: number }).jcStack)).toEqual([...Array(20).fill(20), ...Array(9).fill(9), 1]);
  });
});

describe("explore.js map start (T-6737)", () => {
  it("says the map cannot start when its constructor throws, and throws nothing itself", () => {
    const Map = class { constructor() { throw new Error("WebGL2 is required to display this map https://x?token=s3cret"); } } as unknown as new (o: object) => FakeMap;
    const { map, kinds } = start(Map);
    expect(map).toBeNull();
    // Only the kind leaves mapStart: no URL, token or stack text from the error.
    expect(kinds).toEqual(["start"]);
  });

  it("ends a map whose style does not load, once", () => {
    const { map, kinds } = start(FakeMap);
    map!.fire("error", { error: new Error("HTTP 503") });
    map!.fire("error", { error: new Error("HTTP 503") });
    expect(kinds).toEqual(["start"]);
    expect(map!.removed).toBe(1);
  });

  it("keeps a drawn map when one source's tiles fail, and says so", () => {
    const { map, kinds } = start(FakeMap);
    map!.fire("load");
    map!.fire("error", { sourceId: "jc-tiles", error: new Error("HTTP 500") });
    map!.fire("error", { error: new Error("late") });
    expect(kinds).toEqual(["tiles"]);
    expect(map!.removed).toBe(0);
  });

  it("ends a map whose graphics context does not come back, and keeps one that does", () => {
    vi.useFakeTimers();
    try {
      const back = start(FakeMap);
      back.map!.fire("load");
      back.map!.fire("webglcontextlost");
      back.map!.fire("webglcontextrestored");
      vi.advanceTimersByTime(10_000);
      expect(back.kinds).toEqual([]);
      const gone = start(FakeMap);
      gone.map!.fire("load");
      gone.map!.fire("webglcontextlost");
      vi.advanceTimersByTime(10_000);
      expect(gone.kinds).toEqual(["lost"]);
      expect(gone.map!.removed).toBe(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("words every failed state in each locale the page speaks, with no address in it", () => {
    for (const lang of Object.keys(X.TEXT)) {
      const t = X.TEXT[lang] as { mapOff: Record<string, string>; mapRetry: string; mapTiles: string };
      for (const kind of ["webgl", "start", "lost"]) expect(t.mapOff[kind], `${lang}.${kind}`).toMatch(/\S.{20,}/);
      expect(t.mapRetry).toBeTruthy();
      expect(Object.values(t.mapOff).concat(t.mapTiles).join(" ")).not.toMatch(/https?:|\{/);
    }
  });
});

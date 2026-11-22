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
  matchLine(t: object, matching: number, all: number): string;
  capLine(t: object, read: number, total: number | null): string;
  badges(points: Feature[], zoom: number, merge: boolean): Badge[];
  mapStart(maplibre: { Map: new (o: object) => FakeMap }, options: object, fail: (kind: string) => void): FakeMap | null;
  TEXT: Record<string, object>;
}
interface Badge {
  geometry: { coordinates: [number, number] };
  properties: { jcId: number; jcColour: string; jcStack: number; jcR: number; jcCluster?: boolean; jcBox?: string };
}
function load(): Explore {
  const module = { exports: {} as Explore };
  new Function("module", "self", readFileSync(file, "utf8"))(module, {});
  return module.exports;
}
const X = load();
// The owner's dataset as the broker answered it on 2026-11-22: 234 Events as [position, starts today or later].
const PODUJATIA = JSON.parse(readFileSync(resolve(dirname(file), "../../../bins/jc/smoke/ui/tests/fixtures/bbsk_podujatia.json"), "utf8")) as Array<[[number, number] | null, boolean]>;

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

  it("says what each number counts, and the numbers add up to the broker's (T-7503)", () => {
    const located = (rows: typeof PODUJATIA) => rows.filter(([at]) => at).length;
    const all = PODUJATIA.length;
    const upcoming = PODUJATIA.filter(([, later]) => later);
    expect([all, located(PODUJATIA), upcoming.length, located(upcoming)]).toEqual([234, 231, 200, 197]);
    const en = { ...X.TEXT.en, locale: "en-GB" };
    expect(X.countLine(en, { inView: 49, total: all, none: all - located(PODUJATIA) })).toBe("49 in view · 231 with a location · 3 without");
    expect(X.countLine(en, { inView: 49, total: 200, none: 3 })).toBe("49 in view · 197 with a location · 3 without");
    expect(X.countLine({ ...X.TEXT.sk, locale: "sk-SK" }, { inView: 49, total: 200, none: 3 })).toBe("Vo výreze: 49 · s polohou: 197 · bez polohy: 3");
    expect(X.matchLine(en, upcoming.length, all)).toBe("200 of 234 records match the filter.");
    expect(X.matchLine({ ...X.TEXT.sk, locale: "sk-SK" }, upcoming.length, all)).toBe("Filtru vyhovuje 200 z 234 záznamov.");
    // A read cap below the total is named as one, never shown as the total.
    expect(X.capLine(en, 200, all)).toBe("The map opens on the first 200 of 234 records; zoom in for the rest.");
  });

  it("says how many rows share a dot", () => {
    // 29 events at two venues: each dot carries its venue's count.
    const at = (lon: number, lat: number, n: number) => Array.from({ length: n }, () => point(lon, lat));
    const out = X.badges([...at(17.1, 48.14, 20), ...at(17.11, 48.15, 9), point(19, 48.5)], 15, true);
    expect(out.map((b) => [b.properties.jcStack, b.properties.jcCluster ?? false])).toEqual([[20, false], [9, false], [1, false]]);
  });

  const located: Feature[] = PODUJATIA.flatMap(([at], i) => (at ? [{ ...point(...at), properties: { jcId: i, jcColour: "#1f5fae" } }] : []));
  const onScreen = (b: Badge, zoom: number) => {
    const [lon, lat] = b.geometry.coordinates;
    const size = 512 * 2 ** zoom;
    const sin = Math.sin((lat * Math.PI) / 180);
    return { x: ((lon + 180) / 360) * size, y: (0.5 - Math.log((1 + sin) / (1 - sin)) / (4 * Math.PI)) * size, r: b.properties.jcR };
  };

  it("draws every point once and no two badge boxes intersect, at any zoom (T-7503)", () => {
    for (let zoom = 0; zoom <= 22; zoom += 0.25) {
      const out = X.badges(located, zoom, true);
      expect(out.reduce((n, b) => n + b.properties.jcStack, 0)).toBe(231);
      const boxes = out.map((b) => onScreen(b, zoom));
      const hits = boxes.flatMap((a, i) => boxes.slice(i + 1).filter((b) => Math.abs(a.x - b.x) < a.r + b.r && Math.abs(a.y - b.y) < a.r + b.r));
      expect(hits, `zoom ${zoom}`).toEqual([]);
    }
  });

  it("opens a merged badge on its own points, and keeps two positions a metre apart as one dot", () => {
    const two = X.badges([point(17.1, 48.14), point(17.11, 48.15)], 5, true);
    expect(two).toHaveLength(1);
    expect(two[0]!.properties.jcCluster).toBe(true);
    expect(JSON.parse(two[0]!.properties.jcBox!)).toEqual([[17.1, 48.14], [17.11, 48.15]]);
    // Nothing to zoom into: the dot opens a row's card.
    const near = X.badges([point(17.1, 48.14), point(17.100001, 48.140001)], 22, true);
    expect(near.map((b) => [b.properties.jcStack, b.properties.jcCluster ?? false])).toEqual([[2, false]]);
  });

  it("keeps each position apart when the reader turned grouping off, and draws nothing for no points", () => {
    const places = new Set(PODUJATIA.flatMap(([at]) => (at ? [at.join(",")] : [])));
    expect(X.badges(located, 3, false)).toHaveLength(places.size);
    expect(X.badges([], 3, true)).toEqual([]);
  });

  it("gives a badge its members' colour, a neutral one when they differ", () => {
    const of = (colour: string, lon: number): Feature => ({ ...point(lon, 48.14), properties: { jcColour: colour } });
    expect(X.badges([of("#15803d", 17.1), of("#15803d", 17.11)], 5, true)[0]!.properties.jcColour).toBe("#15803d");
    expect(X.badges([of("#15803d", 17.1), of("#c2410c", 17.11)], 5, true)[0]!.properties.jcColour).toBe("#334155");
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

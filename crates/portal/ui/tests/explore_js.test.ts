/**
 * The catalogue explorer's own script (vendor/viz/site/explore.js, brief 132): its pure helpers,
 * loaded as the deployment's node suite loads them (CommonJS `module.exports`), without its page.
 */
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const file = resolve(dirname(fileURLToPath(import.meta.url)), "../../../../vendor/viz/site/explore.js");
type Feature = { type: "Feature"; geometry: { type: string; coordinates: unknown } | null; properties?: object };
interface Explore {
  boundsOf(features: Feature[]): [[number, number], [number, number]] | null;
  inBox(features: Feature[], box: number[]): number;
  viewPlan(held: Feature[] | null, lastUrl: string | null, url: string, box: number[]): { local?: number; skip?: boolean; read?: string };
  countLine(t: object, c: { inView: number; total: number; none: number; tiled?: boolean }): string;
  TEXT: Record<string, object>;
}
function load(): Explore {
  const module = { exports: {} as Explore };
  new Function("module", "self", readFileSync(file, "utf8"))(module, {});
  return module.exports;
}
const X = load();
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
});

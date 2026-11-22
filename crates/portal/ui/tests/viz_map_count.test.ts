/**
 * What the renderer's map says on itself (T-7504): "N shown of TOTAL", "K without a location",
 * from counts the host's and the map page's refusal list both check (vendor/viz/site/refuse.js).
 */
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const site = resolve(dirname(fileURLToPath(import.meta.url)), "../../../../vendor/viz/site");
function load<T>(name: string): T {
  const module = { exports: {} as T };
  new Function("module", "self", readFileSync(resolve(site, name), "utf8"))(module, {});
  return module.exports;
}
type Refusal = { pointer: string; rule: string };
const R = load<{ map(spec: object, domain: string): Refusal[] }>("refuse.js");
const M = load<{ countText(spec: object, lang: string): string }>("map.js");
const H = load<{ hosts(hostname: string, meta: string | null): { domain: string } }>("host.js");

const spec = (count?: object, features = 2) => ({
  $schema: "urn:joinedcontext:map:v1",
  label: "name",
  popup: [],
  data: {
    type: "FeatureCollection",
    features: Array.from({ length: features }, (_, i) => ({
      type: "Feature",
      geometry: { type: "Point", coordinates: [19 + i / 100, 48.7] },
      properties: { name: `Most ${i}` },
    })),
  },
  ...(count ? { count } : {}),
});

describe("a map's count", () => {
  it("says how many it shows of all and how many have no location", () => {
    expect(M.countText(spec({ total: 151 }, 50), "en")).toBe("50 shown of 151");
    expect(M.countText(spec({ total: 1651, unlocated: 11 }, 1), "en")).toBe("1 shown of 1,651 · 11 without a location");
    expect(M.countText(spec({ total: 151, unlocated: 3 }, 50), "sk")).toBe("Zobrazené: 50, spolu 151 · 3 bez polohy");
    expect(M.countText(spec(), "en")).toBe("");
  });

  it("draws only whole counts that can be true", () => {
    expect(R.map(spec({ total: 151, unlocated: 3 }), "example.org")).toEqual([]);
    expect(R.map(spec(), "example.org")).toEqual([]);
    for (const count of [{ total: 1 }, { total: 2.5 }, { total: "151" }, { total: 5, unlocated: 4 }, { total: 5, note: "<b>" }, [5]]) {
      expect(R.map(spec(count), "example.org").map((e) => e.pointer)).toContain("/count");
    }
  });

  it("reads the domain of a page on embed. as on its old name viz.", () => {
    expect(H.hosts("embed.smartcity.example.org", null).domain).toBe("smartcity.example.org");
    expect(H.hosts("viz.smartcity.example.org", null).domain).toBe("smartcity.example.org");
  });
});

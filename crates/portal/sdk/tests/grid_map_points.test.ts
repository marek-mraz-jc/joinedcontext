/**
 * The map's lean read of every entity as a point (T-6345): id, name and location as plain values,
 * a thousand a request, until the answer ends.
 */
import { describe, expect, it } from "vitest";

import { featuresOfPoints, MAX_POINTS, pointGeometry, POINTS_PAGE, readPoints } from "../src/grid/mapPoints";
import { endpointSource, fixtureSource, pointOf } from "../src/grid/source";
import type { Transport } from "../src/sdk/transport";

/** A broker of `count` stations answering keyValues pages, recording every path it was asked. */
function broker(count: number, counted = true): { transport: Transport; paths: string[] } {
  const paths: string[] = [];
  const transport: Transport = async (request) => {
    paths.push(request.path);
    const url = new URL(request.path, "https://host.invalid");
    const offset = Number(url.searchParams.get("offset") ?? 0);
    const limit = Number(url.searchParams.get("limit"));
    const body = Array.from({ length: Math.max(0, Math.min(limit, count - offset)) }, (_, i) => ({
      id: `urn:ngsi-ld:Station:${offset + i}`,
      type: "Station",
      name: `Station ${offset + i}`,
      location: { type: "Point", coordinates: [17 + (offset + i) / 1e4, 48] },
    }));
    const headers: Record<string, string> = counted && url.searchParams.get("count") === "true" ? { "ngsild-results-count": String(count) } : {};
    return { status: 200, body, headers };
  };
  return { transport, paths };
}

describe("the map's point read", () => {
  it("asks for id, name and the geometry alone, as keyValues, a thousand at a time", async () => {
    const { transport, paths } = broker(10);
    const source = endpointSource("air-sk-stations-aaaaaaaaaaaaaaaa", transport, "sk");
    const done = await readPoints(source, { type: "Station", q: "status==\"open\"" }, "location", () => {});
    expect(done.points).toHaveLength(10);
    expect(done.done).toBe(true);
    const url = new URL(paths[0], "https://host.invalid");
    expect(url.searchParams.get("attrs")).toBe("name,location");
    expect(url.searchParams.get("options")).toBe("keyValues");
    expect(url.searchParams.get("lang")).toBe("sk");
    expect(url.searchParams.get("limit")).toBe(String(POINTS_PAGE));
    expect(url.searchParams.get("q")).toBe('status=="open"');
    // A count walks the whole match and can outlast the gateway's 10 s cut: the map never asks one (T-6541).
    expect(url.searchParams.get("count")).toBeNull();
  });

  it("pages until the answer ends, reporting each page, and asks no count on any", async () => {
    const { transport, paths } = broker(2_310);
    const seen: number[] = [];
    const done = await readPoints(endpointSource("s", transport), { type: "Station" }, "location", (p) => seen.push(p.points.length));
    expect(paths).toHaveLength(3);
    expect(seen).toEqual([1000, 2000, 2310]);
    expect(done.total).toBeUndefined();
    expect(paths.every((path) => new URL(path, "https://host.invalid").searchParams.get("count") === null)).toBe(true);
    expect(new URL(paths[2], "https://host.invalid").searchParams.get("offset")).toBe("2000");
  });

  it("stops on a short page when the answer carries no count", async () => {
    const { transport, paths } = broker(1_500, false);
    const done = await readPoints(endpointSource("s", transport), { type: "Station" }, "location", () => {});
    expect(paths).toHaveLength(2);
    expect(done).toMatchObject({ done: true, capped: false, total: undefined });
  });

  it("stops at its cap and says so", async () => {
    const { transport, paths } = broker(50_000, false);
    const done = await readPoints(endpointSource("s", transport), { type: "Station" }, "location", () => {}, undefined, 2_000);
    expect(paths).toHaveLength(2);
    expect(done).toMatchObject({ done: true, capped: true });
    expect(MAX_POINTS).toBe(10_000);
  });

  it("reads nothing more once the question changed", async () => {
    const { transport, paths } = broker(5_000);
    const abort = new AbortController();
    const done = await readPoints(endpointSource("s", transport), { type: "Station" }, "location", () => abort.abort(), abort.signal);
    expect(paths).toHaveLength(1);
    expect(done.done).toBe(false);
  });

  it("refuses as the grid refuses: a 401 or 403 is an error, never an empty map", async () => {
    const transport: Transport = async () => ({ status: 403, body: { title: "Forbidden", detail: "no grant" } });
    await expect(readPoints(endpointSource("s", transport), { type: "Station" }, "location", () => {})).rejects.toMatchObject({ status: 403 });
  });

  it("falls back to the row read for a source without a lean one", async () => {
    const source = fixtureSource([
      { id: "urn:a", type: "Station", name: { type: "Property", value: "A" }, location: { type: "GeoProperty", value: { type: "Point", coordinates: [1, 2] } } },
      { id: "urn:b", type: "Station", name: { type: "Property", value: "B" } },
    ]);
    const done = await readPoints(source, { type: "Station" }, "location", () => {});
    expect(done.points).toEqual([{ id: "urn:a", name: "A", geometry: { type: "Point", coordinates: [1, 2] } }]);
  });

  it("draws a polygon at the mean of its positions and keeps the name for the panel", () => {
    expect(pointGeometry({ type: "Polygon", coordinates: [[[0, 0], [2, 0], [2, 2], [0, 2]]] } as never)).toEqual({ type: "Point", coordinates: [1, 1] });
    const [feature] = featuresOfPoints([{ id: "urn:x", name: "X", geometry: { type: "Point", coordinates: [1, 2] } as never }]);
    expect(feature).toMatchObject({ id: "urn:x", properties: { name: "X", point: true } });
    expect(pointOf({ id: "urn:y", name: "" }, "location")).toBeNull();
  });
});

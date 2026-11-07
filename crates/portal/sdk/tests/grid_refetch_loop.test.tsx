import { describe, expect, it } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource } from "../src/grid/source";
import type { EntitySource, GridPage, GridQuery } from "../src/grid/source";
import { useEntityGrid } from "../src/grid/useEntityGrid";
import { ringOfBounds } from "../src/grid/geoarea";

/**
 * The answer must not ask the question again (T-2276, UI-72).
 *
 * `askedQuery` is derived from the columns, whose kinds are read off the rows, so it is a new object
 * after every answer. While the fetch depended on that object, one drawn area started a query that
 * never stopped — 20 000 of them in a second and a half in a real browser, with the main thread
 * blocked for as long as the page lived, because a trusted click makes React flush the update
 * synchronously. A host passing `query={{ q: "…" }}` inline had the same effect.
 */
const stations: Record<string, unknown>[] = [
  {
    id: "urn:ngsi-ld:Station:hel:helsinki:001",
    type: "Station",
    name: { type: "Property", value: "Kamppi" },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.931, 60.169] } },
  },
  {
    id: "urn:ngsi-ld:Station:hel:helsinki:002",
    type: "Station",
    name: { type: "Property", value: "Kallio" },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.951, 60.186] } },
  },
];

const config = parseGridConfig({
  source: { kind: "fixture", name: "test" },
  type: "Station",
  columns: [{ attr: "name" }, { attr: "location" }],
  pageSize: 10,
}).config!;

function countingSource(): { source: EntitySource; queries: GridQuery[] } {
  const base = fixtureSource(stations);
  const queries: GridQuery[] = [];
  return {
    queries,
    source: {
      ...base,
      query: (query: GridQuery, page: { offset: number; limit: number }): Promise<GridPage> => {
        queries.push(query);
        // A fresh array every time, the way a real endpoint answers: nothing here may depend on the
        // identity of what came back.
        return base.query(query, page).then((answer) => ({ ...answer, rows: [...answer.rows] }));
      },
    },
  };
}

const settle = async (): Promise<void> => {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 60));
  });
};

describe("the grid asks once per question", () => {
  it("stops querying after a drawn area", async () => {
    const { source, queries } = countingSource();
    const { result } = renderHook(() => useEntityGrid({ config, source }));
    await settle();
    const atRest = queries.length;

    await act(async () => {
      result.current.setArea({
        geoproperty: "location",
        geometry: { type: "Polygon", coordinates: [ringOfBounds(24.9, 60.15, 25.0, 60.2)] },
      });
    });
    await settle();
    const afterArea = queries.length - atRest;
    await settle();
    const later = queries.length - atRest - afterArea;

    expect(atRest).toBeLessThanOrEqual(2);
    expect(afterArea).toBeLessThanOrEqual(2);
    // Nothing more once the page has its answer: this is the number that ran to 20 000.
    expect(later).toBe(0);
  });

  it("stops querying when the host passes the query as a literal", async () => {
    const { source, queries } = countingSource();
    const { result, rerender } = renderHook(() =>
      useEntityGrid({ config, source, query: { q: "name==\"Kamppi\"" } }),
    );
    await settle();
    const first = queries.length;
    rerender();
    await settle();
    expect(first).toBeLessThanOrEqual(2);
    expect(queries.length - first).toBe(0);
    expect(result.current.rows.length).toBeGreaterThan(0);
  });
});

import { describe, expect, it } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource } from "../src/grid/source";
import { useEntityGrid } from "../src/grid/useEntityGrid";

/** An edit outlives its page: it is written as the cell the person edited, not as page 2 has it (T-6230). */
const stations: Record<string, unknown>[] = ["001", "002"].map((n, i) => ({
  id: `urn:ngsi-ld:Station:hel:helsinki:${n}`,
  type: "Station",
  capacity: { type: "Property", value: 10 + i, unitCode: "C62" },
  location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.93 + i / 100, 60.17] } },
}));

const config = parseGridConfig({
  source: { kind: "fixture", name: "test" },
  type: "Station",
  columns: [{ attr: "capacity" }, { attr: "location" }],
  pageSize: 1,
}).config!;

const settle = async (): Promise<void> => {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 60));
  });
};

describe("a pending edit on another page", () => {
  it("keeps the kind, unit and value before of the row it was made on", async () => {
    const source = fixtureSource(stations);
    const { result } = renderHook(() => useEntityGrid({ config, source }));
    await settle();
    const first = result.current.rows[0].id;
    act(() => {
      result.current.setEdits([
        { id: first, attr: "location", value: { type: "Point", coordinates: [25, 60] } },
        { id: first, attr: "capacity", value: 12 },
      ]);
    });
    act(() => result.current.setOffset(1));
    await settle();
    expect(result.current.rows.map((row) => row.id)).not.toContain(first);
    const [change] = result.current.pendingChanges;
    expect(change.id).toBe(first);
    expect(change.changes).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ attribute: "location", kind: "geo" }),
        expect.objectContaining({ attribute: "capacity", kind: "property", unitCode: "C62", before: 10 }),
      ]),
    );
  });
});

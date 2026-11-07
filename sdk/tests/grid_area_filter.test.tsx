/**
 * The location column's filter cell opens the grid's map to draw an area on, and clears it
 * (T-6345). The map is stubbed as in grid_map.test.tsx: MapLibre needs WebGL, jsdom has none.
 */
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const editorCalls: Record<string, unknown>[] = [];
const viewCalls: Record<string, unknown>[] = [];

vi.mock("../src/geo/GeoView", async (importOriginal) => ({
  ...((await importOriginal()) as Record<string, unknown>),
  GeoView: (props: Record<string, unknown>) => {
    viewCalls.push(props);
    return <div data-testid="geo-view" />;
  },
}));

vi.mock("../src/geo/GeoEditor", async (importOriginal) => ({
  ...((await importOriginal()) as Record<string, unknown>),
  GeoEditor: (props: Record<string, unknown>) => {
    editorCalls.push(props);
    return <div data-testid="geo-editor" />;
  },
}));

const { EntityGrid } = await import("../src/grid/EntityGrid");
const { parseGridConfig } = await import("../src/grid/config");
const { fixtureSource } = await import("../src/grid/source");

const stations = [
  { id: "urn:ngsi-ld:Station:in", type: "Station", name: { type: "Property", value: "Inside" }, location: { type: "GeoProperty", value: { type: "Point", coordinates: [19.15, 48.74] } } },
  { id: "urn:ngsi-ld:Station:out", type: "Station", name: { type: "Property", value: "Outside" }, location: { type: "GeoProperty", value: { type: "Point", coordinates: [17.1, 48.1] } } },
];

const config = parseGridConfig({
  source: { kind: "fixture", name: "test" },
  type: "Station",
  columns: [
    { attr: "name", label: "Name", pinned: true },
    { attr: "location", label: "Location" },
  ],
  pageSize: 10,
}).config!;

afterEach(() => {
  cleanup();
  editorCalls.length = 0;
  viewCalls.length = 0;
});

describe("the location filter cell", () => {
  it("opens the map on the drawing tools, sets the area from the drawn shape and clears it", async () => {
    const source = fixtureSource(stations);
    const asked = vi.spyOn(source, "query");
    render(<EntityGrid config={config} source={source} />);
    await screen.findByText("Inside");
    const filterRow = document.querySelector(".jc-grid-filter-row") as HTMLElement;
    fireEvent.click(within(filterRow).getByRole("button", { name: "Pick area on map" }));
    expect(await screen.findByTestId("geo-editor")).toBeTruthy();

    // The map shows every entity of the type, read lean beside the page.
    await waitFor(() => expect((editorCalls.at(-1)?.snapTo as { id: string }[]).map((f) => f.id)).toContain("urn:ngsi-ld:Station:out"));

    const drawn = { type: "Polygon", coordinates: [[[19, 48.6], [19.3, 48.6], [19.3, 48.9], [19, 48.9], [19, 48.6]]] };
    (editorCalls.at(-1)!.onChange as (g: unknown) => void)(drawn);
    await waitFor(() => expect(asked.mock.calls.some(([q]) => q.area?.georel === "within" && q.area.geoproperty === "location")).toBe(true));
    expect(within(filterRow).getByRole("button", { name: "Clear area" })).toHaveAttribute("aria-pressed", "true");

    fireEvent.click(within(filterRow).getByRole("button", { name: "Clear area" }));
    await waitFor(() => expect(within(filterRow).getByRole("button", { name: "Pick area on map" })).toBeTruthy());
    expect(asked.mock.calls.at(-1)![0].area).toBeUndefined();
  });
});

describe("the query line", () => {
  it("names the drawn area's geo query beside the filters (T-6345)", async () => {
    render(<EntityGrid config={config} source={fixtureSource(stations)} />);
    await screen.findByText("Inside");
    fireEvent.click(within(document.querySelector(".jc-grid-filter-row") as HTMLElement).getByRole("button", { name: "Pick area on map" }));
    await screen.findByTestId("geo-editor");
    (editorCalls.at(-1)!.onChange as (g: unknown) => void)({ type: "Polygon", coordinates: [[[19, 48.6], [19.3, 48.6], [19.3, 48.9], [19, 48.6]]] });
    await waitFor(() => expect(document.querySelector(".jc-grid-query-value")?.textContent).toContain("georel=within&geometry=Polygon"));
    expect(document.querySelector(".jc-grid-query-value")?.textContent).toContain("geoproperty=location");
    // A phone wraps it after commas and ampersands, never inside a coordinate.
    const value = document.querySelector(".jc-grid-query-value") as HTMLElement;
    for (const mark of value.querySelectorAll("wbr")) expect(mark.previousSibling?.textContent).toMatch(/[,&]$/);
    expect(value.querySelectorAll("wbr").length).toBeGreaterThan(4);
  });
});

/**
 * The dashboard over a stub endpoint: the rows arrive, the tiles count them, a filter narrows
 * table and tiles together, a row click fills the detail, and a failing endpoint is said out
 * loud. MapLibre needs WebGL, which jsdom has not, so the library is a double that records what
 * it was handed, as the reference app's test does.
 */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import example from "../spec.example.json";

const setData = vi.fn();
let load: (() => void) | undefined;
vi.mock("maplibre-gl", () => {
  class Map {
    constructor(public options: unknown) {}
    on(event: string, handler: () => void) {
      if (event === "load") load = handler;
    }
    addSource = vi.fn();
    addLayer = vi.fn();
    getSource = () => ({ setData });
    fitBounds = vi.fn();
    remove = vi.fn();
  }
  return { Map };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

import { App } from "../src/App";
import { parseSpec } from "../src/spec";
import { featureCollection, colorOf } from "../src/views/MapView";
import { series } from "../src/views/Chart";
import { compare } from "../src/views/Table";

const parsed = parseSpec(example);
const spec = parsed.spec!;

const STATIONS = [
  { id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:001", type: "BikeHireDockingStation", name: "Kaivopuisto", availableBikeNumber: 12, freeSlotNumber: 18, totalSlotNumber: 30, status: "working", location: { type: "Point", coordinates: [24.95, 60.155] } },
  { id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:002", type: "BikeHireDockingStation", name: "Laivasillankatu", availableBikeNumber: 3, freeSlotNumber: 9, totalSlotNumber: 12, status: "working", location: { type: "Point", coordinates: [24.956, 60.16] } },
  { id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:003", type: "BikeHireDockingStation", name: "Kapteeninpuistikko", availableBikeNumber: 0, freeSlotNumber: 16, totalSlotNumber: 16, status: "closed", location: { type: "Point", coordinates: [24.94, 60.158] } },
];

const calls: string[] = [];
beforeEach(() => {
  calls.length = 0;
  vi.stubGlobal("fetch", vi.fn(async (url: string) => {
    calls.push(url);
    // A real Response: a write goes through the SDK's origin transport, which reads the headers (T-6230).
    return new Response(JSON.stringify(STATIONS), { status: 200, headers: { "content-type": "application/json" } });
  }));
});
afterEach(() => {
  vi.unstubAllGlobals();
  setData.mockClear();
});

describe("App", () => {
  it("reads the endpoint, counts, filters, selects, and hands the map its points", async () => {
    render(<App slug="demo" spec={spec} />);
    // The header's own counter: the grid view of the example also renders a `status`, so the role
    // alone is ambiguous since T-1440.
    const counter = () => document.querySelector(".status") as HTMLElement;
    await waitFor(() => expect(counter()).toHaveTextContent("3 entities"));
    // The person's decisions are asked first (T-5530), so the read never names an attribute they may not see.
    expect(calls.find((call) => call.includes("/ngsi-ld/v1/entities"))).toContain("/api/endpoint/demo/ngsi-ld/v1/entities?type=BikeHireDockingStation");
    // The tiles: count and sum over every row.
    expect(screen.getByText("Stations").previousSibling).toHaveTextContent("3");
    expect(screen.getByText("Bikes available").previousSibling).toHaveTextContent("15");
    // The map got the three points once the double said it loaded.
    load?.();
    await waitFor(() => expect(setData).toHaveBeenCalled());
    const collection = setData.mock.calls.at(-1)?.[0] as { features: unknown[] };
    expect(collection.features).toHaveLength(3);
    // A select narrows tiles and table together.
    fireEvent.change(screen.getByLabelText("Status"), { target: { value: "closed" } });
    expect(counter()).toHaveTextContent("1 of 3 entities");
    expect(screen.getByText("Stations").previousSibling).toHaveTextContent("1");
    // The `table` view's own rows: the grid view of the example is a `role="grid"` of its own.
    expect(within(screen.getByRole("table")).getAllByRole("row")).toHaveLength(2);
    // A row click fills the detail.
    fireEvent.click(screen.getByText("Kapteeninpuistikko"));
    expect(screen.getByText(":003", { exact: false })).toBeInTheDocument();
  });

  it("uses the rows the document carries and fetches nothing", async () => {
    render(<App slug="demo" spec={spec} inline={{ stations: STATIONS.slice(0, 2), ghost: [1, null] }} />);
    await waitFor(() => expect(screen.getByText("2 entities")).toBeInTheDocument());
    expect(calls).toEqual([]);
    expect(screen.getByText("Laivasillankatu")).toBeInTheDocument();
  });

  it("opens the form on a pick, and a save changes the rows on screen", async () => {
    // jsdom draws no dialog: the modal calls are recorded, the content renders regardless.
    const showModal = vi.fn();
    HTMLDialogElement.prototype.showModal = showModal;
    HTMLDialogElement.prototype.close = vi.fn();
    const withForm = { ...spec, views: [...spec.views, { kind: "form" as const, title: "Station", fields: ["name", "availableBikeNumber"] }] };
    render(<App slug="demo" spec={withForm} inline={{ stations: STATIONS }} />);
    await waitFor(() => expect(screen.getByText("3 entities")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Laivasillankatu"));
    expect(showModal).toHaveBeenCalled();
    const bikes = screen.getByLabelText("availableBikeNumber") as HTMLInputElement;
    expect(bikes.value).toBe("3");
    fireEvent.change(bikes, { target: { value: "7" } });
    fireEvent.click(screen.getByText("Save"));
    // 12 + 7 + 0 once the endpoint accepted the write.
    await waitFor(() => expect(screen.getByText("Bikes available").previousSibling).toHaveTextContent("19"));
    expect(calls.at(-1)).toContain("/entities/urn%3Angsi-ld%3ABikeHireDockingStation%3Ahel.fi%3Ahelsinki%3A002/attrs");
  });

  it("shows one page at a time when views name pages", async () => {
    const paged = { ...spec, views: spec.views.map((v, i) => ({ ...v, page: i < 2 ? "Overview" : "Data" })) };
    render(<App slug="demo" spec={paged} inline={{ stations: STATIONS }} />);
    await waitFor(() => expect(screen.getByText("3 entities")).toBeInTheDocument());
    expect(screen.getByRole("navigation", { name: "Pages" })).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Data" }));
    expect(screen.getByRole("table")).toBeInTheDocument();
  });

  it("says when the endpoint refuses instead of showing an empty dashboard", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => ({ ok: false, status: 403, json: async () => ({}) })));
    render(<App slug="demo" spec={spec} />);
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("BikeHireDockingStation: the endpoint answered 403"));
  });
});

describe("helpers", () => {
  it("sorts numbers numerically, text naturally, nulls last", () => {
    const rows = STATIONS.map((s) => ({ ...s, location: null })) as never[];
    expect([...rows].sort((a, b) => compare(a, b, "availableBikeNumber", "asc")).map((r: { name: string }) => r.name)).toEqual(["Kapteeninpuistikko", "Laivasillankatu", "Kaivopuisto"]);
    expect(compare({ id: "a", type: "t", n: null }, { id: "b", type: "t", n: 1 }, "n", "asc")).toBe(1);
  });
  it("builds a series per distinct x, largest first, and colours a ramp", () => {
    const rows = STATIONS.map((s) => ({ ...s, location: null })) as never[];
    const points = series(rows, "status", "availableBikeNumber", "sum", 5);
    expect(points).toEqual([{ x: "working", y: 15 }, { x: "closed", y: 0 }]);
    expect(series(rows, "status", "availableBikeNumber", "count", 1)).toEqual([{ x: "working", y: 2 }]);
    expect(colorOf(0, [0, 10], "#000")).toBe("hsl(200, 70%, 45%)");
    expect(colorOf(10, [0, 10], "#000")).toBe("hsl(30, 70%, 45%)");
    expect(colorOf(null, null, "#abc")).toBe("#abc");
    const fc = featureCollection(STATIONS.map((s) => ({ ...s })) as never[], "location", "name", "availableBikeNumber", "#000");
    expect(fc.features.map((f) => f.properties.label)).toEqual(["Kaivopuisto", "Laivasillankatu", "Kapteeninpuistikko"]);
    expect(featureCollection([{ id: "x", type: "t" }], "location", undefined, undefined, "#000").features).toEqual([]);
  });
});

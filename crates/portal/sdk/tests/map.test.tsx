import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Row } from "../src/ngsi";
import mapViewSource from "../src/views/MapView.tsx?raw";

interface MapOptions {
  container: HTMLElement;
  style: unknown;
  center: [number, number];
  zoom: number;
}

const mapInstances: Array<{ options: MapOptions; setData: ReturnType<typeof vi.fn>; fitBounds: ReturnType<typeof vi.fn> }> = [];
let loadHandler: (() => void) | undefined;
let clickHandler: ((event: { features?: Array<{ properties?: { id?: string } }> }) => void) | undefined;

vi.mock("maplibre-gl", () => {
  class Map {
    setData = vi.fn();
    fitBounds = vi.fn();
    constructor(public options: MapOptions) {
      mapInstances.push({ options, setData: this.setData, fitBounds: this.fitBounds });
    }
    on(event: string, ...args: unknown[]) {
      if (event === "load" && typeof args[0] === "function") {
        loadHandler = args[0] as () => void;
      } else if (event === "click" && typeof args[1] === "function") {
        clickHandler = args[1] as (e: { features?: Array<{ properties?: { id?: string } }> }) => void;
      }
    }
    addSource = vi.fn();
    addLayer = vi.fn();
    getSource = () => ({ setData: this.setData });
    remove = vi.fn();
  }
  return { Map, setWorkerUrl: vi.fn() };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

import { MapView, NO_BASEMAP, styleFor } from "../src/views/MapView";

const STATIONS: Row[] = [
  {
    id: "urn:ngsi-ld:Station:001",
    type: "Station",
    name: "Kaivopuisto",
    availableBikeNumber: 12,
    location: { type: "Point", coordinates: [24.95, 60.155] },
  },
  {
    id: "urn:ngsi-ld:Station:002",
    type: "Station",
    name: "Laivasillankatu",
    availableBikeNumber: 3,
    location: { type: "Point", coordinates: [24.956, 60.16] },
  },
  {
    id: "urn:ngsi-ld:Station:003",
    type: "Station",
    name: "No Location",
    availableBikeNumber: 0,
    location: null,
  },
];

describe("styleFor helper", () => {
  it("returns the configured basemap style URL when a non-empty string is provided", () => {
    const url = "https://portal.example.com/api/v1/projects/helsinki/basemap/default/style.json";
    expect(styleFor(url)).toBe(url);
  });

  it("returns a background-only style object when basemap is undefined, null, or empty string", () => {
    const fallback = styleFor(undefined);
    expect(typeof fallback).toBe("object");
    expect(fallback).toMatchObject({
      version: 8,
      sources: {},
      layers: [
        {
          id: "background",
          type: "background",
        },
      ],
    });

    expect(styleFor("")).toEqual(fallback);
  });

  it("exports NO_BASEMAP matching the contract", () => {
    expect(NO_BASEMAP).toBe("No basemap is configured");
  });
});

describe("MapView component", () => {
  beforeEach(() => {
    mapInstances.length = 0;
    loadHandler = undefined;
    clickHandler = undefined;
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("initializes MapLibre with the style URL when basemap prop is provided and renders no fallback warning", async () => {
    const styleUrl = "https://portal.example.com/api/v1/projects/helsinki/basemap/default/style.json";
    render(
      <MapView
        rows={STATIONS}
        location="location"
        label="name"
        color="availableBikeNumber"
        accent="#0f766e"
        selected={null}
        onSelect={() => {}}
        basemap={styleUrl}
      />,
    );

    await waitFor(() => expect(mapInstances.length).toBeGreaterThan(0));
    const instance = mapInstances[0];
    expect(instance.options.style).toBe(styleUrl);

    // Baseline element check
    expect(screen.getByTestId("map")).toBeInTheDocument();
    expect(screen.queryByText(NO_BASEMAP)).not.toBeInTheDocument();

    // Trigger map loaded event
    loadHandler?.();
    await waitFor(() => expect(instance.setData).toHaveBeenCalled());

    // 2 valid geo points out of 3 rows
    expect(screen.getByText("2 on the map")).toBeInTheDocument();
  });

  it("initializes MapLibre with fallback background style and displays visible warning when basemap is missing", async () => {
    render(
      <MapView
        rows={STATIONS}
        location="location"
        label="name"
        color="availableBikeNumber"
        accent="#0f766e"
        selected={null}
        onSelect={() => {}}
      />,
    );

    await waitFor(() => expect(mapInstances.length).toBeGreaterThan(0));
    const instance = mapInstances[0];
    expect(instance.options.style).toEqual(styleFor(undefined));

    // Visible status line stating that no basemap is configured
    expect(screen.getByText(NO_BASEMAP)).toBeInTheDocument();

    loadHandler?.();
    await waitFor(() => expect(instance.setData).toHaveBeenCalled());
    expect(screen.getByText("2 on the map")).toBeInTheDocument();
  });

  it("handles point selection and invokes onSelect callback", async () => {
    const onSelect = vi.fn();
    render(
      <MapView
        rows={STATIONS}
        location="location"
        label="name"
        accent="#0f766e"
        selected="urn:ngsi-ld:Station:001"
        onSelect={onSelect}
        basemap="https://portal.example.com/style.json"
      />,
    );

    await waitFor(() => expect(mapInstances.length).toBeGreaterThan(0));
    loadHandler?.();

    // Selected point label is reflected in map count string
    expect(screen.getByText("2 on the map · Kaivopuisto")).toBeInTheDocument();

    // Simulating dot click event
    clickHandler?.({ features: [{ properties: { id: "urn:ngsi-ld:Station:002" } }] });
    expect(onSelect).toHaveBeenCalledWith("urn:ngsi-ld:Station:002");
  });
});

describe("MapView source code host isolation (AP-49, AP-67)", () => {
  it("contains no hardcoded third-party tile hosts in MapView.tsx", () => {
    const lower = mapViewSource.toLowerCase();

    expect(lower).not.toContain("openfreemap");
    expect(lower).not.toContain("tile.openstreetmap.org");
    expect(lower).not.toContain("tiles.openfreemap.org");
  });
});

/**
 * The geomap panel of dev's Grafana ("Air quality map", one markers layer) drawn by the Portal's
 * MapLibre map over the frame daiya answers for its target: the 100 helsinki AirQualityObserved
 * of the dev export, written by daiya's own frame code (crates/daiya/query/tests/ngsild.rs
 * `the_helsinki_geomap_target_answers_the_frame_the_portal_map_reads`), never typed here.
 */
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "../src/i18n";
import { PanelView } from "../src/components/panels/PanelView";
import type { PanelDefinition } from "../src/components/panels/PanelView";
import { markersOf } from "../src/components/panels/GeomapPanel";
import type { Frame } from "../src/components/panels/frames";
import { colorOf } from "../src/components/panels/palette";
import { blankStyle } from "../src/components/dashboards/MapLibreView";
import frameFixture from "./fixtures/geomap_frame.json";
import dashboard from "./fixtures/dashboards/air-quality-map.json";

// jsdom has no WebGL: the map is a recorder of what the panel asks MapLibre for.
const calls = vi.hoisted(() => ({
  constructed: [] as Record<string, unknown>[],
  sources: [] as { id: string; source: { data: { features: { geometry: { coordinates: [number, number] }; properties: Record<string, string> }[] } } }[],
  layers: [] as { id: string; paint: Record<string, unknown> }[],
  fits: [] as unknown[],
  errorHandlers: [] as (() => void)[],
  styles: [] as unknown[],
  clicks: new Map<string, (event: unknown) => void>(),
  popups: [] as HTMLElement[],
  controls: [] as [string, string | undefined][],
}));

vi.mock("maplibre-gl", () => {
  class FakeMap {
    constructor(options: Record<string, unknown>) {
      calls.constructed.push(options);
    }
    addControl(control: unknown, at?: string) {
      calls.controls.push([control?.constructor.name ?? "", at]);
    }
    removeControl() {}
    addSource(id: string, source: never) {
      calls.sources.push({ id, source });
    }
    addLayer(layer: never) {
      calls.layers.push(layer);
    }
    on(event: string, second: unknown, third?: unknown) {
      if (event === "load" && typeof second === "function") (second as () => void)();
      if (event === "error" && typeof second === "function") calls.errorHandlers.push(second as () => void);
      if (event === "click" && typeof second === "string") calls.clicks.set(second, third as (event: unknown) => void);
    }
    once() {}
    off() {}
    remove() {}
    isStyleLoaded() {
      return false;
    }
    setStyle(style: unknown) {
      calls.styles.push(style);
    }
    getSource() {
      return undefined;
    }
    fitBounds(box: unknown) {
      calls.fits.push(box);
    }
    getBounds() {
      return { getWest: () => 0, getSouth: () => 0, getEast: () => 1, getNorth: () => 1 };
    }
  }
  class FakePopup {
    setLngLat() {
      return this;
    }
    setDOMContent(node: HTMLElement) {
      calls.popups.push(node);
      return this;
    }
    addTo() {
      return this;
    }
  }
  return { setWorkerUrl: () => {}, Map: FakeMap, NavigationControl: class {}, ScaleControl: class {}, Popup: FakePopup };
});

const frames = frameFixture as unknown as Frame[];
const panel = dashboard.panels[0] as unknown as PanelDefinition;

/** The frame with the location of the rows `drop` takes away. */
function without(drop: (row: number) => boolean): Frame[] {
  return frames.map((frame) => ({
    ...frame,
    fields: frame.fields.map((field) => (field.name === "location" ? { ...field, values: field.values.map((value, row) => (drop(row) ? null : value)) } : field)),
  }));
}

function draw(data: Frame[], definition: PanelDefinition = panel, project?: string) {
  return render(<PanelView panel={definition} data={{ frames: data }} from={0} to={1} locale="en" height={400} project={project} />);
}

beforeEach(() => {
  calls.constructed.length = 0;
  calls.sources.length = 0;
  calls.layers.length = 0;
  calls.fits.length = 0;
  calls.errorHandlers.length = 0;
  calls.styles.length = 0;
  calls.clicks.clear();
  calls.popups.length = 0;
  calls.controls.length = 0;
});

describe("the helsinki air quality geomap", () => {
  it("draws the 100 markers in the layer's fixed colour, size and opacity", () => {
    draw(frames);
    expect(screen.getByRole("application", { name: "Map of Air quality map (t6516-mw3xzrhm): 100 markers" })).toBeTruthy();
    expect(calls.sources).toHaveLength(1);
    expect(calls.sources[0].source.data.features).toHaveLength(100);
    expect(calls.layers[0].paint).toMatchObject({ "circle-color": "#FF9830", "circle-radius": 9, "circle-opacity": 0.9 });
    // Drawn, so no "shown as a table" word and no note.
    expect(screen.queryByText(/shown as a table/)).toBeNull();
    expect(document.querySelector("[data-geomap-note]")).toBeNull();
  });

  it("fits the map to the markers' extent", () => {
    draw(frames);
    const points = frames[0].fields.find((field) => field.name === "location")!.values.map((value) => (value as { coordinates: [number, number] }).coordinates);
    const xs = points.map(([x]) => x);
    const ys = points.map(([, y]) => y);
    expect(calls.fits).toEqual([[Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)]]);
  });

  it("opens a tooltip naming the vehicle first, its values in their units", () => {
    draw(frames);
    calls.clicks.get("markers")!({ features: [{ properties: calls.sources[0].source.data.features[0].properties }], lngLat: [0, 0] });
    const terms = [...calls.popups[0].querySelectorAll("dt")].map((term) => term.textContent);
    expect(terms[0]).toBe("name");
    expect(terms).toContain("pm10");
    expect(terms).not.toContain("location");
    expect(calls.popups[0].querySelector("dd")!.textContent).toBe("Volvo (pesuauto)");
    expect(calls.popups[0].textContent).toMatch(/µg\/m³|μg\/m³/);
  });

  it("lists every stacked marker under a tap, five and a count of the rest", () => {
    draw(frames);
    const under = calls.sources[0].source.data.features.slice(0, 7).map((feature) => ({ properties: feature.properties }));
    calls.clicks.get("markers")!({ features: under, lngLat: [0, 0] });
    expect(calls.popups[0].querySelectorAll("dl")).toHaveLength(5);
    expect(calls.popups[0].textContent).toContain("2 more here");
  });

  it("opens no tooltip when the layer has it off", () => {
    const off = structuredClone(panel);
    (off.options!.layers as { tooltip: boolean }[])[0].tooltip = false;
    draw(frames, off);
    calls.clicks.get("markers")!({ features: [{ properties: {} }], lngLat: [0, 0] });
    expect(calls.popups).toHaveLength(0);
  });

  it("shows the designed empty state for a frame without rows", () => {
    const empty = frames.map((frame) => ({ ...frame, fields: frame.fields.map((field) => ({ ...field, values: [] })) }));
    draw(empty);
    expect(screen.getByRole("status").textContent).toContain("No data");
    expect(calls.constructed).toHaveLength(0);
  });

  it("says how many rows have no location and draws the rest", () => {
    draw(without((row) => row < 3));
    expect(screen.getByText("3 of 100 rows have no location and are not drawn.")).toBeTruthy();
    expect(calls.sources[0].source.data.features).toHaveLength(97);
  });

  it("says no row has a location instead of an empty map", () => {
    draw(without(() => true));
    expect(screen.getByRole("status").textContent).toContain("No row has a location");
    expect(calls.constructed).toHaveLength(0);
  });

  it("notes another basemap and a fixed view in one line, and still draws", () => {
    const other = structuredClone(panel);
    other.options!.basemap = { type: "xyz" };
    other.options!.view = { id: "coords", lat: 60.17, lon: 24.94, zoom: 12 };
    draw(frames, other);
    const note = document.querySelector("[data-geomap-note]")!;
    expect(note.textContent).toBe("The “xyz” basemap is not drawn here. The saved view is not kept; the map fits its markers.");
    expect(note.textContent).not.toMatch(/[{}]/);
    expect(calls.sources[0].source.data.features).toHaveLength(100);
    expect(calls.fits).toHaveLength(1);
  });

  it("names a location mode it does not read", () => {
    const geohash = structuredClone(panel);
    (geohash.options!.layers as { location: unknown }[])[0].location = { mode: "geohash", geohash: "hash" };
    expect(markersOf(frames, geohash.options).notes).toEqual([{ key: "mode", values: { mode: "geohash" } }]);
    draw(frames, geohash);
    expect(screen.getByRole("status").textContent).toContain("not by “geohash”");
  });

  it("reads latitude and longitude fields where the frame has no point", () => {
    const flat: Frame[] = [
      {
        fields: [
          { name: "name", type: "string", values: ["a", "b"] },
          { name: "latitude", type: "number", values: [60.1, null] },
          { name: "longitude", type: "number", values: [24.9, 25] },
        ],
      },
    ];
    const markers = markersOf(flat, { layers: [{ type: "markers" }] });
    expect(markers.features.features.map((feature) => feature.geometry.coordinates)).toEqual([[24.9, 60.1]]);
    // Grafana's defaults, its `green` in the palette every panel of the page paints with.
    expect(markers.marker).toEqual({ color: colorOf("green"), size: 5, opacity: 0.4 });
  });
});

describe("the geomap's basemap", () => {
  it("draws on the plain ground without a project, fetching nothing, and says so beside a scale", () => {
    draw(frames);
    expect(calls.constructed[0].style).toEqual(blankStyle());
    expect(document.querySelector("[data-geomap-caption]")!.textContent).toBe("Markers: 100 · no basemap on this platform, positions only");
    expect(calls.controls).toContainEqual(["ScaleControl", "bottom-left"]);
  });

  it("asks the project's basemap route on the Portal's own origin, no token in the address, and falls back to the plain ground on a 404", () => {
    draw(frames, panel, "helsinki");
    const style = calls.constructed[0].style as string;
    const url = new URL(style);
    expect(url.origin).toBe(window.location.origin);
    expect(url.pathname).toBe("/api/v1/projects/helsinki/basemap/default/style.json");
    expect(url.search).toBe("");
    expect(document.querySelector("[data-geomap-caption]")!.textContent).toBe("Markers: 100");
    act(() => calls.errorHandlers[0]());
    expect(calls.styles).toEqual([blankStyle()]);
    expect(document.querySelector("[data-geomap-caption]")!.textContent).toBe("Markers: 100 · no basemap on this platform, positions only");
  });
});

it("keeps the map's tooltip text out of markup", () => {
  const hostile: Frame[] = [{ fields: [{ name: "name", type: "string", values: ["<img src=x onerror=alert(1)>"] }, { name: "location", type: "json", values: [{ type: "Point", coordinates: [1, 2] }] }] }];
  draw(hostile);
  calls.clicks.get("markers")!({ features: [{ properties: calls.sources[0].source.data.features[0].properties }], lngLat: [0, 0] });
  expect(calls.popups[0].querySelector("img")).toBeNull();
  fireEvent.click(calls.popups[0]);
});

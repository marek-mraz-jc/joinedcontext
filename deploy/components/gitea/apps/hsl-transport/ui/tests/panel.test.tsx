/**
 * SDK-40, T-3399: a bus opens in the shell's entity panel, from the map or from the list of those
 * in view, read from this app's own backend (the endpoint never reaches the browser). The app
 * writes nothing, so the panel offers no Edit and links the bus to the Portal where the page
 * names the project's page there.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/** What a click on the map hands the app, as MapLibre would. */
let clickBus: ((event: { features?: Array<{ properties?: Record<string, unknown> }> }) => void) | null = null;

vi.mock("maplibre-gl", () => {
  class Map {
    on(event: string, layerOrHandler: unknown, handler?: unknown) {
      if (event === "load") (layerOrHandler as () => void)();
      if (event === "click" && handler) clickBus = handler as typeof clickBus;
    }
    addSource = vi.fn();
    addLayer = vi.fn();
    addImage = vi.fn();
    hasImage = () => false;
    resize = () => undefined;
    getBounds = () => ({ toArray: () => [[24.5, 60.0], [25.5, 60.4]] });
    getSource = () => ({ setData: vi.fn() });
    remove = vi.fn();
  }
  return { Map };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

import { App, portalOf } from "../src/App";
import { panelSource, type Vehicle } from "../src/api";

const BUS = "urn:ngsi-ld:Vehicle:hel.fi:helsinki:12-2201";
const QUIET = "urn:ngsi-ld:Vehicle:hel.fi:helsinki:12-2204";
let fleet: Vehicle[];

class StubEventSource {
  addEventListener() {}
  close() {}
}

function page(config: unknown) {
  const script = document.createElement("script");
  script.id = "jc-config";
  script.type = "application/json";
  script.textContent = typeof config === "string" ? config : JSON.stringify(config);
  document.head.append(script);
}

beforeEach(() => {
  fleet = [
    { id: BUS, coordinates: [24.99, 60.29], bearing: 280, speed: 13.4, refLine: "4570" },
    { id: QUIET, coordinates: [25.09, 60.26] },
  ];
  vi.stubGlobal("EventSource", StubEventSource);
  vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify(fleet), { status: 200 })));
});

afterEach(() => {
  document.getElementById("jc-config")?.remove();
  vi.unstubAllGlobals();
  // The handler belongs to the map of the test that drew it; the next test waits for its own.
  clickBus = null;
});

describe("a bus in the entity panel", () => {
  it("opens from the map with its line and speed, links to the Portal, offers no Edit, and closes", async () => {
    page({ slug: "s", space: "helsinki", portal: "https://portal.test/projects/helsinki" });
    render(<App />);
    await screen.findByText(/^2 vehicles/);
    // The map binds its click in an effect that may run after the list is on screen (T-4257).
    await waitFor(() => expect(clickBus).not.toBeNull());
    act(() => clickBus?.({ features: [{ properties: {} }] }));
    expect(screen.queryByRole("dialog")).toBeNull();
    act(() => clickBus?.({ features: [{ properties: { id: BUS } }] }));
    const panel = await screen.findByRole("dialog");
    expect(await within(panel).findByText("4570")).toBeInTheDocument();
    expect(within(panel).getByText(/13\.4/)).toBeInTheDocument();
    const link = within(panel).getByRole("link", { name: "Open in the Portal" });
    expect(link).toHaveAttribute("href", expect.stringContaining(encodeURIComponent(BUS)));
    expect(within(panel).queryByRole("button", { name: "Edit" })).toBeNull();
    const user = userEvent.setup();
    // The Portal opens in a new tab, the browser's to follow; the test stops it there.
    const stop = (event: Event) => event.preventDefault();
    document.addEventListener("click", stop);
    await user.click(link);
    document.removeEventListener("click", stop);
    await user.click(within(panel).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("opens from the list of the buses in view, by fleet number and line", async () => {
    render(<App />);
    const list = await screen.findByRole("combobox", { name: "Open a vehicle" });
    expect(Array.from((list as HTMLSelectElement).options).map((option) => option.textContent)).toEqual(["—", "12-2201, line 4570", "12-2204"]);
    await userEvent.setup().selectOptions(list, QUIET);
    const panel = await screen.findByRole("dialog");
    expect(await within(panel).findByText("Vehicle")).toBeInTheDocument();
    expect(within(panel).queryByRole("link")).toBeNull();
  });

  it("says a bus the backend no longer reports is gone", async () => {
    render(<App />);
    const list = await screen.findByRole("combobox", { name: "Open a vehicle" });
    fleet = [];
    await userEvent.setup().selectOptions(list, BUS);
    expect(await within(await screen.findByRole("dialog")).findByText("This entity is no longer there.")).toBeInTheDocument();
  });

  it("offers no list while no bus is reported", async () => {
    fleet = [];
    render(<App />);
    await screen.findByText("Waiting for the first positions");
    expect(screen.queryByRole("combobox", { name: "Open a vehicle" })).toBeNull();
  });
});

describe("the panel's source", () => {
  it("writes nothing, edits nothing, and links only with the project's page and the space", async () => {
    const quiet = panelSource({});
    await expect(quiet.update({ id: BUS, type: "Vehicle" }, {})).rejects.toMatchObject({ status: 403 });
    expect(quiet.mayEdit("Vehicle")).toBe(false);
    expect(quiet.portalLink?.({ id: BUS, type: "Vehicle" })).toBeNull();
    expect(panelSource({ portal: "https://portal.test/projects/helsinki" }).portalLink?.({ id: BUS, type: "Vehicle" })).toBeNull();
    expect(panelSource({ portal: "https://portal.test/projects/helsinki", space: "helsinki" }).portalLink?.({ id: BUS, type: "Vehicle" })).toContain("https://portal.test/projects/helsinki");
  });

  it("reads the project's page only as https from a page that names it", () => {
    expect(portalOf()).toEqual({});
    page("{not json");
    expect(portalOf()).toEqual({});
    document.getElementById("jc-config")?.remove();
    page("null");
    expect(portalOf()).toEqual({});
    document.getElementById("jc-config")?.remove();
    page({ portal: "javascript:alert(1)", space: 3 });
    expect(portalOf()).toEqual({});
    document.getElementById("jc-config")?.remove();
    page({ portal: "https://portal.test/projects/helsinki", space: "helsinki" });
    expect(portalOf()).toEqual({ portal: "https://portal.test/projects/helsinki", space: "helsinki" });
  });
});

describe("the reader's way back", () => {
  it("gives the focus back to the list after the panel closes", async () => {
    render(<App />);
    const list = await screen.findByRole("combobox", { name: "Open a vehicle" });
    const user = userEvent.setup();
    await user.selectOptions(list, BUS);
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(list).toHaveFocus());
  });
});

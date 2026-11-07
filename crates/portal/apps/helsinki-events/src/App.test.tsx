import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { EVENTS } from "./fixtures/events";

/** What a click on an event of the map hands the page, as MapLibre would; the last map built's. */
let clickEvent: (event: { features?: Array<{ properties?: { id?: string } }> }) => void = () => undefined;
vi.mock("maplibre-gl", () => ({
  Map: class {
    on(event: string, layer: unknown, handler?: unknown) {
      if (event === "load" && typeof layer === "function") (layer as () => void)();
      if (event === "click" && layer === "jc-points") clickEvent = handler as typeof clickEvent;
    }
    addSource = vi.fn();
    addLayer = vi.fn();
    getSource = () => ({ setData: vi.fn() });
    fitBounds = vi.fn();
    setPaintProperty = vi.fn();
    remove = vi.fn();
  },
  setWorkerUrl: vi.fn(),
}));
vi.mock("@deck.gl/mapbox", () => ({ MapboxOverlay: class {} }));
vi.mock("@deck.gl/layers", () => ({ ScatterplotLayer: class {} }));
vi.mock("@deck.gl/aggregation-layers", () => ({ HexagonLayer: class {}, GridLayer: class {} }));
// jsdom has no canvas; the charts themselves are tested in pages/Events.test.tsx.
vi.mock("echarts", () => ({ init: vi.fn(() => ({ setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn(), on: vi.fn() })) }));

const READ = {
  permissions: [{ resource: { type: "Event" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const }],
  prohibitions: [],
};

function app() {
  const client = stubClient({ entities: EVENTS, access: READ }, { appName: "helsinki-events" });
  render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
  return client;
}

describe("helsinki-events", () => {
  // AP-04, AP-138: one page, read through the app's own endpoint only, never written to.
  it("opens on the events, read only through the app's endpoint", async () => {
    const client = app();
    expect(screen.getByRole("heading", { level: 1, name: "Helsinki events" })).toBeInTheDocument();
    const events = await screen.findByRole("region", { name: "Events" });
    await waitFor(() => expect(within(events).getByRole("heading", { name: "Organ concert" })).toBeInTheDocument());
    expect(client.transport.calls.length).toBeGreaterThan(0);
    expect(client.transport.calls.every((call) => call.path.includes("/api/endpoint/"))).toBe(true);
    expect(client.transport.calls.every((call) => call.method === "GET")).toBe(true);
  });

  // SDK-40, T-3397: an event opens in the shell's entity panel from the list or the map; a public
  // App writes nothing, so the panel offers no Edit.
  it("opens every listed event and a clicked marker in the entity panel, without Edit", async () => {
    app();
    const list = await screen.findByRole("list", { name: "Upcoming events" });
    for (const name of ["Workshop for Families", "Organ concert", "Story hour", "Dance workshop", "Jazz at Stoa"]) {
      fireEvent.click(within(list).getByRole("button", { name }));
      const panel = await screen.findByRole("dialog");
      await waitFor(() => expect(within(panel).getAllByText(name).length).toBeGreaterThan(0));
      expect(within(panel).queryByRole("button", { name: "Edit" })).toBeNull();
      fireEvent.click(within(panel).getByRole("button", { name: "Close" }));
      await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    }
    // A marker the map no longer holds opens nothing; one it holds opens its event.
    act(() => clickEvent({ features: [{ properties: { id: "urn:ngsi-ld:Event:hel.fi:helsinki:gone" } }] }));
    act(() => clickEvent({ features: [] }));
    expect(screen.queryByRole("dialog")).toBeNull();
    act(() => clickEvent({ features: [{ properties: { id: String(EVENTS[1].id) } }] }));
    const panel = await screen.findByRole("dialog");
    await waitFor(() => expect(within(panel).getAllByText("Organ concert").length).toBeGreaterThan(0));
  });
});

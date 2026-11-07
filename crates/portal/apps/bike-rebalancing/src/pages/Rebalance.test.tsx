import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import { STATIONS } from "../fixtures/stations";
import type { PlansApi } from "../plans";
import { choiceOf, fillBands, Rebalance, vanOf, watched } from "./Rebalance";

vi.mock("maplibre-gl", () => ({
  Map: class {
    on = vi.fn();
    remove = vi.fn();
  },
  LngLatBounds: class {
    extend = vi.fn();
  },
  Popup: class {},
  setWorkerUrl: vi.fn(),
}));
// jsdom has no canvas; what the chart is given is tested through fillBands.
vi.mock("echarts/core", () => ({ use: vi.fn(), init: vi.fn(() => ({ setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn(), on: vi.fn() })) }));

/** The App's server with no plan saved yet: these tests are about the planner on the page. */
const NO_PLANS: PlansApi = {
  list: vi.fn(async () => []),
  save: vi.fn(),
  get: vi.fn(),
  remove: vi.fn(),
  drive: vi.fn(),
  sheetUrl: vi.fn(),
  handOver: vi.fn(),
  me: vi.fn(async () => null),
  levels: vi.fn(),
  setLevels: vi.fn(),
  history: vi.fn(),
};

const READ = {
  permissions: [{ resource: { type: "BikeHireDockingStation" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const }],
  prohibitions: [],
};

function show(entities = STATIONS, language = "en") {
  const client = stubClient({ entities, access: READ }, { appName: "bike-rebalancing", language });
  render(
    <JcProvider client={client}>
      <Rebalance plans={NO_PLANS} />
    </JcProvider>,
  );
  return client;
}

beforeEach(() => window.history.replaceState(null, "", "/"));
afterEach(() => window.history.replaceState(null, "", "/"));

// T-3328: the first screen answers the operator's question without a click.
describe("Rebalance", () => {
  it("counts the stations about to run empty and full and lists the van's stops", async () => {
    const client = show();
    const summary = screen.getByRole("list", { name: "Rebalancing" });
    await waitFor(() => expect(within(summary).getByText("About to run empty").nextSibling).toHaveTextContent("3"));
    expect(within(summary).getByText("About to run empty").nextSibling).toHaveTextContent("3");
    expect(within(summary).getByText("About to be full").nextSibling).toHaveTextContent("2");
    const route = await screen.findByRole("list", { name: "Route" });
    const stops = within(route).getAllByRole("listitem");
    expect(stops.length).toBeGreaterThan(1);
    expect(within(route).queryByText("Rautatientori")).toBeNull();
    expect(client.transport.calls.every((call) => call.method === "GET" && call.path.includes("/api/endpoint/"))).toBe(true);
  });

  it("leaves a station out of the route at a click, and keeps the choice in the address", async () => {
    show();
    const route = await screen.findByRole("list", { name: "Route" });
    const first = within(route).getAllByRole("listitem")[0];
    const station = first.querySelector("strong")?.textContent ?? "";
    expect(station).not.toBe("");
    fireEvent.click(within(first).getByRole("button", { name: `Leave out of the route: ${station}` }));
    await waitFor(() => expect(within(screen.getByRole("list", { name: "Route" })).queryByText(station)).toBeNull());
    expect(new URLSearchParams(window.location.search).get("skip")).toContain("BikeHireDockingStation");
    fireEvent.click(screen.getByRole("button", { name: `Put back in the route: ${station}` }));
    await waitFor(() => expect(within(screen.getByRole("list", { name: "Route" })).getByText(station)).toBeInTheDocument());
    expect(new URLSearchParams(window.location.search).get("skip")).toBeNull();
  });

  it("reads the van's capacity from the address and plans within it", async () => {
    window.history.replaceState(null, "", "/?van=3");
    show();
    expect(screen.getByRole("spinbutton", { name: "Van capacity (bikes)" })).toHaveValue(3);
    const route = await screen.findByRole("list", { name: "Route" });
    for (const stop of within(route).getAllByRole("listitem")) {
      const load = Number(/(\d+) on board/.exec(stop.textContent ?? "")?.[1]);
      expect(load).toBeLessThanOrEqual(3);
    }
  });

  it("speaks Finnish when the Portal does", async () => {
    show(STATIONS, "fi");
    expect(await screen.findByText("Tyhjenemässä")).toBeInTheDocument();
    expect(await screen.findByRole("list", { name: "Reitti" })).toBeInTheDocument();
  });

  it("says so when there is no station", async () => {
    show([]);
    expect(await screen.findAllByText("No docking station was found.")).not.toHaveLength(0);
  });

  it("says in words when the planner fails, and keeps the stations' counts", async () => {
    // A worker that refuses every plan, as one whose module could not load would.
    vi.stubGlobal(
      "Worker",
      class {
        onmessage: ((event: { data: { id: number; error: string } }) => void) | null = null;
        postMessage({ id }: { id: number }) {
          queueMicrotask(() => this.onmessage?.({ data: { id, error: "out of memory" } }));
        }
      },
    );
    try {
      show();
      expect(await screen.findByRole("alert")).toHaveTextContent("The route could not be planned: out of memory");
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("says in words when the stations cannot be read, and offers to try again", async () => {
    const client = stubClient(
      { entities: STATIONS, access: READ, refuse: () => ({ status: 503, body: { title: "Unavailable" } }) },
      { appName: "bike-rebalancing", language: "en" },
    );
    render(
      <JcProvider client={client}>
        <Rebalance plans={NO_PLANS} />
      </JcProvider>,
    );
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The stations could not be read (HTTP 503). Try again later.");
    // Retry asks the endpoint again, and is refused again.
    const asked = client.transport.calls.length;
    fireEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(client.transport.calls.length).toBeGreaterThan(asked));
  });
});

describe("the page's pure parts", () => {
  it("names a station's choice by what its click does", () => {
    const none = { add: [] as string[], skip: [] as string[], route: new Set<string>() };
    // A station about to run empty or full is routed unless left out, whether or not the plan on
    // screen has reached it yet: its button leaves it out, and says so.
    expect(choiceOf("a", true, none)).toBe("leaveOut");
    expect(choiceOf("a", false, { ...none, route: new Set(["a"]) })).toBe("leaveOut");
    expect(choiceOf("a", false, { ...none, add: ["a"] })).toBe("leaveOut");
    expect(choiceOf("a", true, { ...none, skip: ["a"] })).toBe("putBack");
    expect(choiceOf("a", false, none)).toBe("addIn");
  });

  it("takes a van capacity only in range", () => {
    expect(vanOf("12")).toBe(12);
    for (const wrong of ["", "0", "-4", "2.5", "201", "many"]) expect(vanOf(wrong)).toBe(20);
  });

  it("bands the stations by tenth of fill, the full ones in the last", () => {
    const bands = fillBands([
      { id: "a", level: "empty", fill: 0, surplus: -5 },
      { id: "b", level: "full", fill: 1, surplus: 5 },
      { id: "c", level: "unknown", fill: null, surplus: 0 },
      { id: "d", level: "balanced", fill: 0.55, surplus: 0 },
    ]);
    expect(bands).toEqual([1, 0, 0, 0, 0, 1, 0, 0, 0, 1]);
  });

  it("lists the stations the operator touched first, then the most out of balance", () => {
    const needs = [
      { id: "a", level: "low" as const, fill: 0.1, surplus: -4 },
      { id: "b", level: "full" as const, fill: 1, surplus: 9 },
      { id: "c", level: "balanced" as const, fill: 0.5, surplus: 0 },
      { id: "d", level: "unknown" as const, fill: null, surplus: 0 },
    ];
    expect(watched(needs, new Set(["c"])).map((n) => n.id)).toEqual(["c", "b", "a"]);
    expect(watched(needs, new Set(), 1).map((n) => n.id)).toEqual(["b"]);
  });
});

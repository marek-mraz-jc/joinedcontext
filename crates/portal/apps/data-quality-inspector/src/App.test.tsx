import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider, ProblemError } from "@joinedcontext/sdk";
import type { Schema } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { Problem as ServerProblem, ServerContext } from "./server";
import type { Run, Server } from "./server";
import { completenessBarOption } from "./pages/Quality";
import { InspectorContext, parseAnswer, type Inspector } from "./inspect";
import { ENTITIES, NOW, SCHEMA_WITH_VEHICLE } from "./fixtures/quality";
import { roles } from "./testing/roles";
import wasm from "../wasm/pkg/data_quality_inspector_bg.wasm?url&inline";
import { initSync, inspect } from "../wasm/pkg/data_quality_inspector.js";

const base64 = wasm.slice(wasm.indexOf(",") + 1);
initSync({ module: Uint8Array.from(atob(base64), (c) => c.charCodeAt(0)) });

const runInspector: Inspector = async (input) => parseAnswer(inspect(JSON.stringify(input)));

/** What ECharts would call on a click of a bar, kept by the double below. */
const chart = vi.hoisted(() => ({ click: null as ((params: unknown) => void) | null }));

vi.mock("echarts", () => ({
  init: vi.fn(() => ({
    setOption: vi.fn(),
    resize: vi.fn(),
    dispose: vi.fn(),
    on: (event: string, handler: (params: unknown) => void) => {
      if (event === "click") chart.click = handler;
    },
  })),
}));

const ACCESS = {
  permissions: [
    { resource: { type: "BikeHireDockingStation" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const },
    { resource: { type: "AdministrativeArea" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const },
    { resource: { type: "Event" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const },
    { resource: { type: "Vehicle" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const },
  ],
  prohibitions: [],
};

// The stub serves the schema document as given; the SDK types it as `Schema`, which names only the
// keywords its forms read, while the published document (and this fixture) carries every keyword the
// validator checks. One cast, here, where the fixture meets the stub.
const STUB_SCHEMA = SCHEMA_WITH_VEHICLE as unknown as Schema;

/** The App's server in memory (T-3354): the runs its daily job kept, and each run's report. */
function fakeServer(start: Run[] = [], fail: { runs?: string; report?: string } = {}, stale = false): Server & { kept: Run[] } {
  const kept = [...start];
  return {
    ...roles(),
    kept,
    runs: async () => {
      if (fail.runs) throw new ServerProblem(502, fail.runs);
      return { runs: [...kept].reverse(), stale };
    },
    reportUrl: async (id) => {
      if (fail.report) throw new ServerProblem(404, fail.report);
      return `https://store.test/runs/${id}/report.json`;
    },
  };
}

function show(
  client = stubClient(
    { entities: ENTITIES, schema: STUB_SCHEMA, access: ACCESS },
    { appName: "data-quality-inspector", portal: "https://portal.hel.fi/projects/helsinki" },
  ),
  inspector: Inspector = runInspector,
  server: Server = fakeServer(),
) {
  render(
    <JcProvider client={client}>
      <ServerContext.Provider value={server}>
        <InspectorContext.Provider value={inspector}>
          <App />
        </InspectorContext.Provider>
      </ServerContext.Provider>
    </JcProvider>,
  );
  return client;
}

describe("data-quality-inspector", () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(NOW * 1000));
    window.history.replaceState(null, "", "/?lang=en");
  });

  afterEach(() => {
    vi.useRealTimers();
    window.history.replaceState(null, "", "/");
  });

  it("renders first screen with tiles sorted worst valid first and completeness chart", async () => {
    show();

    expect(screen.getByRole("heading", { level: 1, name: "Helsinki data quality" })).toBeInTheDocument();

    // Type buttons bar contains all 4 types
    const typeList = await screen.findByRole("list", { name: "Entity types" });
    expect(within(typeList).getByRole("button", { name: "BikeHireDockingStation" })).toBeInTheDocument();
    expect(within(typeList).getByRole("button", { name: "AdministrativeArea" })).toBeInTheDocument();
    expect(within(typeList).getByRole("button", { name: "Event" })).toBeInTheDocument();
    expect(within(typeList).getByRole("button", { name: "Vehicle" })).toBeInTheDocument();

    // Wait for overview tiles to render
    const bikeTile = await screen.findByRole("button", { name: /^BikeHireDockingStation: 6 entities/i });
    expect(bikeTile).toBeInTheDocument();
    expect(within(bikeTile).getByText("33 %")).toBeInTheDocument(); // valid 2/6
    expect(within(bikeTile).getByText("5 min")).toBeInTheDocument(); // median age

    const districtTile = screen.getByRole("button", { name: /^AdministrativeArea: 2 entities/i });
    expect(within(districtTile).getByText("50 %")).toBeInTheDocument(); // valid 1/2
    expect(within(districtTile).getByText("no timestamp")).toBeInTheDocument();

    const eventTile = screen.getByRole("button", { name: /^Event: 2 entities/i });
    expect(within(eventTile).getByText("50 %")).toBeInTheDocument(); // valid 1/2
    expect(within(eventTile).getByText("3 h")).toBeInTheDocument();

    const vehicleTile = screen.getByRole("button", { name: /^Vehicle: 2 entities/i });
    expect(within(vehicleTile).getByText("no published schema")).toBeInTheDocument();

    // Verify ordering: BikeHireDockingStation (33%) is first tile, Vehicle (null) is last
    const tiles = screen.getAllByRole("button", { name: /entities$/i });
    expect(tiles[0]).toHaveTextContent("BikeHireDockingStation");
    expect(tiles[3]).toHaveTextContent("Vehicle");

    // Completeness bar chart canvas
    expect(screen.getByRole("img", { name: "Completeness by type" })).toBeInTheDocument();
  });

  it("chooses BikeHireDockingStation by button, updating aria-pressed, hash, and tables", async () => {
    show();
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });

    const bikeButton = await screen.findByRole("button", { name: "BikeHireDockingStation" });
    expect(bikeButton).toHaveAttribute("aria-pressed", "false");

    await user.click(bikeButton);
    expect(bikeButton).toHaveAttribute("aria-pressed", "true");
    expect(decodeURIComponent(window.location.hash)).toContain("#quality?type=BikeHireDockingStation");

    // Freshness details
    expect(await screen.findByRole("table", { name: "Attributes of BikeHireDockingStation" })).toBeInTheDocument();
    expect(screen.getByText("5 min")).toBeInTheDocument();

    // Attributes table
    const attrTable = screen.getByRole("table", { name: "Attributes of BikeHireDockingStation" });
    expect(within(attrTable).getByText("availableBikeNumber")).toBeInTheDocument();
    expect(within(attrTable).getByText("dateModified")).toBeInTheDocument();
    expect(within(attrTable).getByText("location")).toBeInTheDocument();
    expect(within(attrTable).getByText("name")).toBeInTheDocument();
    expect(within(attrTable).getByText("status")).toBeInTheDocument();

    // Failing entities table with plain words
    const failTable = screen.getByRole("table", { name: "Failing entities" });
    const failRows = within(failTable).getAllByRole("row");
    // Header row + 4 failing rows = 5 rows
    expect(failRows).toHaveLength(5);

    expect(within(failTable).getByText("fail-neg")).toBeInTheDocument();
    expect(within(failTable).getByText("below the minimum 0")).toBeInTheDocument();

    expect(within(failTable).getByText("fail-float")).toBeInTheDocument();
    expect(within(failTable).getByText("not of type integer")).toBeInTheDocument();

    expect(within(failTable).getByText("fail-date")).toBeInTheDocument();
    expect(within(failTable).getByText("not a date-time")).toBeInTheDocument();

    // Through the SDK a geometry without coordinates arrives as its `type` string ("Point"): the
    // steward reads that the value is not a geometry. wasm.test.ts covers the `required` path.
    expect(within(failTable).getByText("fail-loc")).toBeInTheDocument();
    expect(within(failTable).getByText("not of type object")).toBeInTheDocument();
  });

  it("displays 'no published schema' for Vehicle in tile and detail screen", async () => {
    show();
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });

    const vehicleBtn = await screen.findByRole("button", { name: "Vehicle" });
    await user.click(vehicleBtn);

    expect(decodeURIComponent(window.location.hash)).toContain("#quality?type=Vehicle");
    expect(await screen.findByText("No published schema; validity could not be checked.")).toBeInTheDocument();
    expect(screen.getByText("All entities pass validation.")).toBeInTheDocument();
  });

  it("handles one refused type, displaying error tile while others remain", async () => {
    const client = stubClient(
      { entities: ENTITIES, schema: STUB_SCHEMA, access: ACCESS },
      { appName: "data-quality-inspector" },
    );
    const all = client.entities.all.bind(client.entities);
    client.entities.all = (async (type: string, query?: Parameters<typeof all>[1]) => {
      if (type === "Event") {
        throw new ProblemError(500, { title: "Event service unavailable", status: 500 });
      }
      return all(type, query);
    }) as typeof client.entities.all;

    show(client);

    // Event tile displays error message
    const errorMsg = await screen.findByText("Data could not be read.");
    expect(errorMsg).toBeInTheDocument();
    expect(errorMsg.closest(".app-tile-error")).toHaveTextContent("Event");

    // Other tiles remain functional
    expect(await screen.findByRole("button", { name: /^BikeHireDockingStation: 6 entities/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^AdministrativeArea: 2 entities/i })).toBeInTheDocument();
  });

  it("reads two types at a time, so the gateway's rate limit is not hit, and still reads every one", async () => {
    const client = stubClient(
      { entities: ENTITIES, schema: STUB_SCHEMA, access: ACCESS },
      { appName: "data-quality-inspector" },
    );
    const all = client.entities.all.bind(client.entities);
    let open = 0;
    let most = 0;
    const read: string[] = [];
    client.entities.all = (async (type: string, query?: Parameters<typeof all>[1]) => {
      open += 1;
      most = Math.max(most, open);
      await new Promise((resolve) => setTimeout(resolve, 5));
      read.push(type);
      open -= 1;
      return all(type, query);
    }) as typeof client.entities.all;

    show(client);

    expect(await screen.findByRole("button", { name: /^BikeHireDockingStation: 6 entities/i })).toBeInTheDocument();
    await waitFor(() => expect(read.length).toBeGreaterThanOrEqual(4));
    expect(most).toBe(2);
    expect(new Set(read).size).toBe(read.length);
  });

  it("handles refused schema, showing notice that validity is not checked", async () => {
    const client = stubClient(
      { entities: ENTITIES, access: ACCESS },
      { appName: "data-quality-inspector" },
    );
    client.schema = vi.fn().mockRejectedValue(new ProblemError(503, { title: "Schema broker down" }));

    show(client);

    expect(
      await screen.findByText("The published schema could not be read; validity is not checked."),
    ).toBeInTheDocument();
    // Without a schema the types are the ones dev holds: each opens, and closes again.
    const typeList = await screen.findByRole("list", { name: "Entity types" });
    for (const button of within(typeList).getAllByRole("button")) {
      fireEvent.click(button);
      expect(button).toHaveAttribute("aria-pressed", "true");
      fireEvent.click(button);
      expect(button).toHaveAttribute("aria-pressed", "false");
    }
    // A type dev holds none of has its tile too, which opens its type and says it holds nothing.
    // The tiles come with the second answer, after the notice: wait for them, do not race them.
    const tiles = await screen.findAllByRole("button", { name: / entities$/ });
    for (const name of tiles.map((tile) => tile.getAttribute("aria-label")!)) {
      fireEvent.click(screen.getByRole("button", { name }));
      fireEvent.click(await screen.findByRole("button", { name: "← All types" }));
    }
  }, 20_000); // every tile opened and closed under coverage: 868 ms here, past 5 s on a CI runner (ci-full 37840079043)

  it("supports Finnish with ?lang=fi and toggles language", async () => {
    window.history.replaceState(null, "", "/?lang=fi");
    show();

    expect(await screen.findByRole("heading", { level: 1, name: "Helsingin datan laatu" })).toBeInTheDocument();
    expect(await screen.findByRole("img", { name: "Tietotyyppien kattavuus" })).toBeInTheDocument();

    fireEvent.change(screen.getByRole("combobox", { name: "Kieli" }), { target: { value: "en" } });

    expect(screen.getByRole("heading", { level: 1, name: "Helsinki data quality" })).toBeInTheDocument();
    expect(document.documentElement.lang).toBe("en");
    await waitFor(() => {
      expect(new URLSearchParams(window.location.search).get("lang")).toBe("en");
    });
  });

  it.each([
    ["en", "Entity types", "entities", "All types", "Language", "fi"],
    ["fi", "Tietotyypit", "kohteet", "Kaikki tyypit", "Kieli", "en"],
  ] as const)("in %s, every type and tile opens its type, and Back returns", async (lang, list, entities, back, language, other) => {
    window.history.replaceState(null, "", `/?lang=${lang}`);
    show();
    const typeList = await screen.findByRole("list", { name: list });
    await screen.findAllByRole("button", { name: new RegExp(`: \\d+ ${entities}$`) });
    for (const button of within(typeList).getAllByRole("button")) {
      fireEvent.click(button);
      expect(button).toHaveAttribute("aria-pressed", "true");
      fireEvent.click(await screen.findByRole("button", { name: `← ${back}` }));
    }
    const tiles = () => screen.getAllByRole("button", { name: new RegExp(`: \\d+ ${entities}$`) });
    for (const name of tiles().map((tile) => tile.getAttribute("aria-label")!)) {
      const tile = screen.getByRole("button", { name });
      // Every other tile by the keyboard, the rest by a click: both open the type.
      if (name.length % 2 === 0) fireEvent.keyDown(tile, { key: "Enter" });
      else fireEvent.click(tile);
      expect(decodeURIComponent(window.location.hash)).toContain(`type=${name.slice(0, name.indexOf(":"))}`);
      fireEvent.click(await screen.findByRole("button", { name: `← ${back}` }));
    }
    fireEvent.keyDown(tiles()[0], { key: " " });
    expect(await screen.findByRole("button", { name: `← ${back}` })).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("button", { name: `← ${back}` }), { key: "Tab" });
    fireEvent.change(screen.getByRole("combobox", { name: language }), { target: { value: other } });
    expect(new URLSearchParams(window.location.search).get("lang")).toBe(other);
  });

  it("opens a type from its bar in the chart, and nothing from a click beside the bars", async () => {
    // The chart takes its click handler in an effect that can run after the canvas is in the DOM:
    // wait for this render's handler, never call the one an earlier test's chart left behind.
    chart.click = null as typeof chart.click;
    show();
    await screen.findByRole("img", { name: "Completeness by type" });
    await waitFor(() => expect(chart.click).not.toBeNull());
    chart.click?.({});
    expect(screen.queryByRole("button", { name: "← All types" })).toBeNull();
    chart.click?.({ name: "Event" });
    expect(await screen.findByRole("button", { name: "← All types" })).toBeInTheDocument();
    expect(decodeURIComponent(window.location.hash)).toContain("type=Event");
  });

  it("opens each failing entity in the SDK's panel, linked to the Portal, with no Edit on a public App", async () => {
    show();
    for (const type of ["BikeHireDockingStation", "AdministrativeArea", "Event"]) {
    fireEvent.click(await screen.findByRole("button", { name: type }));
    const failing = await screen.findByRole("table", { name: "Failing entities" });
    // The ids open the panel; the state column's buttons are the steward's (T-3827).
    const idButtons = () => [...failing.querySelectorAll<HTMLButtonElement>("button.app-open")];
    const ids = idButtons();
    expect(ids.length).toBeGreaterThan(0);
    for (const id of new Set(ids.map((one) => one.textContent))) {
      fireEvent.click(idButtons().find((one) => one.textContent === id)!);
      const panel = await screen.findByRole("dialog");
      expect(within(panel).getByText(type)).toBeInTheDocument();
      const link = await within(panel).findByRole("link", { name: "Open in the Portal" });
      expect(link.getAttribute("href")).toContain("https://portal.hel.fi/projects/helsinki/explore?");
      link.addEventListener("click", (event) => event.preventDefault());
      fireEvent.click(link);
      expect(within(panel).queryByRole("button", { name: "Edit" })).toBeNull();
      fireEvent.click(within(panel).getByRole("button", { name: "Close" }));
      expect(screen.queryByRole("dialog")).toBeNull();
    }
    fireEvent.click(screen.getByRole("button", { name: "← All types" }));
    }
  });

  it("follows a type named in the address after the page opened, and goes back to all types without one", async () => {
    show();
    await screen.findByRole("img", { name: "Completeness by type" });
    window.location.hash = "#quality?type=Event";
    window.dispatchEvent(new HashChangeEvent("hashchange"));
    expect(await screen.findByRole("button", { name: "← All types" })).toBeInTheDocument();
    window.location.hash = "";
    window.dispatchEvent(new HashChangeEvent("hashchange"));
    expect(await screen.findByRole("img", { name: "Completeness by type" })).toBeInTheDocument();
  });

  it("says when no type could be read, and reads them all again on Retry", async () => {
    const client = stubClient({ entities: ENTITIES, schema: STUB_SCHEMA, access: ACCESS }, { appName: "data-quality-inspector" });
    const all = client.entities.all.bind(client.entities);
    let refuse = true;
    client.entities.all = (async (type: string, query?: Parameters<typeof all>[1]) => {
      if (refuse) throw new Error("offline");
      return all(type, query);
    }) as typeof client.entities.all;
    show(client);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Could not read entities for any type.");
    refuse = false;
    fireEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("button", { name: /^BikeHireDockingStation: 6 entities/i })).toBeInTheDocument();
  });

  it("says why one type could not be read when it is opened, reads it again on Retry, and ignores other keys on a tile", async () => {
    const client = stubClient({ entities: ENTITIES, schema: STUB_SCHEMA, access: ACCESS }, { appName: "data-quality-inspector" });
    const all = client.entities.all.bind(client.entities);
    let refuse = true;
    client.entities.all = (async (type: string, query?: Parameters<typeof all>[1]) => {
      if (refuse && type === "Event") throw "Event service unavailable";
      return all(type, query);
    }) as typeof client.entities.all;
    show(client);
    const tile = await screen.findByRole("button", { name: /^BikeHireDockingStation: 6 entities/i });
    fireEvent.keyDown(tile, { key: "a" });
    expect(screen.queryByRole("button", { name: "← All types" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Event" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Event service unavailable");
    refuse = false;
    fireEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("table", { name: "Failing entities" })).toBeInTheDocument();
  });

  it("offers Retry on the tile of a type it could not read", async () => {
    const client = stubClient({ entities: ENTITIES, schema: STUB_SCHEMA, access: ACCESS }, { appName: "data-quality-inspector" });
    const all = client.entities.all.bind(client.entities);
    let refuse = true;
    client.entities.all = (async (type: string, query?: Parameters<typeof all>[1]) => {
      if (refuse && type === "Event") throw "Event service unavailable";
      return all(type, query);
    }) as typeof client.entities.all;
    show(client);
    const retry = await screen.findByRole("button", { name: "Retry" });
    refuse = false;
    fireEvent.click(retry);
    expect(await screen.findByRole("button", { name: /^Event: \d+ entities/i })).toBeInTheDocument();
    expect(screen.queryByText("Data could not be read.")).toBeNull();
  });

  it("ignores a language it does not speak", async () => {
    show();
    const language = await screen.findByRole("combobox", { name: "Language" });
    fireEvent.change(language, { target: { value: "de" } });
    expect(new URLSearchParams(window.location.search).get("lang")).toBe("en");
  });

  it("says a failed inspection in words", async () => {
    show(undefined, async () => {
      throw new Error("out of memory");
    });
    expect(await screen.findByRole("alert")).toHaveTextContent("out of memory");
  });

  it("says a bar's type and share in the chart's tooltip", () => {
    const option = completenessBarOption(
      [{ type: "Event", entities: 2, completeness: 0.5, valid: 0.5, attributes: [], findings: [], findingsTotal: 0, freshness: null }],
      "en",
    ) as { tooltip: { formatter: (params: unknown) => string } };
    expect(option.tooltip.formatter([{ name: "Event", value: 50 }])).toBe("Event: 50%");
    expect(option.tooltip.formatter({ name: "Event", value: 50 })).toBe("Event: 50%");
    expect(completenessBarOption([], "en")).toBeNull();
  });

  // T-3354: every run the server keeps and each run's full report; the daily job makes them (T-3579).
  it("lists the kept runs, offers no run on request, and opens a run's full report", async () => {
    window.history.replaceState(null, "", "/?lang=en");
    const server = fakeServer(
      [
        { id: 1, ran_at: "2030-10-20T09:00:00Z", types: [], entities: 1234, findings: 7, completeness: 0.875, valid: null },
        { id: 2, ran_at: "2030-10-21T09:00:00Z", types: [], entities: 9, findings: 1, completeness: 0.9, valid: 0.5 },
      ],
      {},
      true,
    );
    show(undefined, runInspector, server);
    const user = userEvent.setup();
    const trend = await screen.findByRole("table", { name: "Quality over time: the runs the server keeps" });
    // A phone lays each cell out with its column's name beside it (components.css).
    expect([...trend.querySelectorAll("tbody tr:first-child td")].map((cell) => cell.getAttribute("data-label"))).toEqual([
      "Entities",
      "Complete",
      "Valid",
      "Findings",
      "Full report",
    ]);
    expect(within(trend).getAllByRole("row").map((row) => row.textContent)).toEqual([
      "Run atEntitiesCompleteValidFindingsFull report",
      "21 Oct 2030, 12:00990 %50 %1Full report",
      "20 Oct 2030, 12:001,23487.5 %–7Full report",
    ]);
    expect(screen.getByText("The newest run is over a day old: the daily run did not succeed. The App's owner sees why in the Portal.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Inspect now" })).toBeNull();
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    await user.click(screen.getByRole("button", { name: "Download the full report of the run at 20 Oct 2030, 12:00" }));
    await user.click(screen.getByRole("button", { name: "Download the full report of the run at 21 Oct 2030, 12:00" }));
    await waitFor(() => expect(open).toHaveBeenCalledTimes(2));
    expect(open).toHaveBeenCalledWith("https://store.test/runs/1/report.json", "_blank", "noopener");
    open.mockRestore();
  });

  it("says in Finnish why a report failed", async () => {
    window.history.replaceState(null, "", "/?lang=fi");
    const server = fakeServer([{ id: 3, ran_at: "2030-10-20T09:00:00Z", types: [], entities: 1, findings: 0, completeness: 1, valid: 1 }], {
      report: "no such run",
    });
    show(undefined, runInspector, server);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Lataa ajon 20.10.2030 klo 12.00 täysi raportti" }));
    expect(await screen.findByText("Raporttia ei voitu ladata: no such run")).toBeInTheDocument();
  });

  it("says when the server keeps no run yet", async () => {
    window.history.replaceState(null, "", "/?lang=en");
    show();
    expect(await screen.findByText("The server has not kept any run yet. The first one runs at the next 02:40 UTC; the data as it is now is shown above.")).toBeInTheDocument();
  });

  it("says why the runs could not be read", async () => {
    window.history.replaceState(null, "", "/?lang=en");
    show(undefined, runInspector, fakeServer([], { runs: "Alert: the gateway could not answer right now; try again shortly (502)" }));
    expect(await screen.findByText("The runs could not be read: Alert: the gateway could not answer right now; try again shortly (502)")).toBeInTheDocument();
  });
});

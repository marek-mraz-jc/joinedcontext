/**
 * T-3827: the roles. A reader keeps places with alert levels, alone; a steward marks a station's
 * readings faulty, which everyone sees and the comparison leaves out; the history shows a viewer
 * their own changes and a steward everyone's. Signed out, the page says how to get them.
 */
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { helsinkiMs, helsinkiWall, withoutFaults } from "./faults";
import { AIR_STATIONS, NOW, WEATHER_STATIONS } from "./fixtures/stations";
import { compareHref, standing } from "./pages/Places";
import { ServerProblem } from "./server";
import type { Place } from "./server";
import { server, STEWARD, VIEWER } from "./testing/server";
import { sweep } from "./testing/sweep";

// A sweep presses every control: a slow CI runner takes several times a laptop's 4 s.
const SWEEP = 30_000;

vi.mock("maplibre-gl", () => import("./testing/maplibre"));
vi.mock("echarts/core", () => ({ use: vi.fn(), init: vi.fn(() => ({ setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn(), on: vi.fn() })) }));

const HOUR = 3_600_000;
const READ = {
  permissions: [
    { resource: { type: "AirQualityObserved" }, actions: ["queryEntity", "retrieveEntity", "retrieveTemporal"], attributes: "*" as const },
    { resource: { type: "WeatherObserved" }, actions: ["queryEntity", "retrieveEntity", "retrieveTemporal"], attributes: "*" as const },
  ],
  prohibitions: [],
};

function show(language = "en") {
  const client = stubClient({ entities: [...AIR_STATIONS, ...WEATHER_STATIONS], access: READ }, { appName: "air-weather-explorer", language, space: "helsinki" });
  return render(
    <JcProvider client={client}>
      <App server={server} />
    </JcProvider>,
  );
}
const open = async (page: string) => fireEvent.click(await screen.findByRole("button", { name: page }));

beforeEach(() => {
  server.reset();
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date(NOW));
  window.history.replaceState(null, "", "/");
});
afterEach(() => {
  vi.useRealTimers();
  window.history.replaceState(null, "", "/");
});

describe("Helsinki's wall clock and the faulty hours", () => {
  it("reads a datetime-local value in Helsinki, summer and winter, and refuses one naming no time", () => {
    expect(helsinkiMs("2030-10-20T09:00")).toBe(NOW);
    expect(helsinkiMs("2030-07-01T12:00")).toBe(Date.UTC(2030, 6, 1, 9));
    expect(helsinkiMs("2030-01-15T12:00")).toBe(Date.UTC(2030, 0, 15, 10));
    expect(helsinkiMs("")).toBeNull();
    expect(helsinkiMs("2030-13-40T25:00")).toBeNull();
    expect(helsinkiMs("tomorrow")).toBeNull();
    expect(helsinkiWall(NOW)).toBe("2030-10-20T09:00");
    expect(helsinkiWall(Date.UTC(2030, 0, 15, 10))).toBe("2030-01-15T12:00");
  });

  it("leaves out every hour a mark covers, the end not included, and counts each hour once", () => {
    const hourly = { pm25: [[NOW - 3 * HOUR, 1], [NOW - 2 * HOUR, 2], [NOW - HOUR, 3]] as [number, number][], pm10: [[NOW - 2 * HOUR, 5]] as [number, number][] };
    expect(withoutFaults(hourly, [])).toEqual({ hourly, leftOut: 0 });
    const cut = withoutFaults(hourly, [{ id: 1, station: "s", from: NOW - 2 * HOUR, to: NOW - HOUR, note: "x" }]);
    expect(cut).toEqual({ hourly: { pm25: [[NOW - 3 * HOUR, 1], [NOW - HOUR, 3]], pm10: [] }, leftOut: 1 });
  });
});

describe("a place's standing and link", () => {
  const place: Place = { id: 1, station: AIR_STATIONS[0].id, name: "Koti", attr: "pm10", threshold: 20, latest: 25.5, latestHour: NOW - HOUR };
  it("says above, below or not read yet, and links to its station and pollutant", () => {
    expect(standing("en", place)).toEqual({ text: "Above the level: 25,5 (level 20,0)", above: true });
    expect(standing("en", { ...place, latest: 12 })).toEqual({ text: "Below the level: 12,0 (level 20,0)", above: false });
    expect(standing("en", { ...place, latest: null }).text).toMatch(/^No hour read yet/);
    expect(compareHref(place, "sv")).toBe("?station=kallio&air=pm10&lang=sv#compare");
  });
});

describe("signed out", () => {
  it("is what the page assumes when the server cannot say who calls", async () => {
    vi.mocked(server.me).mockRejectedValueOnce(new ServerProblem(502, ""));
    show();
    expect(await screen.findByLabelText("Your role: Not signed in")).toBeInTheDocument();
  });


  it("shows that in the header, and says to sign in for places and the history", async () => {
    show();
    expect(await screen.findByLabelText("Your role: Not signed in")).toBeInTheDocument();
    await open("My places");
    expect(await screen.findByText("Sign in to keep places of your own.")).toBeInTheDocument();
    await open("History");
    expect(await screen.findByText("Sign in to see the history.")).toBeInTheDocument();
    expect(server.places).not.toHaveBeenCalled();
    expect(server.history).not.toHaveBeenCalled();
  });
});

describe("a viewer", () => {
  beforeEach(() => {
    server.who = VIEWER;
  });

  it("adds a place, changes it, deletes it, and finds each change in their history", async () => {
    show();
    expect(await screen.findByLabelText("Your role: Viewer")).toBeInTheDocument();
    await open("My places");
    expect(await screen.findByText("You have no place yet.")).toBeInTheDocument();
    const add = screen.getByRole("form", { name: "Add the place" });
    await waitFor(() => expect(within(add).getByRole("textbox", { name: "Name" })).toHaveValue("Kallio 2"));
    fireEvent.change(within(add).getByRole("combobox", { name: "Pollutant" }), { target: { value: "pm10" } });
    fireEvent.change(within(add).getByRole("textbox", { name: "Level" }), { target: { value: "" } });
    expect(within(add).getByRole("button", { name: "Add the place" })).toBeDisabled();
    fireEvent.change(within(add).getByRole("textbox", { name: "Level" }), { target: { value: "20,5" } });
    fireEvent.click(within(add).getByRole("button", { name: "Add the place" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Place added: Kallio 2.");
    expect(server.addPlace).toHaveBeenCalledWith({ station: AIR_STATIONS[0].id, name: "Kallio 2", attr: "pm10", threshold: 20.5 });
    expect(screen.getByText("No hour read yet (level 20,5). Open the station in the comparison.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open in the comparison: Kallio 2" })).toHaveAttribute("href", "?station=kallio&air=pm10&lang=en#compare");

    fireEvent.click(screen.getByRole("button", { name: "Change: Kallio 2" }));
    const change = screen.getByRole("form", { name: "Change: Kallio 2" });
    fireEvent.change(within(change).getByRole("textbox", { name: "Name" }), { target: { value: "Koti" } });
    fireEvent.change(within(change).getByRole("textbox", { name: "Level" }), { target: { value: "12" } });
    fireEvent.click(within(change).getByRole("button", { name: "Save the change" }));
    expect(screen.getByText("Koti")).toBeInTheDocument();
    expect(await screen.findByRole("status")).toHaveTextContent("The change is saved: Koti.");
    expect(server.changePlace).toHaveBeenCalledWith(1, "Koti", 12);

    fireEvent.click(screen.getByRole("button", { name: "Delete: Koti" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete: Koti" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete for good" }));
    expect(await screen.findByText("You have no place yet.")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Deleted: Koti.");

    await open("History");
    expect(await screen.findByText("Your own changes; a steward sees everyone's.")).toBeInTheDocument();
    expect(await screen.findByText(/kept the place Kallio 2/)).toBeInTheDocument();
    expect(within(screen.getByRole("table")).getAllByText("Vilja Viewer")).toHaveLength(3);
  });

  it("gets a change back when the server refuses it, with the reason in words", async () => {
    server.kept.push({ id: 7, station: AIR_STATIONS[0].id, name: "Koti", attr: "pm25", threshold: 10, latest: 15, latestHour: NOW - HOUR, owner: VIEWER.id });
    show();
    await open("My places");
    expect(await screen.findByText("Above the level: 15,0 (level 10,0)")).toBeInTheDocument();
    vi.mocked(server.removePlace).mockRejectedValueOnce(new ServerProblem(404, "no such record, or not yours to change"));
    fireEvent.click(screen.getByRole("button", { name: "Delete: Koti" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete for good" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("It is no longer there, or it is not yours to change."));
    expect(screen.getByText("Koti")).toBeInTheDocument();

    vi.mocked(server.changePlace).mockRejectedValueOnce(new ServerProblem(0, ""));
    fireEvent.click(screen.getByRole("button", { name: "Change: Koti" }));
    const change = screen.getByRole("form", { name: "Change: Koti" });
    fireEvent.change(within(change).getByRole("textbox", { name: "Name" }), { target: { value: "Elsewhere" } });
    fireEvent.click(within(change).getByRole("button", { name: "Save the change" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent(/could not be reached/));
    expect(screen.getByText("Koti")).toBeInTheDocument();
  });

  it("says so when their places or the history cannot be read", async () => {
    vi.mocked(server.places).mockRejectedValueOnce(new ServerProblem(502, ""));
    vi.mocked(server.history).mockRejectedValueOnce(new ServerProblem(502, ""));
    show();
    await open("My places");
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("The server did not answer. Try again in a moment."));
    await open("History");
    expect(await screen.findByText("The server did not answer. Try again in a moment.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(await screen.findByText("No change yet.")).toBeInTheDocument();
  });

  it("is told when the marks cannot be read, and the comparison keeps every hour", async () => {
    vi.mocked(server.faults).mockRejectedValueOnce(new ServerProblem(502, ""));
    show();
    await waitFor(() => expect(screen.getAllByRole("status").some((s) => s.textContent === "The server did not answer. Try again in a moment.")).toBe(true));
    expect(screen.queryByText("No reading of this station is marked faulty.")).toBeNull();
  });

  it("sees the marks but cannot make one, and is told who can", async () => {
    server.marks.push({ id: 1, station: AIR_STATIONS[0].id, from: NOW - 5 * HOUR, to: NOW - 2 * HOUR, note: "sensor cleaned" });
    show();
    const list = await screen.findByRole("list", { name: "Readings marked faulty" });
    expect(within(list).getByText(/sensor cleaned/)).toBeInTheDocument();
    expect(within(list).queryByRole("button")).toBeNull();
    expect(screen.getByText("Only a steward marks readings faulty.")).toBeInTheDocument();
    expect(screen.queryByRole("form", { name: "Mark faulty" })).toBeNull();
    expect(await screen.findByText("3 hours left out as faulty.")).toBeInTheDocument();
  });
});

describe("a steward", () => {
  beforeEach(() => {
    server.who = STEWARD;
  });

  it("marks a period faulty, which the comparison leaves out, takes it back, and sees everyone's history", async () => {
    show();
    expect(await screen.findByLabelText("Your role: Steward")).toBeInTheDocument();
    expect(await screen.findByText("No reading of this station is marked faulty.")).toBeInTheDocument();
    const form = screen.getByRole("form", { name: "Mark faulty" });
    expect(within(form).getByLabelText("From")).toHaveValue("2030-10-20T03:00");
    expect(within(form).getByLabelText("To")).toHaveValue("2030-10-20T09:00");
    expect(within(form).getByRole("button", { name: "Mark faulty" })).toBeDisabled();
    fireEvent.change(within(form).getByLabelText("Reason"), { target: { value: "  sensor cleaned " } });
    fireEvent.change(within(form).getByLabelText("From"), { target: { value: "2030-10-20T10:00" } });
    expect(within(form).getByRole("button", { name: "Mark faulty" })).toBeDisabled();
    fireEvent.change(within(form).getByLabelText("From"), { target: { value: "2030-10-20T06:00" } });
    fireEvent.click(within(form).getByRole("button", { name: "Mark faulty" }));
    await waitFor(() => expect(screen.getAllByRole("status").some((s) => s.textContent === "Marked faulty.")).toBe(true));
    expect(server.markFault).toHaveBeenCalledWith({ station: AIR_STATIONS[0].id, from: NOW - 3 * HOUR, to: NOW, note: "sensor cleaned" });
    expect(await screen.findByText("3 hours left out as faulty.")).toBeInTheDocument();
    expect(within(form).getByLabelText("Reason")).toHaveValue("");

    vi.mocked(server.unmarkFault).mockRejectedValueOnce(new ServerProblem(403, ""));
    const unmark = screen.getByRole("button", { name: /^Take the mark back: / });
    fireEvent.click(unmark);
    await waitFor(() => expect(screen.getAllByRole("status").some((s) => s.textContent === "Your account may not do this.")).toBe(true));
    expect(screen.getByRole("list", { name: "Readings marked faulty" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /^Take the mark back: / }));
    expect(await screen.findByText("No reading of this station is marked faulty.")).toBeInTheDocument();
    expect(screen.queryByText(/hours left out as faulty/)).toBeNull();

    server.log.push({ id: 99, at: new Date(NOW - HOUR).toISOString(), actorName: "Vilja Viewer", action: "insert", target: "places/1", summary: "kept the place Koti", owner: VIEWER.id });
    await open("History");
    expect(await screen.findByText("Everyone's changes, newest first.")).toBeInTheDocument();
    expect(await screen.findByText(/kept the place Koti/)).toBeInTheDocument();
    expect(screen.getByText(/took back a faulty mark/)).toBeInTheDocument();
  });

  it("is told in words when marking is refused", async () => {
    vi.mocked(server.markFault).mockRejectedValueOnce(new ServerProblem(409, "a mark is at most 31 days long"));
    show();
    const form = await screen.findByRole("form", { name: "Mark faulty" });
    fireEvent.change(within(form).getByLabelText("Reason"), { target: { value: "x" } });
    fireEvent.click(within(form).getByRole("button", { name: "Mark faulty" }));
    await waitFor(() => expect(screen.getAllByRole("status").some((s) => s.textContent?.includes("a mark is at most 31 days long"))).toBe(true));
  });

  for (const [language, pages, variable] of [
    ["en", ["My places", "History", "Compare"], "Weather variable"],
    ["fi", ["Omat paikat", "Historia", "Vertailu"], "Säämuuttuja"],
    ["sv", ["Mina platser", "Historik", "Jämförelse"], "Vädervariabel"],
  ] as const) {
    // At each station, each with its analysis read first (Vartiokylä has no history to wait for).
    for (const address of ["/", "/?station=makelankatu&weather=road-1002", "/?station=vartiokyla&weather=fmi-100968"]) {
      it(`uses every control of every page once, in ${language}, from ${address}`, async () => {
        window.history.replaceState(null, "", address);
        server.kept.push({ id: 7, station: AIR_STATIONS[0].id, name: "Koti", attr: "pm25", threshold: 10, latest: 5, latestHour: NOW - HOUR, owner: STEWARD.id });
        server.marks.push({ id: 1, station: AIR_STATIONS[0].id, from: NOW - 5 * HOUR, to: NOW - 2 * HOUR, note: "sensor cleaned" });
        const { container } = show(language);
        if (!address.includes("vartiokyla")) await waitFor(() => expect(screen.getByRole("combobox", { name: variable })).toBeEnabled());
        else await waitFor(() => expect(server.faults).toHaveBeenCalled());
        await act(async () => sweep(container));
        for (const page of pages) {
          await open(page);
          await act(async () => new Promise((done) => setTimeout(done, 0)));
          await act(async () => sweep(container));
        }
      }, SWEEP);
    }
  }
});

describe("in Swedish", () => {
  it("speaks Swedish in the header and on the pages", async () => {
    server.who = VIEWER;
    show("sv");
    expect(await screen.findByLabelText("Din roll: Läsare")).toBeInTheDocument();
    await open("Mina platser");
    expect(await screen.findByText("Du har inga platser ännu.")).toBeInTheDocument();
  });
});

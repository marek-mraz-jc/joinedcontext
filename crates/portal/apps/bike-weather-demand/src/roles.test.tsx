/**
 * T-3827: who reads, and what a steward changes. Everyone reads a station's model inputs; a
 * steward sets them, saved at once on screen and taken back when the server refuses; the page then
 * reads the weather station they chose over the days they chose; History shows the changes.
 */
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { checkedDraft } from "./components/ModelInputs";
import { EstimaterContext } from "./estimate";
import type { Estimater } from "./estimate";
import { ENTITIES, NOW, STATION_1, STATION_2, TEMPORAL, WEATHER_1, WEATHER_2 } from "./fixtures/bikes";
import { Problem, ServerContext } from "./server";
import { memory, STEWARD, VIEWER } from "./testing/server";
import type { Memory } from "./testing/server";
import { sweep } from "./testing/sweep";

// A sweep presses every control: a slow CI runner takes several times a laptop's 4 s.
const SWEEP = 30_000;

vi.mock("maplibre-gl", () => ({
  Map: class {
    on = vi.fn();
    once = vi.fn();
    remove = vi.fn();
  },
  setWorkerUrl: vi.fn(),
}));
vi.mock("echarts", () => ({ init: vi.fn(() => ({ setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn(), on: vi.fn() })) }));

const ACCESS = {
  permissions: [
    { resource: { type: "BikeHireDockingStation" }, actions: ["queryEntity", "retrieveEntity", "retrieveTemporal"], attributes: "*" as const },
    { resource: { type: "WeatherObserved" }, actions: ["queryEntity", "retrieveEntity", "retrieveTemporal"], attributes: "*" as const },
  ],
  prohibitions: [],
};
const ESTIMATE: Estimater = async () => ({ hours: 30, profile: [], weather: null, sigma: 1, estimate: [], assumption: null, enough: true });

let server: Memory;
let asked: { type: string; id?: string[]; timeAt?: string }[];

function show(language = "en") {
  window.history.replaceState(null, "", `/?lang=${language}`);
  const client = stubClient({ entities: ENTITIES, temporal: TEMPORAL, access: ACCESS }, { appName: "bike-weather-demand" });
  const list = client.temporal.list.bind(client.temporal);
  client.temporal.list = vi.fn(async (type, query) => {
    asked.push({ type, id: query.id, timeAt: query.timeAt });
    return list(type, query);
  });
  return render(
    <JcProvider client={client}>
      <ServerContext.Provider value={server}>
        <EstimaterContext.Provider value={ESTIMATE}>
          <App />
        </EstimaterContext.Provider>
      </ServerContext.Provider>
    </JcProvider>,
  );
}

const open = async (page: string) => fireEvent.click(await screen.findByRole("button", { name: page }));
const form = async () => screen.findByRole("form", { name: "Model inputs" });
const field = (name: string) => within(screen.getByRole("form", { name: "Model inputs" })).getByLabelText(name);

beforeEach(() => {
  server = memory();
  asked = [];
  vi.setSystemTime(new Date(NOW * 1000));
  window.location.hash = "";
});
afterEach(() => {
  vi.useRealTimers();
  window.history.replaceState(null, "", "/");
  window.location.hash = "";
});

describe("a signed-out reader", () => {
  it("reads the defaults, every control closed with the reason, and History asks them to sign in", async () => {
    show();
    expect(await screen.findByLabelText("Role: Signed out")).toBeInTheDocument();
    await form();
    expect(screen.getByTestId("inputs-now")).toHaveTextContent("The model reads the weather of the nearest within 10 km and trains on 7 days. The defaults.");
    for (const name of ["Weather station", "Radius (km, 1–25)", "Days to train on (1–7)", "Note"]) {
      expect(field(name)).toBeDisabled();
      expect(field(name)).toHaveAttribute("title", "Sign in as a steward to change the model inputs.");
    }
    expect(screen.getByRole("button", { name: "Save inputs" })).toBeDisabled();
    await open("History");
    expect(await screen.findByText("Sign in to see the changes.")).toBeInTheDocument();
    expect(server.history).not.toHaveBeenCalled();
  });
});

describe("a viewer", () => {
  it("reads a steward's inputs and their own history, every control closed with the reason", async () => {
    server.who = VIEWER;
    server.kept.set(STATION_1.id, { station: STATION_1.id, weatherStation: WEATHER_2.id, radiusKm: 5, days: 3, note: "Kaisaniemi is under repair", custom: true, updatedBy: "Sami Steward", updatedAt: "2026-10-15T06:00:00Z" });
    show();
    expect(await screen.findByLabelText("Role: Viewer")).toBeInTheDocument();
    expect(screen.getByText("Vilja Viewer")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByTestId("inputs-now")).toHaveTextContent(`The model reads the weather of ${WEATHER_2.name} and trains on 3 days. Set by Sami Steward,`));
    expect(screen.getByText("Kaisaniemi is under repair")).toBeInTheDocument();
    // The page reads with them too: the chosen weather station, over three days.
    expect(await screen.findByText(new RegExp(`Chosen weather station: ${WEATHER_2.name} \\(20(\\.0)? km\\)`))).toBeInTheDocument();
    await waitFor(() => expect(asked.some((a) => a.type === "WeatherObserved" && a.id?.[0] === WEATHER_2.id)).toBe(true));
    expect(asked.every((a) => a.timeAt === new Date((NOW - 3 * 86_400) * 1000).toISOString())).toBe(true);
    expect(field("Days to train on (1–7)")).toHaveAttribute("title", "Only a steward changes the model inputs.");
    expect(screen.getByRole("button", { name: "Back to the defaults" })).toBeDisabled();
    await open("History");
    expect(await screen.findByText("Your own changes, newest first.")).toBeInTheDocument();
    expect(await screen.findByText("No changes yet.")).toBeInTheDocument();
  });

  it("says when the inputs cannot be read, and the page reads with the defaults", async () => {
    server.inputs = vi.fn(async () => {
      throw new Problem(503, "the database gave no answer");
    });
    show();
    expect(await screen.findByText("The model inputs could not be read: the database gave no answer")).toBeInTheDocument();
    expect(await screen.findByText(/Nearest weather station: Kaisaniemi/)).toBeInTheDocument();
  });
});

describe("a steward", () => {
  beforeEach(() => {
    server.who = STEWARD;
  });

  it("sets the inputs, at once on screen, the models read again; History shows it; then back to the defaults", async () => {
    show();
    expect(await screen.findByLabelText("Role: Steward")).toBeInTheDocument();
    await form();
    await waitFor(() => expect(field("Days to train on (1–7)")).toBeEnabled());
    expect(screen.getByRole("button", { name: "Back to the defaults" })).toHaveAttribute("title", "The station has the defaults already.");
    const models = vi.mocked(server.models).mock.calls.length;
    fireEvent.change(field("Weather station"), { target: { value: WEATHER_1.id } });
    fireEvent.change(field("Radius (km, 1–25)"), { target: { value: "2,5" } });
    fireEvent.change(field("Days to train on (1–7)"), { target: { value: "3" } });
    fireEvent.change(field("Note"), { target: { value: "  a closer station  " } });
    fireEvent.click(screen.getByRole("button", { name: "Save inputs" }));
    expect(screen.getByTestId("inputs-now")).toHaveTextContent("Set by Sami Steward,");
    expect(await screen.findByText("Saved: today's model is trained again.")).toBeInTheDocument();
    expect(server.setInputs).toHaveBeenCalledWith(STATION_1.id, { weatherStation: WEATHER_1.id, radiusKm: 2.5, days: 3, note: "a closer station" });
    await waitFor(() => expect(vi.mocked(server.models).mock.calls.length).toBeGreaterThan(models));
    expect(screen.getByText("a closer station")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Back to the defaults" }));
    expect(await screen.findByText("Back to the defaults: today's model is trained again.")).toBeInTheDocument();
    expect(screen.getByTestId("inputs-now")).toHaveTextContent("The defaults.");
    expect(server.kept.size).toBe(0);

    await open("History");
    expect(await screen.findByText("Everyone's changes, newest first.")).toBeInTheDocument();
    const rows = within(await screen.findByRole("table", { name: "Changes" })).getAllByRole("row");
    expect(rows.map((row) => row.textContent)).toEqual([
      "WhenWhoWhat",
      expect.stringMatching(/Sami StewardRemoved: back to the defaults$/),
      expect.stringMatching(/Sami StewardChanged: 3 days$/),
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await waitFor(() => expect(server.history).toHaveBeenCalledTimes(2));
  });

  it("takes a refused save back and says why; a draft out of range cannot be saved", async () => {
    show();
    await form();
    await waitFor(() => expect(field("Days to train on (1–7)")).toBeEnabled());
    fireEvent.change(field("Days to train on (1–7)"), { target: { value: "9" } });
    expect(screen.getByRole("alert")).toHaveTextContent("A model trains on 1 to 7 days.");
    expect(field("Days to train on (1–7)")).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("button", { name: "Save inputs" })).toBeDisabled();
    fireEvent.submit(await form());
    expect(server.setInputs).not.toHaveBeenCalled();
    fireEvent.change(field("Days to train on (1–7)"), { target: { value: "2" } });
    fireEvent.change(field("Radius (km, 1–25)"), { target: { value: "30" } });
    expect(screen.getByRole("alert")).toHaveTextContent("The radius is 1 to 25 km.");
    fireEvent.change(field("Radius (km, 1–25)"), { target: { value: "4" } });

    // The role was taken away meanwhile: the server refuses, and the page shows what is kept.
    server.who = VIEWER;
    fireEvent.click(screen.getByRole("button", { name: "Save inputs" }));
    expect(await screen.findByText("Not saved: this needs the role steward")).toBeInTheDocument();
    expect(screen.getByTestId("inputs-now")).toHaveTextContent("trains on 7 days. The defaults.");
    // What they typed stays in the form, to try again.
    expect(field("Days to train on (1–7)")).toHaveValue("2");
    expect(server.kept.size).toBe(0);
  });

  for (const [language, pages] of [
    ["en", ["History", "Bikes and the weather"]],
    ["fi", ["Muutokset", "Pyörät ja sää"]],
    ["sv", ["Historik", "Cyklar och vädret"]],
  ] as const) {
    it(`uses every control of every page once, in ${language}`, async () => {
      server.kept.set(STATION_2.id, { station: STATION_2.id, weatherStation: WEATHER_2.id, radiusKm: 5, days: 3, note: "", custom: true, updatedBy: "Sami Steward", updatedAt: "2026-10-15T06:00:00Z" });
      const { container } = show(language);
      await waitFor(() => expect(server.inputs).toHaveBeenCalled());
      await waitFor(() => expect(container.querySelector("form select")).toBeEnabled());
      const main = () => container.querySelector<HTMLElement>("main") ?? container;
      await sweep(main());
      for (const page of pages) {
        await open(page);
        await act(async () => new Promise((done) => setTimeout(done, 0)));
        await sweep(main());
      }
      // Each station once, with its own weather station's Details.
      const stations = container.querySelector<HTMLSelectElement>("#station-select")!;
      for (const value of [...stations.options].map((o) => o.value)) {
        await act(async () => {
          fireEvent.change(container.querySelector("#station-select")!, { target: { value } });
        });
        await act(async () => new Promise((done) => setTimeout(done, 0)));
        await sweep(main());
      }
    }, SWEEP);
  }
});

describe("a draft", () => {
  it("is checked as the server checks it", () => {
    const draft = { weatherStation: "", radiusKm: "10", days: "7", note: "" };
    expect(checkedDraft(draft)).toEqual({ weatherStation: null, radiusKm: 10, days: 7, note: "" });
    expect(checkedDraft({ ...draft, radiusKm: "" })).toBe("radiusRange");
    expect(checkedDraft({ ...draft, radiusKm: "0.5" })).toBe("radiusRange");
    expect(checkedDraft({ ...draft, days: "2.5" })).toBe("daysRange");
    expect(checkedDraft({ ...draft, days: "0" })).toBe("daysRange");
    expect(checkedDraft({ ...draft, note: "x".repeat(201) })).toBe("noteLong");
  });
});

describe("in Swedish", () => {
  it("speaks Swedish in the header and on the page", async () => {
    server.who = VIEWER;
    show("sv");
    expect(await screen.findByLabelText("Roll: Läsare")).toBeInTheDocument();
    expect(await screen.findByRole("form", { name: "Modellens indata" })).toBeInTheDocument();
    expect(await screen.findByText("Bara en förvaltare ändrar modellens indata.")).toBeInTheDocument();
  });
});

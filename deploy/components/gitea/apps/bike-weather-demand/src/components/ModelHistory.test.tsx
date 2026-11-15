import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ModelHistory } from "./ModelHistory";
import { Problem } from "../server";
import { memory } from "../testing/server";
import type { Model, Models, Server } from "../server";

const STATION = "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:001";

const model = (id: number, day: string, per: number | null, rain: number | null, sigma: number | null): Model => ({
  id,
  trained_on: day,
  trained_at: `${day}T06:00:00Z`,
  hours: 160,
  enough: true,
  per_degree: per,
  rain,
  sigma,
  weather_station: per === null ? null : "urn:ngsi-ld:WeatherObserved:w1",
});

function serverOf(kept: Models | Error, snapshot: string | Error = "https://store.test/training/2030-10-21/ab.json"): Server & { asked: string[] } {
  const asked: string[] = [];
  return {
    ...memory(),
    asked,
    models: async (station) => {
      asked.push(station);
      if (kept instanceof Error) throw kept;
      return kept;
    },
    snapshotUrl: async () => {
      if (snapshot instanceof Error) throw snapshot;
      return snapshot;
    },
  };
}

describe("the station's model over time", () => {
  afterEach(() => vi.restoreAllMocks());

  it("lists the kept models, the newest first, says they are as last kept, and opens a day's training data", async () => {
    const server = serverOf({ models: [{ ...model(2, "2030-10-21", 0.42, -1.5, 2.25), days: 3 }, model(1, "2030-10-20", null, null, null)], stale: true });
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    const { rerender } = render(<ModelHistory lang="en" server={server} station={STATION} />);
    const table = await screen.findByRole("table", { name: "The station's model over time: kept by the server" });
    expect(within(table).getAllByRole("row").map((row) => row.textContent)).toEqual([
      "Trained onDaysHoursBikes per °CRainSpread (σ)Training data",
      "21 Oct 20303160+0.42-1.52.25Training data",
      "20 Oct 20307160–––Training data",
    ]);
    expect(server.asked).toEqual([STATION]);
    // A steward's change of the inputs reads the models again.
    rerender(<ModelHistory lang="en" server={server} station={STATION} version={1} />);
    await waitFor(() => expect(server.asked).toEqual([STATION, STATION]));
    expect(screen.getByText("The data could not be read just now: these are the models as kept.")).toBeInTheDocument();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Download the training data of 21 Oct 2030" }));
    await user.click(screen.getByRole("button", { name: "Download the training data of 20 Oct 2030" }));
    await waitFor(() => expect(open).toHaveBeenCalledTimes(2));
    expect(open).toHaveBeenCalledWith("https://store.test/training/2030-10-21/ab.json", "_blank", "noopener");
  });

  it("says in Finnish when a day's training data cannot be downloaded", async () => {
    render(<ModelHistory lang="fi" server={serverOf({ models: [model(2, "2030-10-21", 0.4, 0, 1)], stale: false }, new Problem(404, "no such model"))} station={STATION} />);
    await userEvent.setup().click(await screen.findByRole("button", { name: "Lataa 21.10.2030 opetusdata" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Opetusdataa ei voitu ladata: no such model");
  });

  it("says when nothing is kept yet, and when the models cannot be read", async () => {
    const { unmount } = render(<ModelHistory lang="en" server={serverOf({ models: [], stale: false })} station={STATION} />);
    expect(screen.getByText("Reading the kept models…")).toBeInTheDocument();
    expect(await screen.findByText("The server has not kept a model of this station yet.")).toBeInTheDocument();
    unmount();
    render(<ModelHistory lang="en" server={serverOf(new Problem(404, "the Endpoint holds no such station"))} station={STATION} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("The models could not be read: the Endpoint holds no such station");
  });
});

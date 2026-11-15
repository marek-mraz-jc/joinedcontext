import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ServerProblem } from "../server";
import type { ReachApi } from "../server";
import { KeptReach, problemText } from "./KeptReach";

const went: string[] = [];
vi.mock("../go", () => ({ go: (url: string) => went.push(url) }));

const STOP = { id: "urn:ngsi-ld:GtfsStop:HSL:1", label: "Rautatientori (H0019): lines M1" };

function server(): Pick<ReachApi, "reach" | "refresh"> {
  return {
    reach: vi.fn(async () => ({
      version: "v1",
      stop: STOP.id,
      bands: [
        { minutes: 10, areaKm2: 1.25, stops: 3 },
        { minutes: 20, areaKm2: 4.5, stops: 9 },
        { minutes: 30, areaKm2: 9.75, stops: 20 },
      ],
      cached: true,
      stale: false,
      url: "https://store.example/apps/s1/x/tiles/v1/1.geojson?X-Amz-Signature=x",
    })),
    refresh: vi.fn(async () => ({ version: "v1", stops: 3, routes: 2, changed: false })),
  };
}

beforeEach(() => {
  went.length = 0;
});

// T-3349: from a stop, the areas as the server keeps them, and their GeoJSON.
describe("KeptReach", () => {
  it("asks for a start stop first, then gives the kept areas and downloads their GeoJSON", async () => {
    const api = server();
    const { rerender } = render(<KeptReach lang="en" api={api} stop={null} />);
    expect(screen.getByRole("button", { name: "Download the areas from the start stop (GeoJSON)" })).toBeDisabled();
    expect(screen.getByText(/Start from a stop/)).toBeInTheDocument();
    rerender(<KeptReach lang="en" api={api} stop={STOP} />);
    expect(screen.getByText("Start stop: Rautatientori (H0019): lines M1.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Download the areas from the start stop (GeoJSON)" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("The GeoJSON file is downloading."));
    expect(api.reach).toHaveBeenCalledWith(STOP.id);
    expect(went).toEqual(["https://store.example/apps/s1/x/tiles/v1/1.geojson?X-Amz-Signature=x"]);
    expect(screen.getByText("Areas from Rautatientori (H0019): lines M1 kept on the server: 10 min 1.3 km², 20 min 4.5 km², 30 min 9.8 km².")).toBeInTheDocument();
  });

  it("says in the visitor's language why the server refused", async () => {
    const api = server();
    api.reach = vi.fn().mockRejectedValue(new ServerProblem(404, "HSL's network holds no such stop with a place"));
    render(<KeptReach lang="fi" api={api} stop={STOP} />);
    fireEvent.click(screen.getByRole("button", { name: "Lataa alueet lähtöpysäkiltä (GeoJSON)" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("HSL:n verkossa ei ole tätä pysäkkiä sijainteineen."));
    expect(went).toEqual([]);
  });

  it("maps every refusal to words a person can act on", () => {
    expect(problemText("en", new ServerProblem(0, ""))).toMatch(/could not be reached/);
    expect(problemText("en", new ServerProblem(401, ""))).toMatch(/Sign in again/);
    expect(problemText("en", new ServerProblem(403, ""))).toMatch(/may not do this/);
    expect(problemText("en", new ServerProblem(409, ""))).toMatch(/holds no HSL stops/);
    expect(problemText("en", new ServerProblem(507, ""))).toMatch(/storage is full/);
    expect(problemText("en", new ServerProblem(422, "no band to reach within"))).toBe("No areas: no band to reach within");
    expect(problemText("en", new ServerProblem(502, ""))).toMatch(/did not answer/);
    expect(problemText("en", "odd")).toMatch(/did not answer/);
  });
});

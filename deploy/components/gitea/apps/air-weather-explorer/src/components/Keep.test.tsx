import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ServerProblem } from "../server";
import { server } from "../testing/server";
import { Keep, problemText } from "./Keep";

const went: string[] = [];
vi.mock("../go", () => ({ go: (url: string) => went.push(url) }));

const COMPARISON = { station: "urn:ngsi-ld:AirQualityObserved:x:kallio", weather: "urn:ngsi-ld:WeatherObserved:x:kaisaniemi", days: 7, smoothing: 6, air: "pm25" as const, variable: "windSpeed" as const };

beforeEach(() => {
  server.reset();
  went.length = 0;
});
afterEach(() => vi.unstubAllGlobals());

// T-3348: the comparison on screen kept under a link, and its hours given as CSV.
describe("Keep", () => {
  it("waits for both stations, then saves the comparison with its name and shows the link", async () => {
    const { rerender } = render(<Keep lang="en" api={server} comparison={null} />);
    expect(screen.getByRole("button", { name: "Save the comparison" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Export the hours (CSV)" })).toBeDisabled();
    rerender(<Keep lang="en" api={server} comparison={COMPARISON} />);
    fireEvent.change(screen.getByLabelText("Name of the comparison (optional)"), { target: { value: "  Kallio and the wind " } });
    fireEvent.click(screen.getByRole("button", { name: "Save the comparison" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Comparison saved."));
    expect(server.save).toHaveBeenCalledWith({ ...COMPARISON, name: "Kallio and the wind" });
    const link = screen.getByLabelText("Link to the comparison") as HTMLInputElement;
    expect(link.value).toContain("?compare=c00000000001");
    fireEvent.click(link);
    fireEvent.focus(link);
    expect(link.selectionEnd).toBe(link.value.length);

    const writeText = vi.fn(async () => undefined);
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    fireEvent.click(screen.getByRole("button", { name: "Copy link" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Link copied."));
    vi.stubGlobal("navigator", { clipboard: { writeText: vi.fn().mockRejectedValue(new Error("denied")) } });
    fireEvent.click(screen.getByRole("button", { name: "Copy link" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("The link is selected: copy it with the keyboard."));
  });

  it("saves an unnamed comparison as one without a name", async () => {
    render(<Keep lang="en" api={server} comparison={COMPARISON} />);
    fireEvent.submit(screen.getByRole("form", { name: "Keep or export the comparison" }));
    await waitFor(() => expect(server.save).toHaveBeenCalledWith({ ...COMPARISON, name: null }));
  });

  it("downloads the hours, and says in the reader's language why the server refused", async () => {
    render(<Keep lang="fi" api={server} comparison={COMPARISON} />);
    fireEvent.change(screen.getByLabelText("Vertailun nimi (valinnainen)"), { target: { value: "Kallio ja tuuli" } });
    fireEvent.click(screen.getByRole("button", { name: "Vie tunnit (CSV)" }));
    await waitFor(() => expect(went).toEqual(["https://store.example/apps/s1/x/exports/c00000000001.csv?X-Amz-Signature=x"]));
    expect(server.exportUrl).toHaveBeenCalledWith(COMPARISON.station, COMPARISON.weather, 7);
    server.save = vi.fn(async () => {
      throw new ServerProblem(400, "a name is one line of at most 80 characters");
    });
    fireEvent.click(screen.getByRole("button", { name: "Tallenna vertailu" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Ei tallennettu: a name is one line of at most 80 characters"));
  });

  it("maps every refusal to words a person can act on", () => {
    expect(problemText("en", new ServerProblem(0, ""), "saveFailed")).toMatch(/could not be reached/);
    expect(problemText("en", new ServerProblem(401, ""), "saveFailed")).toMatch(/Sign in again/);
    expect(problemText("en", new ServerProblem(403, ""), "saveFailed")).toMatch(/may not do this/);
    expect(problemText("en", new ServerProblem(404, ""), "openFailed")).toMatch(/No comparison is saved/);
    expect(problemText("en", new ServerProblem(507, ""), "saveFailed")).toMatch(/storage is full/);
    expect(problemText("en", new ServerProblem(502, ""), "exportFailed")).toMatch(/did not answer/);
    expect(problemText("en", "odd", "exportFailed")).toMatch(/did not answer/);
  });
});

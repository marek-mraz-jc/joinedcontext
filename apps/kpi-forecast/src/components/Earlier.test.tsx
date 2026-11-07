import { render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ServerProblem } from "../server";
import { memoryServer } from "../test-server";
import { Earlier, problemText } from "./Earlier";

const DAY = 86_400_000;
const T = Date.UTC(2030, 9, 10, 9);
const KPI = "urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:helsinki-kpi:bike-network-stations";

// T-3350: the chosen indicator's earlier forecasts that came due, against what was measured.
describe("Earlier", () => {
  it("lists the forecasts that came due against the readings, and how many fell within the band", async () => {
    const api = memoryServer();
    api.kept.set(KPI, [
      { madeOn: "2030-10-08", days: 30, step: DAY, latestT: T - 2 * DAY, latestV: 470, direction: "up", points: [
        { t: T - DAY, v: 471, lo: 468, hi: 474 },
        { t: T, v: 472, lo: 469, hi: 475 },
        { t: T + DAY, v: 473, lo: 470, hi: 476 },
      ] },
    ]);
    render(<Earlier lang="en" api={api} kpi={KPI} history={[[T - DAY, 473], [T, 480]]} now={T} />);
    const table = await screen.findByRole("table", { name: "Earlier forecasts against what came" });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent("472 (469–475)");
    expect(rows[0]).toHaveTextContent("480 · outside");
    expect(rows[1]).toHaveTextContent("473 · within");
    expect(screen.getByText("1 of 2 measured fell within the 95 % band.")).toBeInTheDocument();
    expect(api.list).toHaveBeenCalledWith(KPI);
  });

  it("says so when nothing has come due, and in words when the server refuses", async () => {
    const api = memoryServer();
    const { rerender } = render(<Earlier lang="fi" api={api} kpi={KPI} history={[]} now={T} />);
    expect(screen.getByText("Luetaan säilytettyjä ennusteita…")).toBeInTheDocument();
    expect(await screen.findByText(/^Tämän mittarin aiempia ennusteita ei ole vielä erääntynyt\./)).toBeInTheDocument();
    api.list = vi.fn().mockRejectedValue(new ServerProblem(0, ""));
    rerender(<Earlier lang="fi" api={api} kpi={`${KPI}-2`} history={[]} now={T} />);
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("Palvelimeen ei saatu yhteyttä."));
  });

  it("maps every refusal to words a person can act on", () => {
    expect(problemText("en", new ServerProblem(401, ""))).toMatch(/Sign in again/);
    expect(problemText("en", new ServerProblem(403, ""))).toMatch(/may not do this/);
    expect(problemText("en", new ServerProblem(507, ""))).toMatch(/storage is full/);
    expect(problemText("en", new ServerProblem(400, "that month has not begun"))).toBe("The server refused: that month has not begun");
    expect(problemText("en", new ServerProblem(409, "you follow this indicator already"))).toBe("The server refused: you follow this indicator already");
    expect(problemText("en", new ServerProblem(404, ""))).toBe("It is no longer there, or it is not yours to change.");
    expect(problemText("en", new ServerProblem(502, ""))).toMatch(/did not answer/);
    expect(problemText("en", "odd")).toMatch(/did not answer/);
  });
});

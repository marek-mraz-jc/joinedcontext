import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { current, lastHour } from "./fixtures";

const NOW = new Date("2026-09-25T12:00:00Z");

function renderMonitor(fixture: Parameters<typeof stubClient>[0] = { entities: current(NOW), temporal: lastHour(NOW) }) {
  const client = stubClient(fixture);
  render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
  return client;
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(NOW);
});
afterEach(() => vi.useRealTimers());

describe("the real-time monitor", () => {
  it("shows each station's value and state in words, a silent one as silent", async () => {
    renderMonitor();
    const busy = await screen.findByRole("article", { name: "Mannerheimintie" });
    expect(within(busy).getByText("31.4 µg/m³")).toBeInTheDocument();
    expect(within(busy).getByText("Over the alert level")).toBeInTheDocument();
    expect(within(screen.getByRole("article", { name: "Tapanila" })).getByText("No reading for 40 min")).toBeInTheDocument();
    expect(within(screen.getByRole("article", { name: "Kallio" })).getByText("Normal")).toBeInTheDocument();
  });

  it("draws the last hour as a sparkline a screen reader hears as its range", async () => {
    renderMonitor();
    const kamppi = await screen.findByRole("article", { name: "Kamppi" });
    await waitFor(() => expect(within(kamppi).getByRole("img")).toHaveAccessibleName(/PM2\.5, 13 readings from .* lowest .* highest/));
  });

  it("says the sparklines start now when the history cannot be read", async () => {
    renderMonitor({
      entities: current(NOW),
      refuse: (request) => (request.path.includes("/temporal/") ? { status: 403, body: { title: "Forbidden", detail: "no temporal read on this endpoint" } } : null),
    });
    expect(await screen.findByText(/The last hour could not be read .*; the sparklines start now\./)).toBeInTheDocument();
    expect(within(screen.getByRole("article", { name: "Kallio" })).getByText("Collecting readings…")).toBeInTheDocument();
  });

  it("announces a station that crosses the level, once, and lists it", async () => {
    const client = renderMonitor();
    await screen.findByRole("article", { name: "Kallio" });
    expect(screen.getByText(/None yet/)).toBeInTheDocument();

    const kallio = client.transport.rows().find((row) => row.stationName === "Kallio")!;
    Object.assign(kallio, { pm25: 38.2, dateObserved: "2026-09-25T12:00:00Z" });
    fireEvent.click(screen.getByRole("button", { name: "Refresh now" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent(/Kallio crossed the alert level: 38\.2 µg\/m³/));

    fireEvent.click(screen.getByRole("button", { name: "Refresh now" }));
    const log = screen.getByRole("article", { name: "Alerts since you opened this page" });
    await waitFor(() => expect(within(log).getAllByRole("listitem")).toHaveLength(1));
    expect(within(log).getByRole("listitem")).toHaveTextContent("Kallio, 38.2 µg/m³");
  });

  it("pauses and resumes the updates", async () => {
    renderMonitor();
    await screen.findByRole("article", { name: "Kallio" });
    fireEvent.click(screen.getByRole("button", { name: "Pause updates" }));
    expect(screen.getByRole("button", { name: "Resume updates" })).toHaveAttribute("aria-pressed", "true");
  });

  it("says so when no station reports", async () => {
    renderMonitor({ entities: [] });
    expect(await screen.findByText("No station reports yet.")).toBeInTheDocument();
  });
});

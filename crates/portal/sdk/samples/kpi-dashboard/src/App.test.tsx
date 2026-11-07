import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App, { axisRange } from "./App";
import { ROWS } from "./fixtures";

// A canvas is not there in jsdom; the numbers the person reads are in the cards, not the chart.
vi.mock("echarts", () => ({ init: vi.fn(() => ({ setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn(), on: vi.fn() })) }));

function renderApp(rows: Row[] = ROWS) {
  const client = stubClient({ entities: rows });
  render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
  return client;
}

describe("the KPI dashboard", () => {
  it("gives every indicator a card with its latest value, target and standing in words", async () => {
    renderApp();
    const cycling = await screen.findByRole("article", { name: "Cycling share of trips" });
    expect(within(cycling).getByText("19.2 %")).toBeInTheDocument();
    expect(within(cycling).getByText("Target 20 %")).toBeInTheDocument();
    expect(within(cycling).getByText("Short by 0.8 %")).toBeInTheDocument();
    expect(within(screen.getByRole("article", { name: "PM2.5 mean" })).getByText("Ahead by 0.6 µg/m³")).toBeInTheDocument();
    expect(within(screen.getByRole("article", { name: "Resident satisfaction" })).getByText("No target set")).toBeInTheDocument();
    expect(screen.getByText("2 of 5 indicators need attention")).toBeInTheDocument();
  });

  it("reads the trend over the period the person picks", async () => {
    renderApp();
    const bus = await screen.findByRole("article", { name: "Bus punctuality" });
    expect(within(bus).getByText("Getting worse")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("combobox", { name: "Period" }), { target: { value: "3" } });
    expect(within(screen.getByRole("article", { name: "Bus punctuality" })).getByText("Getting better")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("combobox", { name: "Period" }), { target: { value: "12" } });
    expect(within(screen.getByRole("article", { name: "Bus punctuality" })).getByText("Level")).toBeInTheDocument();
  });

  it("says so when no indicator has reported", async () => {
    renderApp([]);
    expect(await screen.findByText("No observations in this period.")).toBeInTheDocument();
    expect(screen.getByText("No indicator has reported yet")).toBeInTheDocument();
  });

  it("offers a retry when the endpoint refuses", async () => {
    const client = stubClient({
      entities: ROWS,
      refuse: () => ({ status: 503, body: { title: "Service Unavailable", detail: "the broker is restarting" } }),
    });
    render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );
    expect(await screen.findByRole("button", { name: /retry|try again/i })).toBeInTheDocument();
  });
});

describe("the trend chart's axis", () => {
  it("takes round bounds around the values and the target", () => {
    expect(axisRange([57, 58.2, 55])).toEqual({ min: 55, max: 59, interval: 1 });
    expect(axisRange([17.6, 19.2, 20])).toEqual({ min: 17, max: 20, interval: 1 });
    expect(axisRange([5, 5])).toEqual({ min: 4, max: 6, interval: 2 });
    expect(axisRange([3.6, 3.9])).toEqual({ min: 3.6, max: 3.9, interval: 0.1 });
    expect(axisRange([0])).toEqual({ min: 0, max: 0.25, interval: 0.25 });
    expect(axisRange([null, "x"])).toEqual({ scale: true });
  });
});

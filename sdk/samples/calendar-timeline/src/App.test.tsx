import { fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { events } from "./fixtures";

const TODAY = new Date(2026, 8, 25);
const LONG = new Intl.DateTimeFormat(undefined, { weekday: "long", day: "numeric", month: "long" });
const MONTH = new Intl.DateTimeFormat(undefined, { month: "long", year: "numeric" });

function renderApp(rows: Row[] = events(TODAY)) {
  render(
    <JcProvider client={stubClient({ entities: rows })}>
      <App />
    </JcProvider>,
  );
}

function eventButton(scope: HTMLElement, name: string): HTMLElement {
  return within(scope).getByRole("button", { name: new RegExp(name) });
}

beforeEach(() => {
  // Only the clock is fake: the SDK's own timers and the tests' waits run as they do in a browser.
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date(2026, 8, 25, 12, 0));
});
afterEach(() => vi.useRealTimers());

describe("the calendar and timeline", () => {
  it("opens on this month with today chosen, and lists today's events under it", async () => {
    renderApp();
    const month = await screen.findByRole("table", { name: MONTH.format(TODAY) });
    const today = within(month).getByRole("button", { name: `${LONG.format(TODAY)}, 2 events` });
    expect(today).toHaveAttribute("aria-current", "date");
    expect(today).toHaveAttribute("aria-pressed", "true");
    const day = screen.getByRole("region", { name: LONG.format(TODAY) });
    expect(within(day).getAllByRole("button").map((button) => button.querySelector(".app-event-name")?.textContent)).toEqual([
      "Farmers' market",
      "City council open session",
    ]);

    fireEvent.click(within(month).getByRole("button", { name: `${LONG.format(new Date(2026, 8, 24))}, no events` }));
    expect(screen.getByText("Nothing planned on this day.")).toBeInTheDocument();
  });

  it("steps to the next month and back to today", async () => {
    renderApp();
    await screen.findByRole("table", { name: MONTH.format(TODAY) });
    fireEvent.click(screen.getByRole("button", { name: "Next month" }));
    const october = screen.getByRole("table", { name: MONTH.format(new Date(2026, 9, 1)) });
    expect(within(october).getByRole("button", { name: `${LONG.format(new Date(2026, 9, 1))}, 1 event` })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Today" }));
    expect(screen.getByRole("table", { name: MONTH.format(TODAY) })).toBeInTheDocument();
  });

  it("reads the chosen event beside the calendar, and Close gives focus back to it", async () => {
    renderApp();
    const day = await screen.findByRole("region", { name: LONG.format(TODAY) });
    const market = eventButton(day, "Farmers' market");
    fireEvent.click(market);
    const detail = screen.getByRole("heading", { name: "Farmers' market" }).closest("section, article, aside, div") as HTMLElement;
    expect(within(detail).getByText("Market Square")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /close/i }));
    expect(screen.queryByRole("heading", { name: "Farmers' market" })).not.toBeInTheDocument();
    expect(market).toHaveFocus();
  });

  it("shows a week Monday to Sunday and a festival on each of its days", async () => {
    renderApp();
    await screen.findByRole("table");
    fireEvent.click(screen.getByRole("tab", { name: "Week" }));
    const days = within(screen.getByRole("tabpanel")).getAllByRole("listitem").filter((item) => item.parentElement?.classList.contains("app-week"));
    expect(days).toHaveLength(7);
    expect(days[4]).toHaveAttribute("aria-current", "date");
    expect(within(days[0]).getByText("Nothing planned.")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Next week" }));
    const next = within(screen.getByRole("tabpanel"));
    const design = next.getAllByRole("button", { name: /Design week/ });
    expect(design).toHaveLength(3);
    expect(design[1]).toHaveTextContent("all day");
    expect(design[2]).toHaveTextContent(/until/);
  });

  it("lists what is coming first, says what has no date, and keeps the past folded", async () => {
    renderApp();
    await screen.findByRole("table");
    fireEvent.click(screen.getByRole("tab", { name: "Timeline" }));
    const panel = within(screen.getByRole("tabpanel"));
    expect(panel.getAllByRole("heading", { level: 4 })[0]).toHaveTextContent(`Today, ${LONG.format(TODAY)}`);
    expect(panel.getByText("1 event has no date yet.")).toBeInTheDocument();
    expect(panel.getByText("Earlier (2)")).toBeInTheDocument();
    expect(panel.getByText("Library reading night").closest("details")).not.toHaveAttribute("open");
  });

  it("says so when there are no events", async () => {
    renderApp([]);
    expect(await screen.findByText("No events yet.")).toBeInTheDocument();
  });

  it("offers a retry when the endpoint refuses", async () => {
    render(
      <JcProvider client={stubClient({ entities: [], refuse: () => ({ status: 503, body: { title: "Service Unavailable", detail: "the broker is restarting" } }) })}>
        <App />
      </JcProvider>,
    );
    expect(await screen.findByRole("button", { name: /retry|try again/i })).toBeInTheDocument();
  });
});

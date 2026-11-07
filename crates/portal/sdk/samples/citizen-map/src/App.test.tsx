import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { ROWS } from "./fixtures";

vi.mock("maplibre-gl", () => ({ Map: class { on = vi.fn(); remove = vi.fn(); }, setWorkerUrl: vi.fn() }));
vi.mock("@deck.gl/mapbox", () => ({ MapboxOverlay: class {} }));
vi.mock("@deck.gl/layers", () => ({ ScatterplotLayer: class {} }));
vi.mock("@deck.gl/aggregation-layers", () => ({ HexagonLayer: class {}, GridLayer: class {} }));

const STATIONS = ROWS.slice(0, 3);

function renderApp(rows: Row[] = STATIONS): ReturnType<typeof render> {
  const client = stubClient({ entities: rows });
  return render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
}

describe("the citizen map", () => {
  it("shows the map under a search, and lists what the search finds for a keyboard", async () => {
    renderApp();
    expect(await screen.findByTestId("jc-map")).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "City bikes near you" })).toBeInTheDocument();
    expect(screen.queryByRole("navigation", { name: "Stations found" })).not.toBeInTheDocument();

    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "ka" } });
    const found = screen.getByRole("navigation", { name: "Stations found" });
    const names = within(found).getAllByRole("button").map((button) => button.textContent);
    expect(names).toEqual(["Kaivopuisto4 bikes", "Kamppi0 bikes", "Kallio— bikes"]);
  });

  it("opens the sheet of the chosen place, reads an unknown count as a dash, and Escape gives focus back", async () => {
    renderApp();
    await screen.findByTestId("jc-map");
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "kallio" } });
    const choice = screen.getByRole("button", { name: /Kallio/ });
    fireEvent.click(choice);

    const sheet = screen.getByRole("region", { name: "Kallio" });
    expect(within(sheet).getByRole("heading", { name: "Kallio" })).toHaveFocus();
    expect(within(sheet).getByText("Bikes now").nextSibling).toHaveTextContent("—");
    expect(within(sheet).getByText("Free docks").nextSibling).toHaveTextContent("20");
    expect(within(sheet).getByText("Out of service")).toBeInTheDocument();
    expect(choice).toHaveAttribute("aria-current", "true");

    fireEvent.keyDown(sheet, { key: "Escape" });
    expect(screen.queryByRole("region", { name: "Kallio" })).not.toBeInTheDocument();
    expect(choice).toHaveFocus();
  });

  it("says when nothing matches, rather than showing an empty list", async () => {
    renderApp();
    await screen.findByTestId("jc-map");
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "zzz" } });
    expect(screen.getByRole("status")).toHaveTextContent("No station is called that");
  });

  it("offers a retry when the endpoint refuses", async () => {
    const client = stubClient({
      entities: STATIONS,
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

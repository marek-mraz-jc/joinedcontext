import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { ROWS } from "./fixtures";
import { readings, stationMeans, summarise } from "./story";

vi.mock("echarts", () => ({ init: vi.fn(() => ({ setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn(), on: vi.fn() })) }));
vi.mock("maplibre-gl", () => ({ Map: class { on = vi.fn(); remove = vi.fn(); }, setWorkerUrl: vi.fn() }));
vi.mock("@deck.gl/mapbox", () => ({ MapboxOverlay: class {} }));
vi.mock("@deck.gl/layers", () => ({ ScatterplotLayer: class {} }));
vi.mock("@deck.gl/aggregation-layers", () => ({ HexagonLayer: class {}, GridLayer: class {} }));

function renderStory(rows: Row[] = ROWS) {
  render(
    <JcProvider client={stubClient({ entities: rows })}>
      <App />
    </JcProvider>,
  );
}

describe("the data story", () => {
  it("says in its lede what the rows say", async () => {
    renderStory();
    const story = summarise(readings(ROWS))!;
    const lede = await screen.findByText(/Across 4 stations, fine particles averaged/);
    expect(lede).toHaveTextContent(`On ${story.daysOver} of 14 days the city-wide mean was above the WHO daily guideline of 15 µg/m³.`);
    expect(story.daysOver).toBeGreaterThan(0);
    expect(screen.getByText(/Mannerheimintie had the most/)).toBeInTheDocument();
    expect(screen.getByText(/Vartiokylä had the least/)).toBeInTheDocument();
  });

  it("gives every chart a caption and a table of the same numbers", async () => {
    renderStory();
    await screen.findByRole("heading", { name: "Where it was worst" });
    const table = screen.getByRole("table", { name: "Mean PM2.5 per station" });
    const names = within(table).getAllByRole("rowheader").map((cell) => cell.textContent);
    expect(names).toEqual(stationMeans(readings(ROWS)).map((item) => item.station));
    expect(screen.getByRole("table", { name: "City-wide mean PM2.5 per day" })).toBeInTheDocument();
    // The story's own figures: the map and chart inside them are figures of their own.
    const figures = [...document.querySelectorAll(".app-story section > figure")];
    expect(figures).toHaveLength(3);
    for (const figure of figures) expect(figure.querySelector(":scope > figcaption")).not.toBeNull();
  });

  it("changes its words when no day was over the guideline", async () => {
    renderStory(ROWS.map((row) => ({ ...row, pm25: 4 })));
    expect(await screen.findByText(/stayed under the WHO daily guideline of 15 µg\/m³ on every one of the 14 days/)).toBeInTheDocument();
    expect(screen.getByText(/The last week was as clean as the first\./)).toBeInTheDocument();
  });

  it("tells no story from no readings", async () => {
    renderStory([]);
    expect(await screen.findByText("No readings to tell a story from yet.")).toBeInTheDocument();
  });

  it("offers a retry when the endpoint refuses", async () => {
    render(
      <JcProvider client={stubClient({ entities: ROWS, refuse: () => ({ status: 503, body: { title: "Service Unavailable", detail: "the broker is restarting" } }) })}>
        <App />
      </JcProvider>,
    );
    expect(await screen.findByRole("button", { name: /retry|try again/i })).toBeInTheDocument();
  });
});

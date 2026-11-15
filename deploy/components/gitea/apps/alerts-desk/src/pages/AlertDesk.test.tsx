import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "../App";
import { ROWS } from "../fixtures";

vi.mock("maplibre-gl", () => ({ Map: class {}, setWorkerUrl: vi.fn() }));

function app(entities: Row[] = ROWS) {
  const client = stubClient({ entities }, { appName: "alerts-desk", orgDomain: "example.org", space: "demo", portal: "https://portal.example.org/projects/demo" });
  render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
  return client;
}

/** The table, once the page has read its rows: the loading state comes first (T-3016). */
async function table() {
  return screen.findByRole("table", { name: "Traffic alerts" });
}

const names = (grid: HTMLElement) => within(grid).getAllByRole("row").slice(1).map((row) => within(row).getAllByRole("cell")[0].textContent);

describe("alerts-desk", () => {
  it("lists every alert, the latest end first, under one heading", async () => {
    app();
    const grid = await table();
    expect(screen.getByRole("heading", { level: 1, name: "Alerts desk" })).toBeInTheDocument();
    expect(names(grid)).toEqual(["Ring I lane closure", "Marathon closes the centre", "Mannerheimintie resurfacing", "Bridge weight limit"]);
  });

  it("sorts by any column, both ways", async () => {
    app();
    const grid = await table();
    fireEvent.click(within(grid).getByRole("button", { name: "Alert" }));
    expect(names(grid)[0]).toBe("Bridge weight limit");
    fireEvent.click(within(grid).getByRole("button", { name: /^Alert/ }));
    expect(names(grid)[0]).toBe("Ring I lane closure");
  });

  it("sorts by every column it shows", async () => {
    app();
    const grid = await table();
    for (const name of ["Category", "Subcategory", "Address", "Valid from", "Valid to", "Issued"]) {
      fireEvent.click(within(grid).getByRole("button", { name }));
      expect(within(grid).getByRole("button", { name }).closest("th")).toHaveAttribute("aria-sort", "ascending");
    }
  });

  it("narrows by subcategory and by when an alert ends", async () => {
    app();
    await table();
    fireEvent.change(screen.getByRole("combobox", { name: "Subcategory" }), { target: { value: "ROAD_WORK" } });
    expect(names(await table())).toEqual(["Ring I lane closure", "Mannerheimintie resurfacing"]);
    fireEvent.change(screen.getByRole("combobox", { name: "Subcategory" }), { target: { value: "" } });
    fireEvent.change(screen.getByLabelText("Valid to from"), { target: { value: "2026-10-01" } });
    fireEvent.change(screen.getByLabelText("Valid to to"), { target: { value: "2026-12-31" } });
    expect(names(await table())).not.toContain("Mannerheimintie resurfacing");
  });

  it("narrows by category and by search, and says when nothing matches", async () => {
    app();
    const grid = await table();
    fireEvent.change(screen.getByRole("combobox", { name: "Category" }), { target: { value: "event" } });
    expect(names(await table())).toEqual(["Marathon closes the centre"]);
    fireEvent.change(screen.getByRole("searchbox", { name: "Search" }), { target: { value: "no such alert" } });
    expect(await screen.findByText("No alert matches these filters.")).toBeInTheDocument();
    expect(grid).not.toBeInTheDocument();
  });

  it("opens the chosen alert in the SDK's panel, links it to the Portal and writes nothing", async () => {
    const client = app();
    const grid = await table();
    fireEvent.click(within(grid).getByText("Mannerheimintie resurfacing"));
    const panel = await screen.findByRole("dialog", { name: "Mannerheimintie resurfacing" });
    expect(await within(panel).findByText("One lane closed northbound.")).toBeInTheDocument();
    expect(within(panel).getByRole("link", { name: "Open in the Portal" })).toHaveAttribute(
      "href",
      expect.stringContaining(`entityId=${encodeURIComponent(ROWS[0].id)}`),
    );
    expect(within(grid).getByText("Mannerheimintie resurfacing").closest("tr")).toHaveAttribute("aria-selected", "true");
    // Following the link leaves the App; jsdom does not navigate, so the click is only seen to reach it.
    const link = within(panel).getByRole("link", { name: "Open in the Portal" });
    const followed = vi.fn((event: Event) => event.preventDefault());
    link.addEventListener("click", followed);
    fireEvent.click(link);
    expect(followed).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("button", { name: /edit|new|delete|save/i })).toBeNull();
    fireEvent.click(within(panel).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    // Enter on a row opens it as a click does.
    fireEvent.keyDown(within(grid).getByText("Bridge weight limit").closest("tr")!, { key: "Enter" });
    expect(await screen.findByRole("dialog", { name: "Bridge weight limit" })).toBeInTheDocument();
    expect(client.transport.calls.filter((call) => call.method !== "GET")).toEqual([]);
  });

  it("says so when the endpoint holds no alerts", async () => {
    app([]);
    expect(await screen.findByText("The endpoint holds no alerts right now.")).toBeInTheDocument();
  });
});

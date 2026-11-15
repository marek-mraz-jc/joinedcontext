import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ProblemError } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { EntityTable, defaultColumns } from "./EntityTable";

const STATIONS: Row[] = [
  {
    id: "urn:ngsi-ld:Station:1",
    type: "Station",
    name: "Kaivopuisto",
    bikes: 10,
    location: { type: "Point", coordinates: [24.95, 60.15] },
  },
  {
    id: "urn:ngsi-ld:Station:2",
    type: "Station",
    name: "Kamppi",
    bikes: 5,
    location: { type: "Point", coordinates: [24.93, 60.17] },
  },
  {
    id: "urn:ngsi-ld:Station:3",
    type: "Station",
    name: "Kallio",
    bikes: 20,
    location: null,
  },
];

describe("defaultColumns", () => {
  it("excludes id, type, @context, and geo columns, and caps at max", () => {
    const cols = defaultColumns(STATIONS, 8);
    expect(cols).toEqual(["name", "bikes"]);
    expect(cols).not.toContain("id");
    expect(cols).not.toContain("type");
    expect(cols).not.toContain("location");

    const capped = defaultColumns(STATIONS, 1);
    expect(capped).toHaveLength(1);
    expect(capped[0]).toBe("name");
  });
});

describe("EntityTable", () => {
  // T-3418: the hooks run before any early return, so an empty table keeps what the reader chose.
  it("keeps its sort when a filter empties it and then lets rows back in", () => {
    const view = render(<EntityTable rows={STATIONS} columns={["name", "bikes"]} />);
    fireEvent.click(screen.getByRole("button", { name: /^bikes/ }));
    const bikes = () => screen.getByRole("columnheader", { name: /^bikes/ });
    expect(bikes()).toHaveAttribute("aria-sort", "ascending");
    view.rerender(<EntityTable rows={[]} columns={["name", "bikes"]} />);
    expect(screen.queryByRole("table")).toBeNull();
    view.rerender(<EntityTable rows={STATIONS} columns={["name", "bikes"]} />);
    expect(bikes()).toHaveAttribute("aria-sort", "ascending");
    expect(screen.getAllByRole("row")[1]).toHaveTextContent("Kamppi");
  });

  it("renders Problem state when error is provided", () => {
    const error = new ProblemError(403, { title: "Forbidden", detail: "Read denied" });
    render(<EntityTable rows={[]} error={error} />);
    expect(screen.getByRole("alert")).toBeInTheDocument();
    // A one-word title is a code, not a sentence: the SDK names the refusal and keeps the code's
    // detail out (T-4200).
    expect(screen.getByText("You do not have the right to do this.")).toBeInTheDocument();
    expect(screen.queryByText("Read denied")).not.toBeInTheDocument();
  });

  it("renders Loading state when loading is true and rows are empty", () => {
    render(<EntityTable rows={[]} loading />);
    expect(screen.getByRole("status")).toHaveTextContent("Loading…");
  });

  it("renders Empty state when rows are empty", () => {
    render(<EntityTable rows={[]} empty="No stations found." />);
    expect(screen.getByText("No stations found.")).toBeInTheDocument();
  });

  it("renders columns and handles sort toggling asc/desc with aria-sort", () => {
    render(<EntityTable rows={STATIONS} columns={["name", "bikes"]} />);

    const nameBtn = screen.getByRole("button", { name: "name" });
    const nameTh = nameBtn.closest("th")!;
    expect(nameTh).toHaveAttribute("aria-sort", "none");

    // First click -> asc
    fireEvent.click(nameBtn);
    expect(nameTh).toHaveAttribute("aria-sort", "ascending");
    let cells = screen.getAllByRole("row").slice(1).map((r) => r.children[0].textContent);
    expect(cells).toEqual(["Kaivopuisto", "Kallio", "Kamppi"]);

    // Second click -> desc
    fireEvent.click(nameBtn);
    expect(nameTh).toHaveAttribute("aria-sort", "descending");
    cells = screen.getAllByRole("row").slice(1).map((r) => r.children[0].textContent);
    expect(cells).toEqual(["Kamppi", "Kallio", "Kaivopuisto"]);

    // Third click -> back to asc
    fireEvent.click(nameBtn);
    expect(nameTh).toHaveAttribute("aria-sort", "ascending");
    cells = screen.getAllByRole("row").slice(1).map((r) => r.children[0].textContent);
    expect(cells).toEqual(["Kaivopuisto", "Kallio", "Kamppi"]);
  });

  it("handles pagination and resets page on rows change or sort change", () => {
    const manyRows: Row[] = Array.from({ length: 65 }, (_, i) => ({
      id: `urn:station:${i}`,
      type: "Station",
      name: `Station ${String(i).padStart(2, "0")}`,
    }));

    const { rerender } = render(<EntityTable rows={manyRows} pageSize={50} columns={["name"]} />);

    expect(screen.getByText("Page 1 of 2")).toBeInTheDocument();
    const prevBtn = screen.getByRole("button", { name: "Previous" });
    const nextBtn = screen.getByRole("button", { name: "Next" });

    expect(prevBtn).toBeDisabled();
    expect(nextBtn).not.toBeDisabled();

    // Go to next page
    fireEvent.click(nextBtn);
    expect(screen.getByText("Page 2 of 2")).toBeInTheDocument();
    expect(nextBtn).toBeDisabled();
    expect(prevBtn).not.toBeDisabled();
    fireEvent.click(prevBtn);
    expect(screen.getByText("Page 1 of 2")).toBeInTheDocument();
    fireEvent.click(nextBtn);

    // Sort change resets page to 1
    const nameBtn = screen.getByRole("button", { name: "name" });
    fireEvent.click(nameBtn);
    expect(screen.getByText("Page 1 of 2")).toBeInTheDocument();

    // Advance to page 2 again
    fireEvent.click(nextBtn);
    expect(screen.getByText("Page 2 of 2")).toBeInTheDocument();

    // Changing rows length resets page to 1
    rerender(<EntityTable rows={manyRows.slice(0, 60)} pageSize={50} columns={["name"]} />);
    expect(screen.getByText("Page 1 of 2")).toBeInTheDocument();
  });

  it("handles onSelect on click and Enter, and marks aria-selected", () => {
    const onSelect = vi.fn();
    render(
      <EntityTable
        rows={STATIONS}
        columns={["name"]}
        selected="urn:ngsi-ld:Station:2"
        onSelect={onSelect}
      />,
    );

    const rows = screen.getAllByRole("row").slice(1);
    expect(rows[1]).toHaveAttribute("aria-selected", "true");
    expect(rows[0]).toHaveAttribute("aria-selected", "false");

    fireEvent.click(rows[0]);
    expect(onSelect).toHaveBeenCalledWith(STATIONS[0]);

    fireEvent.keyDown(rows[2], { key: "Enter" });
    expect(onSelect).toHaveBeenCalledWith(STATIONS[2]);
  });
});

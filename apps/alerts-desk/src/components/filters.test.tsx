import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { useFilters } from "@joinedcontext/sdk";
import type { FilterDef, Row } from "@joinedcontext/sdk";
import {
  DateRangeFilter,
  FilterBar,
  RangeFilter,
  SearchBox,
  SelectFilter,
} from "./filters";

const SAMPLE_ROWS: Row[] = [
  { id: "urn:1", type: "Station", name: "Kamppi", bikes: 5, status: "active", registered: "2024-05-09T10:00:00Z" },
  { id: "urn:2", type: "Station", name: "Kaivopuisto", bikes: 10, status: "inactive", registered: "2024-05-10T18:30:00Z" },
  { id: "urn:3", type: "Station", name: "Kallio", bikes: 20, status: "active", registered: "2024-05-11T09:00:00Z" },
];

function TestHarness({
  rows = SAMPLE_ROWS,
  filters,
}: {
  rows?: Row[];
  filters: FilterDef[];
}) {
  const { shown, bind, reset } = useFilters(rows, filters);

  return (
    <div>
      <FilterBar onReset={reset} shown={shown.length} total={rows.length}>
        {filters.map((f, i) => {
          if (f.kind === "search") return <SearchBox key={i} binding={bind(i)} placeholder="Search..." />;
          if (f.kind === "select") return <SelectFilter key={i} binding={bind(i)} />;
          if (f.kind === "range") return <RangeFilter key={i} binding={bind(i)} />;
          if (f.kind === "dateRange") return <DateRangeFilter key={i} binding={bind(i)} />;
          return null;
        })}
      </FilterBar>
      <ul data-testid="results">
        {shown.map((r) => (
          <li key={r.id}>{String(r.name)}</li>
        ))}
      </ul>
    </div>
  );
}

describe("Filter UI components with useFilters", () => {
  it("SearchBox narrows results and clearing restores them", () => {
    const filters: FilterDef[] = [{ kind: "search", attrs: ["name"] }];
    render(<TestHarness filters={filters} />);

    expect(screen.getByText("3 of 3")).toBeInTheDocument();
    const input = screen.getByPlaceholderText("Search...");

    fireEvent.change(input, { target: { value: "Kamppi" } });
    expect(screen.getByText("1 of 3")).toBeInTheDocument();
    expect(screen.getByText("Kamppi")).toBeInTheDocument();
    expect(screen.queryByText("Kaivopuisto")).not.toBeInTheDocument();

    fireEvent.change(input, { target: { value: "" } });
    expect(screen.getByText("3 of 3")).toBeInTheDocument();
  });

  it("SelectFilter lists distinct values and 'All' resets selection", () => {
    const filters: FilterDef[] = [{ kind: "select", attr: "status" }];
    render(<TestHarness filters={filters} />);

    const select = screen.getByRole("combobox") as HTMLSelectElement;
    expect(select.options).toHaveLength(3); // All, active, inactive
    expect(select.options[0].text).toBe("All");

    fireEvent.change(select, { target: { value: "inactive" } });
    expect(screen.getByText("1 of 3")).toBeInTheDocument();
    expect(screen.getByText("Kaivopuisto")).toBeInTheDocument();

    fireEvent.change(select, { target: { value: "" } });
    expect(screen.getByText("3 of 3")).toBeInTheDocument();
  });

  it("RangeFilter supports open bounds on either side", () => {
    const filters: FilterDef[] = [{ kind: "range", attr: "bikes" }];
    render(<TestHarness filters={filters} />);

    const fromInput = screen.getByLabelText("bikes from");
    const toInput = screen.getByLabelText("bikes to");

    // Open upper bound: from 10, to blank
    fireEvent.change(fromInput, { target: { value: "10" } });
    expect(screen.getByText("2 of 3")).toBeInTheDocument();
    expect(screen.getByText("Kaivopuisto")).toBeInTheDocument();
    expect(screen.getByText("Kallio")).toBeInTheDocument();

    // Open lower bound: from blank, to 10
    fireEvent.change(fromInput, { target: { value: "" } });
    fireEvent.change(toInput, { target: { value: "10" } });
    expect(screen.getByText("2 of 3")).toBeInTheDocument();
    expect(screen.getByText("Kamppi")).toBeInTheDocument();
    expect(screen.getByText("Kaivopuisto")).toBeInTheDocument();

    // Both blank resets
    fireEvent.change(toInput, { target: { value: "" } });
    expect(screen.getByText("3 of 3")).toBeInTheDocument();
  });

  it("DateRangeFilter includes the whole 'to' day and displays first 10 characters", () => {
    const filters: FilterDef[] = [{ kind: "dateRange", attr: "registered" }];
    render(<TestHarness filters={filters} />);

    const toInput = screen.getByLabelText("registered to") as HTMLInputElement;
    // From the 10th on: Kamppi (the 9th) drops out, then the whole range is cleared again.
    fireEvent.change(screen.getByLabelText("registered from"), { target: { value: "2024-05-10" } });
    expect(screen.getByText("2 of 3")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("registered from"), { target: { value: "" } });
    expect(screen.getByText("3 of 3")).toBeInTheDocument();

    // Row 2 is 2024-05-10T18:30:00Z; setting to "2024-05-10" includes it via T23:59:59.999Z
    fireEvent.change(toInput, { target: { value: "2024-05-10" } });

    expect(screen.getByText("2 of 3")).toBeInTheDocument();
    expect(screen.getByText("Kamppi")).toBeInTheDocument();
    expect(screen.getByText("Kaivopuisto")).toBeInTheDocument();
    expect(screen.queryByText("Kallio")).not.toBeInTheDocument();

    expect(toInput.value).toBe("2024-05-10");
  });

  it("FilterBar displays count and Reset button restores all rows", () => {
    const filters: FilterDef[] = [{ kind: "search", attrs: ["name"] }];
    render(<TestHarness filters={filters} />);

    const input = screen.getByPlaceholderText("Search...");
    fireEvent.change(input, { target: { value: "Kamppi" } });
    expect(screen.getByText("1 of 3")).toBeInTheDocument();

    const resetBtn = screen.getByRole("button", { name: "Reset" });
    fireEvent.click(resetBtn);

    expect(screen.getByText("3 of 3")).toBeInTheDocument();
  });

  it("throws when a binding does not match component expectation", () => {
    const searchBinding = {
      def: { kind: "search" as const, attrs: ["name"] },
      rows: [],
      value: undefined,
      onChange: () => {},
    };
    const selectBinding = {
      def: { kind: "select" as const, attr: "status" },
      rows: [],
      value: undefined,
      onChange: () => {},
    };

    expect(() => render(<SearchBox binding={selectBinding as never} />)).toThrow(
      "SearchBox needs a search filter",
    );
    expect(() => render(<SelectFilter binding={searchBinding as never} />)).toThrow(
      "SelectFilter needs a select filter",
    );
    expect(() => render(<RangeFilter binding={searchBinding as never} />)).toThrow(
      "RangeFilter needs a range filter",
    );
    expect(() => render(<DateRangeFilter binding={searchBinding as never} />)).toThrow(
      "DateRangeFilter needs a dateRange filter",
    );
  });
});

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Row } from "@joinedcontext/sdk";
import { StatTiles } from "./StatTiles";

const ROWS: Row[] = [
  { id: "a", type: "Station", bikes: 0 },
  { id: "b", type: "Station", bikes: 4 },
  { id: "c", type: "Station" },
  { id: "d", type: "Station", bikes: null },
];

describe("StatTiles", () => {
  // T-6442: 67 empty stations read 187, because 120 report no number.
  it("counts a value exactly, and a row without it as not reporting", () => {
    render(<StatTiles rows={ROWS} tiles={[{ label: "Empty", attr: "bikes", equals: 0 }]} />);
    expect(screen.getByText("1")).toBeTruthy();
    expect(screen.getByText("2 not reporting")).toBeTruthy();
  });

  it("averages only what the rows report", () => {
    render(<StatTiles rows={ROWS} tiles={[{ label: "Average", agg: "avg", attr: "bikes" }]} />);
    expect(screen.getByText("2")).toBeTruthy();
    expect(screen.getByText("2 not reporting")).toBeTruthy();
  });

  it("says nothing of unknowns when every row reports, or while loading", () => {
    render(<StatTiles rows={ROWS.slice(0, 2)} tiles={[{ label: "All", attr: "bikes", agg: "sum" }]} />);
    render(<StatTiles rows={ROWS} loading tiles={[{ label: "Busy", attr: "bikes", agg: "sum" }]} />);
    expect(screen.queryByText(/not reporting/)).toBeNull();
  });
});

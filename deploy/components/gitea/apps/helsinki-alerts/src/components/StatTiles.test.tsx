import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Row } from "@joinedcontext/sdk";
import { StatTiles } from "./StatTiles";

const ROWS: Row[] = [
  { id: "urn:a", type: "Alert", lanes: 2 },
  { id: "urn:b", type: "Alert", lanes: 3 },
];

describe("StatTiles", () => {
  it("shows the count and a mean with its unit, a dash for what has no number, and an ellipsis while reading", () => {
    const tiles = [
      { label: "Alerts" },
      { label: "Lanes", agg: "avg" as const, attr: "lanes", unit: "lanes", digits: 0 },
      { label: "Length", agg: "avg" as const, attr: "length" },
    ];
    const view = render(<StatTiles rows={ROWS} tiles={tiles} />);
    const value = (label: string) => screen.getByText(label).closest(".jc-tile")?.querySelector(".jc-tile-value")?.textContent;
    expect(value("Alerts")).toBe("2");
    expect(value("Lanes")).toBe("3 lanes");
    expect(value("Length")).toBe("–");
    view.rerender(<StatTiles rows={ROWS} tiles={tiles} loading />);
    expect(value("Alerts")).toBe("…");
  });
});

/**
 * T-3127: a station at 100 % ends its bar at the plot's right edge, and its value label ("100 %")
 * is drawn right of it; the plot leaves room for it, or the label is cut to "10".
 */
import { describe, expect, it } from "vitest";
import { DEFAULT_TOKENS } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { rankedOption } from "./charts";

const FULL: Row[] = [
  { id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:1", type: "BikeHireDockingStation", name: "Sompasaari", availableBikeNumber: 20, freeSlotNumber: 0, totalSlotNumber: 20 } as Row,
];

describe("the ranked station bars", () => {
  it("leave room right of a full bar for its whole value label", () => {
    const option = rankedOption(FULL, "fullest", 10, DEFAULT_TOKENS) as { grid: { right: number }; series: { label: { position: string } }[] };
    expect(option.series[0].label.position).toBe("right");
    // "100 %" at the chart's 12 px label font is about 40 px wide.
    expect(option.grid.right).toBeGreaterThanOrEqual(48);
  });
});

// T-7589: coverage for builderGrid settle sorting logic
import { describe, expect, it } from "vitest";
import { anyOverlap, dragged, firstFree, float, nextId, settle } from "../src/pages/grafana/builderGrid";
import type { Placed } from "../src/pages/grafana/builderGrid";

describe("builderGrid settle sorting comparator (line 43)", () => {
  it("sorts other panels with multiple items comparing by y first, and then x when y matches", () => {
    // 4 panels. Panel 1 is moved.
    // The other panels (id 2, 3, 4) must be sorted:
    // Panel 2 at (x: 12, y: 4)
    // Panel 3 at (x: 0, y: 4) -> same y=4, smaller x=0. The sort compares a.grid.y - b.grid.y (0) || a.grid.x - b.grid.x
    // Panel 4 at (x: 0, y: 10) -> different y=10. The sort compares a.grid.y - b.grid.y (!= 0)
    const panels: Placed[] = [
      { id: 1, grid: { x: 0, y: 0, w: 12, h: 4 } },
      { id: 2, grid: { x: 12, y: 4, w: 12, h: 4 } },
      { id: 3, grid: { x: 0, y: 4, w: 12, h: 4 } },
      { id: 4, grid: { x: 0, y: 10, w: 12, h: 4 } },
    ];

    // Move panel 1 down to (x: 0, y: 4), overlapping panel 3
    const settled = settle(panels, 1, { x: 0, y: 4, w: 12, h: 4 });
    expect(settled.find((p) => p.id === 1)?.grid).toEqual({ x: 0, y: 4, w: 12, h: 4 });
    expect(anyOverlap(settled)).toBe(false);

    // Another case where panel 2 has smaller x and panel 3 has larger x at same y, and panel 4 has smaller y
    const panels2: Placed[] = [
      { id: 10, grid: { x: 0, y: 0, w: 12, h: 4 } },
      { id: 20, grid: { x: 0, y: 6, w: 6, h: 4 } },
      { id: 30, grid: { x: 6, y: 6, w: 6, h: 4 } },
      { id: 40, grid: { x: 0, y: 2, w: 12, h: 2 } },
    ];

    const settled2 = settle(panels2, 10, { x: 0, y: 2, w: 24, h: 8 });
    expect(settled2.find((p) => p.id === 10)?.grid).toEqual({ x: 0, y: 2, w: 24, h: 8 });
    expect(anyOverlap(settled2)).toBe(false);
  });

  it("handles additional grid utility edge cases", () => {
    // Settle when moved panel is not in list
    const unlisted = settle([{ id: 1, grid: { x: 0, y: 0, w: 12, h: 4 } }], 999, { x: 0, y: 0, w: 12, h: 4 });
    expect(unlisted).toHaveLength(1);

    // dragged resize and move boundaries
    const start = { x: 5, y: 5, w: 10, h: 10 };
    expect(dragged(start, 0, 0, 10, 10, "move")).toEqual(start);
    expect(dragged(start, 0, 0, 10, 10, "resize")).toEqual(start);

    // float with empty list
    expect(float([])).toEqual([]);

    // firstFree and nextId with clamp
    expect(firstFree([], 25, 45)).toEqual({ x: 0, y: 0, w: 24, h: 40 });
    expect(nextId([{ id: 42, grid: { x: 0, y: 0, w: 1, h: 2 } }])).toBe(43);
  });
});

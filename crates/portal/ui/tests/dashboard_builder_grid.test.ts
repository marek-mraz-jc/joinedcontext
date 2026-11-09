/**
 * The builder's grid behaves as Grafana's: a moved or resized panel pushes those it lands on down,
 * the rest float up, nothing ever overlaps, and nothing leaves the 24 columns.
 */
import { describe, expect, it } from "vitest";
import { anyOverlap, clamp, dragged, firstFree, float, nextId, settle } from "../src/pages/grafana/builderGrid";
import type { Placed } from "../src/pages/grafana/builderGrid";

const two: Placed[] = [
  { id: 1, grid: { x: 0, y: 0, w: 12, h: 8 } },
  { id: 2, grid: { x: 12, y: 0, w: 12, h: 8 } },
];

describe("builder grid", () => {
  it("keeps a place inside the columns and rows", () => {
    expect(clamp({ x: 20, y: -3, w: 10, h: 1 })).toEqual({ x: 14, y: 0, w: 10, h: 2 });
    expect(clamp({ x: 0, y: 0, w: 40, h: 99 })).toEqual({ x: 0, y: 0, w: 24, h: 40 });
  });

  it("pushes a panel it lands on below it", () => {
    const out = settle(two, 2, { x: 6, y: 0, w: 12, h: 8 });
    expect(out.find((p) => p.id === 2)?.grid).toEqual({ x: 6, y: 0, w: 12, h: 8 });
    expect(out.find((p) => p.id === 1)?.grid).toEqual({ x: 0, y: 8, w: 12, h: 8 });
    expect(anyOverlap(out)).toBe(false);
  });

  it("widens a panel over its neighbour and keeps both", () => {
    const out = settle(two, 1, { x: 0, y: 0, w: 18, h: 8 });
    expect(out.find((p) => p.id === 2)?.grid.y).toBe(8);
    expect(anyOverlap(out)).toBe(false);
  });

  it("floats a panel up once the one above it moves away", () => {
    const stacked: Placed[] = [
      { id: 1, grid: { x: 0, y: 0, w: 24, h: 4 } },
      { id: 2, grid: { x: 0, y: 4, w: 24, h: 4 } },
    ];
    const out = settle(stacked, 1, { x: 0, y: 30, w: 12, h: 4 });
    expect(out.find((p) => p.id === 2)?.grid.y).toBe(0);
    expect(float([{ id: 3, grid: { x: 0, y: 9, w: 6, h: 2 } }])[0].grid.y).toBe(0);
  });

  it("places a new panel in the first free spot and numbers it next", () => {
    expect(firstFree(two, 12, 8)).toEqual({ x: 0, y: 8, w: 12, h: 8 });
    expect(firstFree([{ id: 1, grid: { x: 0, y: 0, w: 12, h: 8 } }], 12, 8)).toEqual({ x: 12, y: 0, w: 12, h: 8 });
    expect(firstFree([], 6, 4)).toEqual({ x: 0, y: 0, w: 6, h: 4 });
    expect(nextId(two)).toBe(3);
    expect(nextId([])).toBe(1);
  });

  it("turns a drag in pixels into whole cells", () => {
    const start = { x: 2, y: 2, w: 6, h: 6 };
    expect(dragged(start, 120, 70, 50, 36, "move")).toEqual({ x: 4, y: 4, w: 6, h: 6 });
    expect(dragged(start, -500, -500, 50, 36, "move")).toEqual({ x: 0, y: 0, w: 6, h: 6 });
    expect(dragged(start, 100, -1000, 50, 36, "resize")).toEqual({ x: 2, y: 2, w: 8, h: 2 });
  });
});

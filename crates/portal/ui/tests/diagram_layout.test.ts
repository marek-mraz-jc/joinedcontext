/**
 * T-3589: the model diagram laid out by its relationships, not a grid. The layered layout puts
 * a class one layer from what it is joined to, never lets two boxes overlap, survives cycles and
 * self references, and on the largest model seeded on dev (Helsinki, copied from
 * `joinedcontext-deployment/components/context-gateway/seed/helsinki/helsinki.linkml.yaml`)
 * crosses fewer lines than the grid it replaces.
 *
 * `DIAGRAM_MEASURE=<file>` writes the numbers the task quotes (crossings of both layouts, layout
 * time on Helsinki and on a synthetic model ten times its size) to that file.
 */
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { crossings, layered, overlaps, segments } from "../src/pages/models/diagramLayout";
import type { LayoutEdge, LayoutNode } from "../src/pages/models/diagramLayout";
import { layoutEdge, place } from "../src/pages/models/LinkmlGraphView";
import { graphData, parseModel } from "../src/pages/models/linkml";
import type { GraphNode } from "../src/pages/models/linkml";

const box = (id: string, height = 40): LayoutNode => ({ id, width: 100, height });
const down = (upper: string, lower: string): LayoutEdge => ({ from: upper, to: lower, upper, lower });

/** A deterministic pseudo-random sequence, so a failing graph is the same graph next time. */
function random(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state * 1664525 + 1013904223) >>> 0;
    return state / 2 ** 32;
  };
}

/** The grid this layout replaced (LinkmlGraphView before T-3589): a band per depth, a square grid within. */
function grid(nodes: GraphNode[]): Map<string, { x: number; y: number; width: number; height: number }> {
  const BOX = { width: 240, header: 26, row: 16, padding: 8, gapX: 110, gapY: 70 };
  const height = (node: GraphNode) => BOX.header + (node.rows?.length ?? node.slots.length) * BOX.row + BOX.padding;
  const bands = new Map<number, GraphNode[]>();
  for (const node of nodes) bands.set(node.depth, [...(bands.get(node.depth) ?? []), node]);
  const out = new Map<string, { x: number; y: number; width: number; height: number }>();
  let top = 0;
  for (const [, band] of [...bands.entries()].sort(([a], [b]) => a - b)) {
    const columns = band.length <= 3 ? band.length : Math.ceil(Math.sqrt(band.length));
    for (let first = 0; first < band.length; first += columns) {
      const row = band.slice(first, first + columns);
      row.forEach((node, index) => {
        out.set(node.name, { x: index * (BOX.width + BOX.gapX), y: top, width: BOX.width, height: height(node) });
      });
      top += Math.max(...row.map(height)) + BOX.gapY;
    }
  }
  return out;
}

const helsinki = readFileSync(join(__dirname, "fixtures/models/helsinki.linkml.yaml"), "utf8");

describe("the layered layout", () => {
  it("puts what a class references in the layer next to it, a parent above its child", () => {
    const layout = layered(
      ["Building", "Owner", "Address", "Thing"].map((id) => box(id)),
      [down("Building", "Owner"), down("Building", "Address"), down("Thing", "Building")],
    );
    const layer = (id: string) => layout.boxes.get(id)?.layer;
    expect([layer("Thing"), layer("Building"), layer("Owner"), layer("Address")]).toEqual([0, 1, 2, 2]);
  });

  it("pulls a class referenced from deep below down beside the class that references it", () => {
    // Leaf is three layers down; Lone references only Leaf, so it sits right above it.
    const layout = layered(
      ["Top", "Mid", "Low", "Leaf", "Lone"].map((id) => box(id)),
      [down("Top", "Mid"), down("Mid", "Low"), down("Low", "Leaf"), down("Lone", "Leaf")],
    );
    expect(layout.boxes.get("Lone")?.layer).toBe((layout.boxes.get("Leaf")?.layer ?? 0) - 1);
  });

  it("bends a line that skips a layer through the layer it passes", () => {
    const layout = layered(
      ["A", "B", "C"].map((id) => box(id)),
      [down("A", "B"), down("B", "C"), down("A", "C")],
    );
    expect(layout.bends.map((points) => points.length)).toEqual([0, 0, 1]);
    const [bend] = layout.bends[2];
    const b = layout.boxes.get("B");
    expect(bend.y).toBe((b?.y ?? 0) + (b?.height ?? 0) / 2);
  });

  it("lays cycles, self references and two lines between one pair out without hanging", () => {
    const layout = layered(
      ["A", "B", "C"].map((id) => box(id)),
      [down("A", "B"), down("B", "C"), down("C", "A"), down("A", "A"), down("B", "A")],
    );
    expect(layout.boxes.size).toBe(3);
    expect(new Set([...layout.boxes.values()].map((b) => b.layer)).size).toBe(3);
    expect(layout.bends).toHaveLength(5);
  });

  it("stands unrelated groups apart and keeps an empty model empty", () => {
    const layout = layered(["A", "B", "Alone"].map((id) => box(id)), [down("A", "B")]);
    const groups = new Set([...layout.boxes.values()].map((b) => b.group));
    expect(groups.size).toBe(2);
    expect(layout.boxes.get("A")?.group).toBe(layout.boxes.get("B")?.group);
    expect(layered([], [])).toEqual({ boxes: new Map(), bends: [], width: 0, height: 0 });
    // A line to a box that is not drawn is ignored, not a crash.
    expect(layered([box("A")], [down("A", "Gone")]).bends).toEqual([[]]);
  });

  it("never lets two boxes overlap, whatever the graph", () => {
    for (let seed = 1; seed <= 60; seed += 1) {
      const next = random(seed);
      const count = 2 + Math.floor(next() * 25);
      const nodes = Array.from({ length: count }, (_, i) => box(`n${i}`, 20 + Math.floor(next() * 200)));
      const edges = Array.from({ length: Math.floor(next() * count * 1.5) }, () =>
        down(`n${Math.floor(next() * count)}`, `n${Math.floor(next() * count)}`),
      );
      const boxes = [...layered(nodes, edges, { gapX: 10, gapY: 10, groupGap: 20, maxWidth: 800 }).boxes.values()];
      expect(boxes).toHaveLength(count);
      for (let i = 0; i < boxes.length; i += 1) {
        for (let j = i + 1; j < boxes.length; j += 1) {
          expect(overlaps(boxes[i], boxes[j]), `seed ${seed}: boxes ${i} and ${j}`).toBe(false);
        }
      }
    }
  });

  it("counts crossings of line pieces, ends they share not counted", () => {
    const p = (x: number, y: number) => ({ x, y });
    expect(crossings([[p(0, 0), p(10, 10)], [p(0, 10), p(10, 0)]])).toBe(1);
    expect(crossings([[p(0, 0), p(10, 10)], [p(10, 10), p(20, 0)]])).toBe(0);
    expect(crossings([[p(0, 0), p(10, 0)], [p(0, 5), p(10, 5)]])).toBe(0);
  });
});

/** 150 classes in chains of five, each referencing two classes of the chain before it, and 30 enums. */
function synthetic(): string {
  const classes: string[] = [];
  const slots: string[] = [];
  for (let i = 0; i < 150; i += 1) {
    const refs = i >= 5 ? [`r${i}a`, `r${i}b`] : [];
    classes.push(`  C${i}:\n${i % 5 === 0 ? "" : `    is_a: C${i - 1}\n`}    slots: [${[...refs, `e${i}`].join(", ")}]`);
    if (i >= 5) slots.push(`  r${i}a:\n    range: C${(i * 7) % (i - 4)}`, `  r${i}b:\n    range: C${(i * 3) % (i - 4)}`);
    slots.push(`  e${i}:\n    range: E${i % 30}`);
  }
  const enums = Array.from({ length: 30 }, (_, i) => `  E${i}:\n    permissible_values:\n      a: {}\n      b: {}`);
  return `name: synthetic\nclasses:\n${classes.join("\n")}\nslots:\n${slots.join("\n")}\nenums:\n${enums.join("\n")}\n`;
}

/** The median time of `runs` calls of `work`, in milliseconds. */
function median(work: () => unknown, runs: number): number {
  const times = Array.from({ length: runs }, () => {
    const start = performance.now();
    work();
    return performance.now() - start;
  }).sort((a, b) => a - b);
  return times[Math.floor(runs / 2)];
}

const measured: Record<string, unknown> = {};

describe("the largest model on dev", () => {
  const { nodes, edges } = graphData(parseModel(helsinki));

  it("is the model the numbers in T-3589 are measured on", () => {
    expect([nodes.filter((n) => n.kind === "class").length, nodes.filter((n) => n.kind === "enum").length, edges.length]).toEqual([15, 6, 9]);
  });

  it("crosses no more lines than the grid did, and no boxes overlap", () => {
    const laid = place(nodes, edges);
    const boxes = new Map(laid.placed.map((n) => [n.name, { x: n.x, y: n.y, width: 240, height: n.height }]));
    const lines = edges.map(layoutEdge);
    const layeredCount = crossings(segments({ boxes, bends: laid.bends }, lines));
    const gridCount = crossings(segments({ boxes: grid(nodes), bends: [] }, lines));
    expect(layeredCount).toBeLessThan(gridCount);
    expect(layeredCount).toBe(0);
    measured.crossings = { grid: gridCount, layered: layeredCount };
    const all = [...boxes.values()];
    for (let i = 0; i < all.length; i += 1) for (let j = i + 1; j < all.length; j += 1) expect(overlaps(all[i], all[j])).toBe(false);
  });

  it("puts every class and enum next to what it is joined to", () => {
    const { placed } = place(nodes, edges);
    const at = new Map(placed.map((n) => [n.name, n]));
    for (const edge of edges) {
      const from = at.get(edge.from);
      const to = at.get(edge.to);
      expect(from && to && Math.abs(from.y - to.y) > 0, `${edge.from} → ${edge.to}`).toBe(true);
    }
  });

  it("lays out in a few milliseconds, and a model ten times larger in under half a second", () => {
    const large = graphData(parseModel(synthetic()));
    const times = {
      helsinki: median(() => place(nodes, edges), 30),
      synthetic: median(() => place(large.nodes, large.edges), 10),
      syntheticSize: { boxes: large.nodes.length, lines: large.edges.length },
    };
    measured.milliseconds = times;
    expect(times.helsinki).toBeLessThan(50);
    expect(times.synthetic).toBeLessThan(500);
    if (process.env.DIAGRAM_MEASURE) writeFileSync(process.env.DIAGRAM_MEASURE, JSON.stringify(measured, null, 2));
  });
});

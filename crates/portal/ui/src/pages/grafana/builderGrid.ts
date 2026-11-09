/**
 * The builder's 24-column grid, as Grafana's dashboard grid behaves: a panel moved or resized
 * pushes the panels it lands on down, and every panel then floats up to the first row where it
 * fits. The server refuses an overlap; these helpers never leave one.
 */

export const COLUMNS = 24;
export const MIN_H = 2;
export const MAX_H = 40;

export interface Grid {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Placed {
  id: number;
  grid: Grid;
}

function overlaps(a: Grid, b: Grid): boolean {
  return a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}

/** A place kept on the grid: whole numbers, inside the columns, at least one column and MIN_H rows. */
export function clamp(grid: Grid): Grid {
  const w = Math.min(COLUMNS, Math.max(1, Math.round(grid.w)));
  const h = Math.min(MAX_H, Math.max(MIN_H, Math.round(grid.h)));
  const x = Math.min(COLUMNS - w, Math.max(0, Math.round(grid.x)));
  const y = Math.max(0, Math.round(grid.y));
  return { x, y, w, h };
}

/**
 * The panels after `moved` takes `to`: it keeps its place, the others it lands on go below it,
 * and then each floats up as far as it can, top to bottom.
 */
export function settle<T extends Placed>(panels: readonly T[], moved: number, to: Grid): T[] {
  const fixed = clamp(to);
  const placed: T[] = [];
  const others = panels.filter((panel) => panel.id !== moved).sort((a, b) => a.grid.y - b.grid.y || a.grid.x - b.grid.x);
  const self = panels.find((panel) => panel.id === moved);
  if (self) placed.push({ ...self, grid: fixed });
  for (const panel of others) {
    let grid = { ...panel.grid };
    // Pushed down below whatever it lands on.
    for (let hit = placed.find((p) => overlaps(p.grid, grid)); hit; hit = placed.find((p) => overlaps(p.grid, grid))) {
      grid = { ...grid, y: hit.grid.y + hit.grid.h };
    }
    placed.push({ ...panel, grid });
  }
  return float(placed, moved);
}

/** Every panel but `keep` up as far as it fits, top to bottom; `keep` stays where it was put. */
export function float<T extends Placed>(panels: readonly T[], keep?: number): T[] {
  const order = [...panels].sort((a, b) => a.grid.y - b.grid.y || a.grid.x - b.grid.x);
  const out: T[] = [];
  const anchor = order.find((panel) => panel.id === keep);
  if (anchor) out.push(anchor);
  for (const panel of order) {
    if (panel.id === keep) continue;
    let y = panel.grid.y;
    while (y > 0 && !out.some((p) => overlaps(p.grid, { ...panel.grid, y: y - 1 }))) y -= 1;
    out.push({ ...panel, grid: { ...panel.grid, y } });
  }
  return panels.map((panel) => out.find((p) => p.id === panel.id) ?? panel);
}

/** Where a new panel of `w` by `h` goes: the first row from the top where it fits, left first. */
export function firstFree(panels: readonly Placed[], w: number, h: number): Grid {
  const size = clamp({ x: 0, y: 0, w, h });
  for (let y = 0; ; y += 1) {
    for (let x = 0; x + size.w <= COLUMNS; x += 1) {
      const grid = { ...size, x, y };
      if (!panels.some((panel) => overlaps(panel.grid, grid))) return grid;
    }
  }
}

/** The next free panel id, as Grafana numbers them. */
export function nextId(panels: readonly Placed[]): number {
  return panels.reduce((most, panel) => Math.max(most, panel.id), 0) + 1;
}

/** Whether any two panels share a cell. */
export function anyOverlap(panels: readonly Placed[]): boolean {
  return panels.some((a, i) => panels.slice(i + 1).some((b) => overlaps(a.grid, b.grid)));
}

/** The grid cell under a pointer at `dx`, `dy` px from where a drag started. */
export function dragged(start: Grid, dx: number, dy: number, column: number, row: number, mode: "move" | "resize"): Grid {
  const cols = Math.round(dx / column);
  const rows = Math.round(dy / row);
  return clamp(mode === "move" ? { ...start, x: start.x + cols, y: start.y + rows } : { ...start, w: start.w + cols, h: start.h + rows });
}

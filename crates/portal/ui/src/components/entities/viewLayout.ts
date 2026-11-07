/**
 * The columns a person arranged on "All rows" of a type, kept in this browser (T-4827): hidden
 * fields, order, widths and labels. A saved view keeps its own in the view store; this is the
 * convenience for the unsaved one, so a reload finds the grid as it was left.
 */
import type { ViewConfig } from "../../api/dataViews";

export type KeptLayout = Pick<ViewConfig, "hidden" | "width" | "order" | "labels">;

const key = (project: string, space: string, type: string) => `jc.space-columns.${project}.${space}.${type}`;

const strings = (value: unknown): string[] | undefined =>
  Array.isArray(value) && value.every((each) => typeof each === "string") ? value : undefined;

function record<T>(value: unknown, ok: (each: unknown) => each is T): Record<string, T> | undefined {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return undefined;
  const entries = Object.entries(value as Record<string, unknown>).filter((entry): entry is [string, T] => ok(entry[1]));
  return Object.fromEntries(entries);
}

/** What another version or a hand wrote is checked field by field: a wrong one is dropped, not trusted. */
export function readLayout(project: string, space: string, type: string): KeptLayout | null {
  if (type === "") return null;
  try {
    const raw = window.localStorage.getItem(key(project, space, type));
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    return {
      hidden: strings(parsed.hidden),
      order: strings(parsed.order),
      width: record(parsed.width, (each): each is number => typeof each === "number" && each >= 40 && each <= 2000),
      labels: record(parsed.labels, (each): each is string => typeof each === "string" && each.length <= 80),
    };
  } catch {
    // A private window or blocked site data: the grid opens with the model's own columns.
    return null;
  }
}

export function writeLayout(project: string, space: string, type: string, layout: KeptLayout): void {
  try {
    window.localStorage.setItem(key(project, space, type), JSON.stringify(layout));
  } catch {
    // A layout that cannot be kept is a convenience, not the data.
  }
}

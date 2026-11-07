/**
 * Saved data views of a space (API/01 §30, T-3104): the Portal's own record of how one entity type
 * is looked at. The rows a view shows are still read with the person's own session.
 */
import { api, unwrap } from "./client";
import type { components } from "./schema";

export type DataView = components["schemas"]["DataView"];
export type ViewConfig = components["schemas"]["ViewConfig"];
export type ViewMode = "personal" | "collaborative" | "locked";
/**
 * The one kind the Portal saves (T-6210). A row stored with a kind of before (gallery, kanban,
 * calendar, timeline, map, form) stays on the server and opens as the grid.
 */
export type ViewKind = "grid";

export const VIEW_MODES: ViewMode[] = ["personal", "collaborative", "locked"];

const LIST = "/api/v1/projects/{project}/spaces/{space}/views";
const ONE = "/api/v1/projects/{project}/spaces/{space}/views/{id}";

export const dataViewsKey = (project: string, space: string) => ["data-views", project, space] as const;

export async function listViews(project: string, space: string): Promise<DataView[]> {
  const answer = await unwrap(await api.GET(LIST, { params: { path: { project, space } } }));
  return answer.items;
}

export async function createView(
  project: string,
  space: string,
  body: { type: string; kind: ViewKind; mode: ViewMode; title: string; config: ViewConfig },
): Promise<DataView> {
  return unwrap(await api.POST(LIST, { params: { path: { project, space } }, body }));
}

export async function updateView(
  project: string,
  space: string,
  id: string,
  body: { kind: string; mode: string; title: string; config: ViewConfig; expectedVersion: number },
): Promise<DataView> {
  return unwrap(await api.PUT(ONE, { params: { path: { project, space, id } }, body }));
}

export async function deleteView(project: string, space: string, id: string): Promise<void> {
  await unwrap(await api.DELETE(ONE, { params: { path: { project, space, id } } }));
}

/** Two configs say the same thing: what a view saved and what the grid shows now. */
export function sameConfig(a: ViewConfig, b: ViewConfig): boolean {
  return JSON.stringify(normalized(a)) === JSON.stringify(normalized(b));
}

function normalized(config: ViewConfig): ViewConfig {
  return {
    q: config.q || undefined,
    sort: config.sort?.length ? config.sort : undefined,
    group: config.group || undefined,
    hidden: config.hidden?.length ? [...config.hidden].sort() : undefined,
    width: config.width && Object.keys(config.width).length ? config.width : undefined,
    colour: config.colour?.length ? config.colour : undefined,
    settings: config.settings && Object.keys(config.settings).length ? config.settings : undefined,
  };
}

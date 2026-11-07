/**
 * The pages a person went to last and the ones they starred (UI-90, T-3240), kept in their own
 * preferences on the server (`/api/v1/preferences`, scoped by their Keycloak `sub`), so they
 * follow the person to another browser. Read once, when something first asks. A star is written
 * at once: the stored preferences are read again and written back with only the stars changed,
 * so a theme or a layout saved elsewhere is kept. An opened page is not a write: the tab collects
 * them and reports them in one `POST /api/v1/preferences/recent` when it is hidden or closed,
 * which the server merges into what is stored.
 */
import { useEffect, useSyncExternalStore } from "react";
import { api } from "../api/client";
import type { components } from "../api/schema";

export type Place = components["schemas"]["Place"];
type Preferences = components["schemas"]["Preferences"];

const MAX_RECENT = 10;
const MAX_FAVOURITES = 50;

interface Places {
  recent: Place[];
  favourites: Place[];
}

let places: Places = { recent: [], favourites: [] };
let loading: Promise<void> | null = null;
/** One write at a time, in order: two quick changes never race each other's read. */
let writing: Promise<void> = Promise.resolve();
const listeners = new Set<() => void>();

function publish(next: Places): void {
  places = next;
  for (const listener of listeners) listener();
}

async function read(): Promise<Preferences | null> {
  try {
    const answer = await api.GET("/api/v1/preferences", {});
    return answer.data ?? null;
  } catch {
    return null;
  }
}

/** Reads the person's places once; a Portal without a preferences database keeps both empty. */
function load(): Promise<void> {
  loading ??= read().then((stored) => {
    if (stored) publish({ recent: stored.recent ?? [], favourites: stored.favourites ?? [] });
  });
  return loading;
}

/** Applies `change` to the stored places and saves them; the screen shows the change at once. */
function save(change: (current: Places) => Places): Promise<void> {
  publish(change(places));
  writing = writing.then(async () => {
    const stored = await read();
    if (!stored) return;
    const next = change({ recent: stored.recent ?? [], favourites: stored.favourites ?? [] });
    try {
      const answer = await api.PUT("/api/v1/preferences", { body: { ...stored, ...next } });
      if (answer.data) publish({ recent: answer.data.recent ?? [], favourites: answer.data.favourites ?? [] });
    } catch {
      // Unsaved: the screen keeps the change for this visit, the next visit reads what is stored.
    }
  });
  return writing;
}

/** The address of the page as a place: its path and query, without the language override. */
export function placeOf(location: { pathname: string; search: string }): string {
  const query = new URLSearchParams(location.search);
  query.delete("lang");
  const rest = query.toString();
  return rest ? `${location.pathname}?${rest}` : location.pathname;
}

/** Whether the address is one thing rather than a list: an item, a tab of one, a dataset. */
export function isItem(path: string): boolean {
  const segments = path.split("?")[0].split("/").filter(Boolean);
  return (segments[0] === "projects" && segments.length >= 4) || (segments[0] === "catalogue" && segments.length >= 2);
}

/** Pages opened since the last report, newest first. */
let unreported: Place[] = [];

const first = (place: Place, list: Place[], max: number) => [place, ...list.filter((p) => p.path !== place.path)].slice(0, max);

/** Puts `place` first in the recent pages, once; the server hears of it when the tab is hidden. */
export function remember(place: Place): Promise<void> {
  return load().then(() => {
    const top = places.recent[0];
    if (top && top.path === place.path && top.title === place.title) return;
    unreported = first(place, unreported, MAX_RECENT);
    publish({ ...places, recent: first(place, places.recent, MAX_RECENT) });
  });
}

/** Sends the pages opened since the last report; `keepalive` lets it finish as the tab closes. */
export function reportRecent(): Promise<void> {
  if (unreported.length === 0) return Promise.resolve();
  const opened = unreported;
  unreported = [];
  return api
    .POST("/api/v1/preferences/recent", { body: { places: opened }, keepalive: true })
    .then(() => undefined)
    .catch(() => undefined);
}

if (typeof document !== "undefined") {
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") void reportRecent();
  });
}

/** Stars the page, or takes its star away. */
export function toggleFavourite(place: Place): Promise<void> {
  return load().then(() =>
    save((current) => ({
      ...current,
      favourites: current.favourites.some((p) => p.path === place.path)
        ? current.favourites.filter((p) => p.path !== place.path)
        : [place, ...current.favourites].slice(0, MAX_FAVOURITES),
    })),
  );
}

/** The project a place is in, when it is in one. */
export function projectOfPlace(path: string): string | undefined {
  return /^\/projects\/([a-z0-9][a-z0-9-]*)(?:[/?]|$)/.exec(path)?.[1];
}

/** The person's places, read once and kept current. */
export function usePlaces(): Places {
  useEffect(() => {
    void load();
  }, []);
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => places,
  );
}

/** For tests: forget what was read, so one case cannot see another's places. */
export function resetPlaces(): void {
  places = { recent: [], favourites: [] };
  unreported = [];
  loading = null;
  writing = Promise.resolve();
}

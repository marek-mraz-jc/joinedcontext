import { vi } from "vitest";
import { ServerProblem } from "../server";
import type { HistoryRow, KeptReach, Me, NewOrigin, Origin, ReachApi } from "../server";

export const VIEWER: Me = { id: "u-viewer", name: "Vilja Viewer", roles: ["viewer"] };
export const STEWARD: Me = { id: "u-steward", name: "Sami Steward", roles: ["viewer", "steward"] };

export const BANDS = [
  { minutes: 10, areaKm2: 1.5, stops: 4 },
  { minutes: 20, areaKm2: 6.25, stops: 12 },
  { minutes: 30, areaKm2: 14, stops: 30 },
];

/** An origin someone kept, as the server answers it to `me`. */
export function origin(id: number, name: string, owner: string, published = false): Origin & { owner: string } {
  return { id, name, lon: 24.93, lat: 60.17, source: "network", bands: BANDS, published, mine: false, createdAt: "2030-10-07T06:00:00Z", owner };
}

/**
 * The App's server in memory, deciding as the real one does from the caller alone (`null` signed
 * out): a signed-in visitor keeps origins of their own, a steward publishes any and changes every
 * one; the published ones are everyone's to see; every write is history, a viewer's own rows only.
 */
export function memory(me: Me | null = VIEWER, kept: (Origin & { owner: string })[] = []) {
  const origins = kept;
  const log: (HistoryRow & { owner: string })[] = [];
  const steward = () => me?.roles.includes("steward") ?? false;
  const signedIn = () => {
    if (!me) throw new ServerProblem(401, "sign in to do this");
    return me;
  };
  const write = (action: HistoryRow["action"], target: string, summary: string) => {
    const caller = signedIn();
    log.unshift({ id: log.length + 1, at: "2030-10-07T06:00:00Z", actorName: caller.name, action, target, summary, owner: caller.id });
  };
  const changeable = (id: number) => {
    const caller = signedIn();
    const found = origins.find((o) => o.id === id && (o.owner === caller.id || steward()));
    if (!found) throw new ServerProblem(404, "no such record, or not yours to change");
    return found;
  };
  const shown = (o: Origin & { owner: string }): Origin => {
    const { owner, ...rest } = o;
    return { ...rest, mine: me !== null && owner === me.id };
  };
  const api = {
    reach: vi.fn(
      async (stop: string): Promise<KeptReach> => ({ version: "v1", stop, bands: BANDS, cached: true, stale: false, url: "https://store.example/t.geojson" }),
    ),
    refresh: vi.fn(async () => ({ version: "v1", stops: 4, routes: 2, changed: false })),
    me: vi.fn(async () => me),
    origins: vi.fn(async () => origins.filter((o) => o.published || o.owner === me?.id || steward()).map(shown)),
    keep: vi.fn(async (wanted: NewOrigin) => {
      const caller = signedIn();
      if (!wanted.name.trim()) throw new ServerProblem(400, "a name is 1 to 80 characters, on one line");
      const made = { ...wanted, id: origins.length + 1, published: false, mine: true, createdAt: "2030-10-07T06:00:00Z", owner: caller.id };
      origins.unshift(made);
      write("insert", `origins/${made.id}`, `kept the origin ${made.name}`);
      return shown(made);
    }),
    rename: vi.fn(async (id: number, name: string) => {
      changeable(id).name = name;
      write("update", `origins/${id}`, `renamed an origin to ${name}`);
    }),
    forget: vi.fn(async (id: number) => {
      changeable(id);
      origins.splice(
        origins.findIndex((o) => o.id === id),
        1,
      );
      write("delete", `origins/${id}`, "deleted an origin");
    }),
    publish: vi.fn(async (id: number, published: boolean) => {
      signedIn();
      if (!steward()) throw new ServerProblem(403, "this needs the role steward");
      changeable(id).published = published;
      write("update", `origins/${id}`, published ? "published an origin" : "withdrew an origin from the shared set");
    }),
    history: vi.fn(async () => {
      const caller = signedIn();
      return log.filter((row) => steward() || row.owner === caller.id).map(({ owner: _, ...row }) => row);
    }),
  } satisfies ReachApi;
  return Object.assign(api, { kept: origins, log });
}

/**
 * The App's server in memory, for the page's tests (T-3827): no model and no inputs kept until a
 * test keeps them. Who calls is `who` (signed out unless a test names someone), and it decides as
 * the real server does: a steward sets and resets a station's inputs, anyone reads them, the
 * history shows a viewer their own rows and a steward everyone's.
 */
import { vi } from "vitest";
import { Problem } from "../server";
import type { HistoryRow, Inputs, Me, Models, NewInputs, Server } from "../server";

export const VIEWER: Me = { id: "u-viewer", name: "Vilja Viewer", roles: ["viewer"] };
export const STEWARD: Me = { id: "u-steward", name: "Sami Steward", roles: ["viewer", "steward"] };
export const DEFAULTS: NewInputs = { weatherStation: null, radiusKm: 10, days: 7, note: "" };

export type Memory = Server & {
  who: Me | null;
  kept: Map<string, Inputs>;
  log: (HistoryRow & { owner: string })[];
  /** What `models` answers. */
  modelsOf: (station: string) => Models;
};

export function memory(who: Me | null = null): Memory {
  let next = 100;
  const m = {
    who,
    kept: new Map<string, Inputs>(),
    log: [],
    modelsOf: () => ({ models: [], stale: false }),
  } as unknown as Memory;
  const signedIn = () => {
    if (!m.who) throw new Problem(401, "sign in to do this");
    return m.who;
  };
  const steward = () => {
    if (!signedIn().roles.includes("steward")) throw new Problem(403, "this needs the role steward");
    return signedIn();
  };
  const read = (station: string): Inputs => m.kept.get(station) ?? { ...DEFAULTS, station, custom: false, updatedBy: null, updatedAt: null };
  m.models = vi.fn(async (station: string) => m.modelsOf(station));
  m.snapshotUrl = vi.fn(async (id: number) => `https://store.test/training/${id}.json`);
  m.me = vi.fn(async () => m.who);
  m.inputs = vi.fn(async (station: string) => read(station));
  m.setInputs = vi.fn(async (station: string, inputs: NewInputs) => {
    const caller = steward();
    if (inputs.days < 1 || inputs.days > 7) throw new Problem(400, "a model trains on 1 to 7 days");
    const kept: Inputs = { ...inputs, station, custom: true, updatedBy: caller.name, updatedAt: new Date(Date.now()).toISOString() };
    m.kept.set(station, kept);
    m.log.unshift({ id: next++, at: kept.updatedAt!, actorName: caller.name, action: "update", target: `inputs/${station}`, summary: `${inputs.days} days`, owner: caller.id });
    return kept;
  });
  m.resetInputs = vi.fn(async (station: string) => {
    const caller = steward();
    if (!m.kept.delete(station)) throw new Problem(404, "the station has the defaults already");
    m.log.unshift({ id: next++, at: new Date(Date.now()).toISOString(), actorName: caller.name, action: "delete", target: `inputs/${station}`, summary: "back to the defaults", owner: caller.id });
  });
  m.history = vi.fn(async () => {
    const caller = signedIn();
    return m.log.filter((row) => row.owner === caller.id || caller.roles.includes("steward")).map(({ owner: _, ...row }) => row);
  });
  return m;
}

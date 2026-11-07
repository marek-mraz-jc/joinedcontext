import { vi } from "vitest";
import { ServerProblem } from "../plans";
import type { HistoryRow, Levels, Me, NewPlan, PlanSummary, PlansApi, SavedPlan } from "../plans";

export const STOP = { id: "urn:ngsi-ld:BikeHireDockingStation:001", name: "Kamppi", at: [24.93, 60.17] as [number, number], action: "pick" as const, bikes: 5, load: 5, legKm: 0 };

export const VIEWER: Me = { id: "u-viewer", name: "Vilja Viewer", roles: ["viewer"] };
export const STEWARD: Me = { id: "u-steward", name: "Sami Steward", roles: ["viewer", "steward"] };

/**
 * The App's server in memory, deciding as the real one does from the caller alone: a viewer sees
 * and changes their own plans, a steward everyone's and the levels; every write is history.
 */
export function memory(me: Me | null = VIEWER): PlansApi & { saved: (SavedPlan & { owner: string })[]; kept: Levels; log: HistoryRow[] } {
  const saved: (SavedPlan & { owner: string })[] = [];
  const log: HistoryRow[] = [];
  const kept: Levels = { low: 0.15, high: 0.85, target: 0.5 };
  const steward = () => me?.roles.includes("steward") ?? false;
  const signedIn = () => {
    if (!me) throw new ServerProblem(401, "sign in to do this");
    return me;
  };
  const write = (action: HistoryRow["action"], target: string, summary: string) =>
    log.unshift({ id: log.length + 1, at: "2026-10-10T08:00:00Z", actorName: signedIn().name, action, target, summary });
  const own = (id: number) => {
    const caller = signedIn();
    const plan = saved.find((p) => p.id === id && (p.owner === caller.id || steward()));
    if (!plan) throw new ServerProblem(404, "no such plan");
    return plan;
  };
  const summary = (p: SavedPlan & { owner: string }): PlanSummary => ({
    id: p.id,
    operator: p.operator,
    vanCapacity: p.vanCapacity,
    km: p.km,
    moved: p.moved,
    stopCount: p.stops.length,
    createdAt: p.createdAt,
    drives: p.drives.length,
    mine: p.owner === me?.id,
  });
  return {
    saved,
    kept,
    log,
    list: vi.fn(async (operator: string) => {
      const caller = signedIn();
      return saved.filter((p) => (p.owner === caller.id || steward()) && (!operator.trim() || p.operator === operator.trim())).map(summary);
    }),
    save: vi.fn(async (plan: NewPlan) => {
      const caller = signedIn();
      const made = { ...plan, id: saved.length + 1, stops: [STOP], km: 1.5, moved: 5, createdAt: "2026-10-10T08:00:00Z", drives: [], owner: caller.id };
      saved.push(made);
      write("insert", `plans/${made.id}`, `saved a plan for ${plan.operator}`);
      return made;
    }),
    get: vi.fn(async (id: number) => own(id)),
    remove: vi.fn(async (id: number) => {
      own(id);
      saved.splice(
        saved.findIndex((p) => p.id === id),
        1,
      );
      write("delete", `plans/${id}`, "removed the plan");
    }),
    drive: vi.fn(async (id: number, stops: string[]) => {
      const record = { id: 1, stops, km: null, note: null, drivenAt: "2026-10-10T09:00:00Z" };
      own(id).drives.push(record);
      write("insert", `drives/${record.id}`, "recorded a drive");
      return record;
    }),
    sheetUrl: vi.fn(async (id: number) => {
      own(id);
      return "https://store.example/apps/s1/x/sheets/1.csv?X-Amz-Signature=x";
    }),
    handOver: vi.fn(async (id: number, operator: string) => {
      signedIn();
      if (!steward()) throw new ServerProblem(403, "this needs the role steward");
      own(id).operator = operator;
      write("update", `plans/${id}`, `handed the plan to ${operator}`);
    }),
    me: vi.fn(async () => me),
    levels: vi.fn(async () => ({ ...kept })),
    setLevels: vi.fn(async (next: Levels) => {
      signedIn();
      if (!steward()) throw new ServerProblem(403, "this needs the role steward");
      Object.assign(kept, next);
      write("update", "levels/1", "levels changed");
      return { ...kept };
    }),
    history: vi.fn(async () => {
      const caller = signedIn();
      return log.filter((row) => steward() || row.actorName === caller.name);
    }),
  };
}

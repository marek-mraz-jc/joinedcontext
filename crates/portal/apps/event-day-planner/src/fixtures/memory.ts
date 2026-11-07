import { vi } from "vitest";
import { ServerProblem } from "../share";
import type { FeaturedEvent, HistoryRow, KeptDay, Me, ShareApi, SharedDay } from "../share";

export const VIEWER: Me = { id: "u-viewer", name: "Vilja Viewer", roles: ["viewer"] };
export const STEWARD: Me = { id: "u-steward", name: "Sami Steward", roles: ["viewer", "steward"] };

/** A shared day as the server answers it: the picks by the end of their id. */
const sharedDay = (code: string, day: string, picks: string[], mine: boolean): SharedDay => ({
  code,
  day,
  lang: "en",
  picks,
  items: [],
  conflicts: [],
  walkKm: 0,
  walkMinutes: 0,
  mine,
});

/**
 * The App's server in memory, deciding as the real one does from the caller alone (`null` signed
 * out): anyone shares a day, a signed-in one keeps theirs, a steward features events; every write
 * is history, which a viewer reads only their own rows of.
 */
export function memory(me: Me | null = VIEWER) {
  const days: (KeptDay & { owner: string })[] = [];
  const featured: FeaturedEvent[] = [];
  const log: (HistoryRow & { owner: string })[] = [];
  const steward = () => me?.roles.includes("steward") ?? false;
  const signedIn = () => {
    if (!me) throw new ServerProblem(401, "sign in to do this");
    return me;
  };
  const write = (action: HistoryRow["action"], target: string, summary: string) => {
    const caller = signedIn();
    log.unshift({ id: log.length + 1, at: "2030-10-20T05:00:00Z", actorName: caller.name, action, target, summary, owner: caller.id });
  };
  const stewardOnly = () => {
    signedIn();
    if (!steward()) throw new ServerProblem(403, "this needs the role steward");
  };
  const api = {
    share: vi.fn(async (day: string, ids: string[], _lang: string) => {
      const code = `abc123def${String(456 + days.length)}`;
      const picks = ids.map((id) => id.slice(id.lastIndexOf(":") + 1));
      if (me) {
        days.push({ id: days.length + 1, code, day, events: ids.length, createdAt: "2030-10-20T05:00:00Z", owner: me.id });
        write("insert", `itineraries/${days.length}`, `shared ${day}`);
      }
      return sharedDay(code, day, picks, me !== null);
    }),
    get: vi.fn(async (code: string) => sharedDay(code, "2030-10-20", ["helsinki-agf2", "helsinki-agf3"], days.some((d) => d.code === code && d.owner === me?.id))),
    icsUrl: vi.fn(async () => "https://store.example/x.ics"),
    me: vi.fn(async () => me),
    days: vi.fn(async () => {
      const caller = signedIn();
      return days.filter((d) => d.owner === caller.id).map(({ owner: _, ...day }) => day);
    }),
    forgetDay: vi.fn(async (id: number) => {
      const caller = signedIn();
      const at = days.findIndex((d) => d.id === id && d.owner === caller.id);
      if (at < 0) throw new ServerProblem(404, "no such day");
      days.splice(at, 1);
      write("delete", `itineraries/${id}`, "deleted a shared day");
    }),
    featured: vi.fn(async () => featured.map((one) => ({ ...one }))),
    feature: vi.fn(async (eventId: string, note: string) => {
      stewardOnly();
      if (featured.some((one) => one.eventId === eventId)) throw new ServerProblem(409, "already featured");
      const made = { id: featured.length + 1, eventId, note };
      featured.push(made);
      write("insert", `featured/${made.id}`, `featured ${eventId}`);
      return { ...made };
    }),
    changeNote: vi.fn(async (id: number, note: string) => {
      stewardOnly();
      const one = featured.find((f) => f.id === id);
      if (!one) throw new ServerProblem(404, "no such featured event");
      one.note = note;
      write("update", `featured/${id}`, "changed the note");
    }),
    unfeature: vi.fn(async (id: number) => {
      stewardOnly();
      const at = featured.findIndex((f) => f.id === id);
      if (at < 0) throw new ServerProblem(404, "no such featured event");
      featured.splice(at, 1);
      write("delete", `featured/${id}`, "no longer featured");
    }),
    history: vi.fn(async () => {
      const caller = signedIn();
      return log.filter((row) => steward() || row.owner === caller.id).map(({ owner: _, ...row }) => row);
    }),
  } satisfies ShareApi;
  return Object.assign(api, { days_: days, featured_: featured, log });
}

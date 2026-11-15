/**
 * The App's server in memory, for the page's tests (T-3348): the fixture's readings kept as the
 * server keeps them, hourly means with humidity in per cent, served for the chosen stations and
 * period; comparisons kept under codes; an export answered with a URL. Who calls is `who`
 * (signed out unless a test names someone), and it decides as the real server does (T-3827): a
 * reader's own places, a steward's marks of faulty readings, the history of both.
 */
import { vi } from "vitest";
import { HISTORY, NOW } from "../fixtures/stations";
import { ServerProblem } from "../problem";
import type { AirApi, Comparison, Fault, HistoryRow, Me, Place } from "../server";
import { hourlyOf } from "./hourly";

export const VIEWER: Me = { id: "u-viewer", name: "Vilja Viewer", roles: ["viewer"] };
export const STEWARD: Me = { id: "u-steward", name: "Sami Steward", roles: ["viewer", "steward"] };

const HOUR = 3_600_000;
const DAY = 24 * HOUR;

type Memory = AirApi & {
  /** The readings the server holds, as the broker's temporalValues answer them. */
  readings: ReadonlyArray<Record<string, unknown>>;
  saved: Map<string, Comparison>;
  /** Who calls; `null` signed out. */
  who: Me | null;
  kept: (Place & { owner: string })[];
  marks: Fault[];
  log: (HistoryRow & { owner: string })[];
  /** Back to the fixture's history, nothing saved, nobody signed in, fresh call records. */
  reset(): void;
};

/** The server's routes as fresh mocks over `memory`'s history and saved comparisons. */
function fill(memory: Partial<Memory> & Pick<Memory, "readings" | "saved">): Memory {
  memory.series = vi.fn(async (air: string, weather: string, days: number) => {
    const from = Math.floor((NOW - days * DAY) / HOUR) * HOUR;
    return { air: hourlyOf(memory.readings, air, from), weather: hourlyOf(memory.readings, weather, from) };
  });
  memory.save = vi.fn(async (comparison: Comparison) => {
    const code = `c${String(memory.saved.size + 1).padStart(11, "0")}`;
    memory.saved.set(code, comparison);
    return { code, name: comparison.name };
  });
  memory.open = vi.fn(async (code: string) => {
    const found = memory.saved.get(code);
    if (!found) throw new ServerProblem(404, "no comparison is saved under this link");
    return { ...found, code };
  });
  memory.exportUrl = vi.fn(async () => "https://store.example/apps/s1/x/exports/c00000000001.csv?X-Amz-Signature=x");
  const m = memory as Memory;
  const signedIn = () => {
    if (!m.who) throw new ServerProblem(401, "sign in to do this");
    return m.who;
  };
  const steward = () => {
    if (!signedIn().roles.includes("steward")) throw new ServerProblem(403, "this needs the role steward");
  };
  const write = (action: HistoryRow["action"], target: string, summary: string) => {
    const caller = signedIn();
    m.log.unshift({ id: m.log.length + 1, at: new Date(NOW).toISOString(), actorName: caller.name, action, target, summary, owner: caller.id });
  };
  const own = (id: number) => {
    const caller = signedIn();
    const found = m.kept.find((p) => p.id === id && p.owner === caller.id);
    if (!found) throw new ServerProblem(404, "no such record, or not yours to change");
    return found;
  };
  m.me = vi.fn(async () => m.who);
  m.places = vi.fn(async () => {
    const caller = signedIn();
    return m.kept.filter((p) => p.owner === caller.id).map(({ owner: _, ...p }) => p);
  });
  m.addPlace = vi.fn(async (place) => {
    const caller = signedIn();
    const made = { ...place, id: m.kept.length + 1, latest: null, latestHour: null, owner: caller.id };
    m.kept.unshift(made);
    write("insert", `places/${made.id}`, `kept the place ${place.name}`);
    const { owner: _, ...shown } = made;
    return shown;
  });
  m.changePlace = vi.fn(async (id, name, threshold) => {
    Object.assign(own(id), { name, threshold });
    write("update", `places/${id}`, `changed the place ${name}`);
  });
  m.removePlace = vi.fn(async (id) => {
    own(id);
    m.kept.splice(
      m.kept.findIndex((p) => p.id === id),
      1,
    );
    write("delete", `places/${id}`, "deleted a place");
  });
  m.faults = vi.fn(async (station) => m.marks.filter((f) => f.station === station));
  m.markFault = vi.fn(async (fault) => {
    steward();
    const made = { ...fault, id: m.marks.length + 1 };
    m.marks.unshift(made);
    write("insert", `faults/${made.id}`, `marked faulty: ${fault.note}`);
    return made;
  });
  m.unmarkFault = vi.fn(async (id) => {
    steward();
    const at = m.marks.findIndex((f) => f.id === id);
    if (at < 0) throw new ServerProblem(404, "no such record, or not yours to change");
    m.marks.splice(at, 1);
    write("delete", `faults/${id}`, "took back a faulty mark");
  });
  m.history = vi.fn(async () => {
    const caller = signedIn();
    const all = caller.roles.includes("steward");
    return m.log.filter((row) => all || row.owner === caller.id).map(({ owner: _, ...row }) => row);
  });
  return m;
}

export function memoryServer(): Memory {
  const memory: Partial<Memory> & Pick<Memory, "readings" | "saved"> = { readings: HISTORY, saved: new Map(), who: null, kept: [], marks: [], log: [] };
  memory.reset = () => {
    memory.readings = HISTORY;
    memory.saved.clear();
    memory.who = null;
    memory.kept = [];
    memory.marks = [];
    memory.log = [];
    fill(memory);
  };
  return fill(memory);
}

/** The one the page's module gets in the tests that mock `../server`. */
export const server = memoryServer();

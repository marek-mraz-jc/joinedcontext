/**
 * The App's server in memory, for the page's tests (T-3350): nothing kept until a test keeps it.
 * Who calls is `who` (signed out unless a test names someone), and it decides as the real server
 * does (T-3827): a steward publishes and withdraws, a reader follows their own indicators, the
 * history shows a viewer their own rows and a steward everyone's.
 */
import { vi } from "vitest";
import { ServerProblem } from "./problem";
import type { Follow, HistoryRow, KeptForecast, KpiApi, Me, PublishedForecast } from "./server";

export const VIEWER: Me = { id: "u-viewer", name: "Vilja Viewer", roles: ["viewer"] };
export const STEWARD: Me = { id: "u-steward", name: "Sami Steward", roles: ["viewer", "steward"] };

export type Memory = KpiApi & {
  kept: Map<string, KeptForecast[]>;
  who: Me | null;
  /** Published forecasts by indicator, newest first. */
  publications: Map<string, PublishedForecast[]>;
  followed: (Follow & { owner: string })[];
  log: (HistoryRow & { owner: string })[];
  reset(): void;
};

const DAY = 86_400_000;

function fill(memory: Partial<Memory> & Pick<Memory, "kept">): Memory {
  const m = memory as Memory;
  memory.record = vi.fn(async () => ({ recorded: 0, already: true }));
  memory.list = vi.fn(async (kpi: string) => memory.kept.get(kpi) ?? []);
  memory.reportUrl = vi.fn(async (month: string) => `https://store.example/apps/s1/x/reports/${month}.csv?X-Amz-Signature=x`);
  const signedIn = () => {
    if (!m.who) throw new ServerProblem(401, "sign in to do this");
    return m.who;
  };
  const steward = () => {
    if (!signedIn().roles.includes("steward")) throw new ServerProblem(403, "this needs the role steward");
    return signedIn();
  };
  let next = 100;
  const write = (action: HistoryRow["action"], target: string, summary: string) => {
    const caller = signedIn();
    m.log.unshift({ id: next++, at: new Date(Date.now()).toISOString(), actorName: caller.name, action, target, summary, owner: caller.id });
  };
  memory.me = vi.fn(async () => m.who);
  memory.published = vi.fn(async (kpi: string) => m.publications.get(kpi) ?? []);
  memory.publish = vi.fn(async (kpi: string, days: number, horizon: number, note: string) => {
    const caller = steward();
    const now = Date.now();
    const made: PublishedForecast = {
      id: next++,
      days,
      horizon,
      note,
      step: DAY,
      points: Array.from({ length: horizon }, (_, i) => ({ t: now + (i + 1) * DAY, v: 100 + i, lo: 90 + i, hi: 110 + i })),
      publisherName: caller.name,
      publishedAt: new Date(now).toISOString(),
    };
    m.publications.set(kpi, [made, ...(m.publications.get(kpi) ?? [])]);
    write("insert", `published/${made.id}`, `published a forecast: ${note}`);
    return made;
  });
  memory.withdraw = vi.fn(async (id: number) => {
    steward();
    for (const [kpi, list] of m.publications) {
      if (list.some((f) => f.id === id)) {
        m.publications.set(
          kpi,
          list.filter((f) => f.id !== id),
        );
        write("delete", `published/${id}`, "withdrew a published forecast");
        return;
      }
    }
    throw new ServerProblem(404, "no such record, or not yours to change");
  });
  memory.follows = vi.fn(async () => {
    const caller = signedIn();
    return m.followed.filter((f) => f.owner === caller.id).map(({ owner: _, ...f }) => f);
  });
  memory.follow = vi.fn(async (kpi: string) => {
    const caller = signedIn();
    if (m.followed.some((f) => f.owner === caller.id && f.kpi === kpi)) throw new ServerProblem(409, "you follow this indicator already");
    const made = { id: next++, kpi, owner: caller.id };
    m.followed.unshift(made);
    write("insert", `follows/${made.id}`, "followed an indicator");
    return { id: made.id, kpi };
  });
  memory.unfollow = vi.fn(async (id: number) => {
    const caller = signedIn();
    const at = m.followed.findIndex((f) => f.id === id && f.owner === caller.id);
    if (at < 0) throw new ServerProblem(404, "no such record, or not yours to change");
    m.followed.splice(at, 1);
    write("delete", `follows/${id}`, "stopped following an indicator");
  });
  memory.history = vi.fn(async () => {
    const caller = signedIn();
    const all = caller.roles.includes("steward");
    return m.log.filter((row) => all || row.owner === caller.id).map(({ owner: _, ...row }) => row);
  });
  return m;
}

export function memoryServer(): Memory {
  const memory: Partial<Memory> & Pick<Memory, "kept"> = { kept: new Map(), who: null, publications: new Map(), followed: [], log: [] };
  memory.reset = () => {
    memory.kept.clear();
    memory.who = null;
    memory.publications = new Map();
    memory.followed = [];
    memory.log = [];
    fill(memory);
  };
  return fill(memory);
}

/** The one the page's module gets in the tests that mock `./server`. */
export const server = memoryServer();

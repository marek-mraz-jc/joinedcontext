/**
 * The App's server in memory, for the page's tests (T-3351): it keeps what is saved, says which
 * reports got a picture, and fails where a test asks it to; the roles' half is `memory` (T-3828).
 */
import type { NewReport, Report, Server, Weeks } from "./server";
import { Problem } from "./server";
import { memory } from "./testing/server";
import type { Memory } from "./testing/server";

export interface FakeServer extends Server, Memory {
  saved: Report[];
  pictures: Map<number, Blob>;
  opened: string[];
}

export function fakeServer(
  weeks: Weeks = { weeks: [], stale: false },
  fail: { weeks?: string; reports?: string; save?: string; attach?: string; picture?: string } = {},
  roles: Memory = memory(),
): FakeServer {
  // The roles' half keeps its state on itself, so it is extended, never copied.
  const fake = Object.assign(roles, {
    saved: [],
    pictures: new Map(),
    opened: [],
    weeks: async () => {
      if (fail.weeks) throw new Problem(502, fail.weeks);
      return weeks;
    },
    reports: async () => {
      if (fail.reports) throw new Problem(503, fail.reports);
      return [...fake.saved].reverse();
    },
    save: async (report: NewReport) => {
      if (fail.save) throw new Problem(400, fail.save);
      const saved: Report = { ...report, id: fake.saved.length + 1, has_snapshot: false, created_at: "2030-10-21T09:00:00Z" };
      fake.saved.push(saved);
      return saved;
    },
    attach: async (id: number, png: Blob) => {
      if (fail.attach) throw new Problem(403, fail.attach);
      fake.pictures.set(id, png);
      const report = fake.saved.find((r) => r.id === id);
      if (report) report.has_snapshot = true;
    },
    snapshotUrl: async (id: number) => {
      if (fail.picture) throw new Problem(404, fail.picture);
      const url = `https://store.test/apps/0/x/reports/${id}/map.png`;
      fake.opened.push(url);
      return url;
    },
  }) as FakeServer;
  return fake;
}

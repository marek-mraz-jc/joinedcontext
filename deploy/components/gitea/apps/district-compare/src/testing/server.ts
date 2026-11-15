/**
 * The App's server in memory, for the page's tests (T-3828): nothing kept until a test keeps it.
 * Who calls is `who` (signed out unless a test names someone), and it decides as the real server
 * does: a reader keeps, changes and deletes their own comparisons and opens anyone's by its share
 * token (the id's digits, padded to 64); a
 * steward pins one for everyone; the history shows a viewer their own rows and a steward everyone's.
 */
import { vi } from "vitest";
import { Problem } from "../server";
import type { Change, Comparison, Me, NewComparison, Server } from "../server";

export const VIEWER: Me = { id: "u-viewer", name: "Vilja Viewer", roles: ["viewer"] };
export const STEWARD: Me = { id: "u-steward", name: "Sami Steward", roles: ["viewer", "steward"] };

export type Memory = Server & {
  who: Me | null;
  kept: (Comparison & { owner: string })[];
  pins: Comparison[];
  log: (Change & { owner: string })[];
};

/** The share token the memory server gives comparison `id`. */
export const token = (id: number) => String(id).padStart(64, "0");

export function memory(): Memory {
  let next = 100;
  const m = { who: null, kept: [], pins: [], log: [] } as unknown as Memory;
  const signedIn = () => {
    if (!m.who) throw new Problem(401, "sign in to do this");
    return m.who;
  };
  const steward = () => {
    if (!signedIn().roles.includes("steward")) throw new Problem(403, "this needs the role steward");
    return signedIn();
  };
  const write = (action: Change["action"], target: string, summary: string) => {
    const caller = signedIn();
    m.log.unshift({ id: next++, at: new Date(Date.now()).toISOString(), actorName: caller.name, action, target, summary, owner: caller.id });
  };
  const own = (id: number) => {
    const found = m.kept.find((c) => c.id === id && c.owner === signedIn().id);
    if (!found) throw new Problem(404, "no such record, or not yours to change");
    return found;
  };
  const plain = ({ owner: _, ...c }: Comparison & { owner: string }): Comparison => c;
  m.history = vi.fn(async () => ({ days: [], stale: false }));
  m.files = vi.fn(async () => ({ geojson: "https://store.test/b.geojson", licence: "https://store.test/LICENCE.txt" }));
  m.me = vi.fn(async () => m.who);
  m.comparisons = vi.fn(async () => m.kept.filter((c) => c.owner === signedIn().id).map(plain));
  m.keep = vi.fn(async (c: NewComparison) => {
    const caller = signedIn();
    const id = next++;
    const made = { ...c, id, share: token(id), ownerName: caller.name, owner: caller.id };
    m.kept.unshift(made);
    write("insert", `comparisons/${made.id}`, `kept the comparison ${c.name}`);
    return plain(made);
  });
  m.shared = vi.fn(async (share: string) => {
    const caller = signedIn();
    const found = m.kept.find((c) => c.share === share);
    if (!found) throw new Problem(404, "no such comparison");
    return { ...plain(found), mine: found.owner === caller.id };
  });
  m.change = vi.fn(async (id: number, c: NewComparison) => {
    Object.assign(own(id), c);
    write("update", `comparisons/${id}`, `changed the comparison ${c.name}`);
  });
  m.forget = vi.fn(async (id: number) => {
    const found = own(id);
    m.kept = m.kept.filter((c) => c !== found);
    write("delete", `comparisons/${id}`, "deleted a comparison");
  });
  m.pinned = vi.fn(async () => m.pins[0] ?? null);
  m.pin = vi.fn(async (c: NewComparison) => {
    const caller = steward();
    const made = { ...c, id: next++, ownerName: caller.name };
    m.pins.unshift(made);
    write("insert", `pins/${made.id}`, `pinned ${c.name} for everyone`);
    return made;
  });
  m.unpin = vi.fn(async () => {
    steward();
    const gone = m.pins.shift();
    if (!gone) throw new Problem(404, "nothing is pinned");
    write("delete", `pins/${gone.id}`, "took back the pinned comparison");
  });
  m.changes = vi.fn(async () => {
    const caller = signedIn();
    return m.log.filter((row) => row.owner === caller.id || caller.roles.includes("steward")).map(({ owner: _, ...row }) => row);
  });
  return m;
}

/**
 * The role half of the App's server in memory (T-3827): who calls is `who` (signed out unless a
 * test names someone), and it decides as the real server does: a steward resolves, reopens and
 * corrects; the history shows a viewer their own rows and a steward everyone's. `corrected` keeps
 * what a correction wrote, as the Endpoint would have.
 */
import { vi } from "vitest";
import { Problem } from "../server";
import type { HistoryRow, Me, Resolution, Server } from "../server";

export const VIEWER: Me = { id: "u-viewer", name: "Vilja Viewer", roles: ["viewer"] };
export const STEWARD: Me = { id: "u-steward", name: "Sami Steward", roles: ["viewer", "steward"] };

type RoleServer = Pick<Server, "me" | "resolutions" | "resolve" | "reopen" | "correct" | "history">;

export interface Roles extends RoleServer {
  who: Me | null;
  resolved: Resolution[];
  corrected: { entity: string; attribute: string; value: unknown }[];
  log: (HistoryRow & { owner: string })[];
}

export function roles(who: Me | null = null): Roles {
  const m = { who, resolved: [], corrected: [], log: [] } as unknown as Roles;
  let next = 100;
  const steward = () => {
    if (!m.who) throw new Problem(401, "sign in to do this");
    if (!m.who.roles.includes("steward")) throw new Problem(403, "this needs the role steward");
    return m.who;
  };
  const write = (caller: Me, action: HistoryRow["action"], target: string, summary: string) =>
    m.log.unshift({ id: next++, at: new Date(Date.now()).toISOString(), actorName: caller.name, action, target, summary, owner: caller.id });
  const resolve = (caller: Me, entity: string, attribute: string, note: string, corrected: boolean): Resolution => {
    const made = { id: next++, entity, attribute, note, corrected, resolverName: caller.name, resolvedAt: new Date(Date.now()).toISOString() };
    m.resolved.unshift(made);
    write(caller, "insert", `resolutions/${made.id}`, `resolved ${attribute} of ${entity}: ${note}`);
    return made;
  };
  const open = (entity: string, attribute: string) => !m.resolved.some((r) => r.entity === entity && r.attribute === attribute);
  m.me = vi.fn(async () => m.who);
  m.resolutions = vi.fn(async () => [...m.resolved]);
  m.resolve = vi.fn(async (entity: string, attribute: string, note: string) => {
    const caller = steward();
    if (!open(entity, attribute)) throw new Problem(409, "this finding is resolved already");
    return resolve(caller, entity, attribute, note, false);
  });
  m.reopen = vi.fn(async (id: number) => {
    const caller = steward();
    const at = m.resolved.findIndex((r) => r.id === id);
    if (at < 0) throw new Problem(404, "no such record, or not yours to change");
    m.resolved.splice(at, 1);
    write(caller, "delete", `resolutions/${id}`, "reopened a finding");
  });
  m.correct = vi.fn(async (entity: string, attribute: string, value: string | number | boolean, note: string) => {
    const caller = steward();
    m.corrected.push({ entity, attribute, value });
    write(caller, "edit", entity, `corrected ${attribute}: ${note}`);
    return open(entity, attribute) ? resolve(caller, entity, attribute, note, true) : null;
  });
  m.history = vi.fn(async () => {
    if (!m.who) throw new Problem(401, "sign in to do this");
    const caller = m.who;
    const all = caller.roles.includes("steward");
    return m.log.filter((row) => all || row.owner === caller.id).map(({ owner: _, ...row }) => row);
  });
  return m;
}

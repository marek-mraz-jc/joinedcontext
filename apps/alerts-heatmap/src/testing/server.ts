/**
 * The role half of the App's server in memory (T-3828), deciding as the real one does: filters are
 * the signed-in reader's own, only a steward acknowledges, one acknowledgement per alert, and the
 * history shows a steward everyone's changes and anyone else their own.
 */
import { Problem } from "../server";
import type { Ack, Change, Filter, Me, Server } from "../server";

export const VIEWER: Me = { id: "u-viewer", name: "Vera Viewer", roles: ["viewer"] };
export const STEWARD: Me = { id: "u-steward", name: "Sara Steward", roles: ["steward", "viewer"] };

type Roles = Pick<Server, "me" | "filters" | "keepFilter" | "forgetFilter" | "acks" | "acknowledge" | "unacknowledge" | "changes">;

export interface Memory extends Roles {
  /** Who calls; `null` is signed out. */
  who: Me | null;
  kept: Array<Filter & { owner: string }>;
  given: Ack[];
  audit: Array<Change & { actorId: string }>;
  /** Names a method that fails with this text, as a refused or broken call would. */
  fail: Partial<Record<keyof Roles, string>>;
}

export function memory(who: Me | null = null): Memory {
  let next = 1;
  const signedIn = () => {
    if (!mem.who) throw new Problem(401, "sign in first");
    return mem.who;
  };
  const failing = (name: keyof Roles) => {
    if (mem.fail[name]) throw new Problem(503, mem.fail[name]!);
  };
  const log = (actor: Me, action: Change["action"], target: string, summary: string) =>
    mem.audit.push({ id: next++, at: "2030-10-21T09:00:00Z", actorId: actor.id, actorName: actor.name, action, target, summary });
  const mem: Memory = {
    who,
    kept: [],
    given: [],
    audit: [],
    fail: {},
    me: async () => {
      failing("me");
      return mem.who;
    },
    filters: async () => {
      failing("filters");
      const me = signedIn();
      return mem.kept.filter((filter) => filter.owner === me.id).reverse().map(({ id, name, view }) => ({ id, name, view }));
    },
    keepFilter: async ({ name, view }) => {
      failing("keepFilter");
      const me = signedIn();
      const filter = { id: next++, name, view };
      mem.kept.push({ ...filter, owner: me.id });
      log(me, "insert", `filters/${filter.id}`, `kept the filter ${name}`);
      return filter;
    },
    forgetFilter: async (id) => {
      failing("forgetFilter");
      const me = signedIn();
      const at = mem.kept.findIndex((filter) => filter.id === id && filter.owner === me.id);
      if (at < 0) throw new Problem(404, "no such filter");
      mem.kept.splice(at, 1);
      log(me, "delete", `filters/${id}`, "deleted a filter");
    },
    acks: async () => {
      failing("acks");
      return [...mem.given].reverse();
    },
    acknowledge: async (alert, comment) => {
      failing("acknowledge");
      const me = signedIn();
      if (!me.roles.includes("steward")) throw new Problem(403, "a steward acknowledges alerts");
      if (mem.given.some((ack) => ack.alert === alert)) throw new Problem(409, "the alert is acknowledged already; take that back first");
      const ack: Ack = { id: next++, alert, comment, ownerName: me.name, at: "2030-10-21T09:00:00Z" };
      mem.given.push(ack);
      log(me, "insert", `acks/${ack.id}`, `acknowledged ${alert.split(":").pop()}: ${comment}`);
      return ack;
    },
    unacknowledge: async (id) => {
      failing("unacknowledge");
      const me = signedIn();
      if (!me.roles.includes("steward")) throw new Problem(403, "a steward takes acknowledgements back");
      const at = mem.given.findIndex((ack) => ack.id === id);
      if (at < 0) throw new Problem(404, "no such acknowledgement");
      mem.given.splice(at, 1);
      log(me, "delete", `acks/${id}`, "took back an acknowledgement");
    },
    changes: async () => {
      failing("changes");
      const me = signedIn();
      const steward = me.roles.includes("steward");
      return mem.audit
        .filter((row) => steward || row.actorId === me.id)
        .reverse()
        .map(({ actorId: _, ...row }) => row);
    },
  };
  return mem;
}

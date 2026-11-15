/**
 * The App's server in memory (T-3828), deciding as the real one does: follows are the signed-in
 * reader's own, only a steward names topics, and the history shows a steward everyone's changes
 * and anyone else their own.
 */
import { Problem } from "../server";
import type { Change, Follow, Kept, Me, Server, TopicName } from "../server";

export const VIEWER: Me = { id: "u-viewer", name: "Vera Viewer", roles: ["viewer"] };
export const STEWARD: Me = { id: "u-steward", name: "Sara Steward", roles: ["steward", "viewer"] };

export interface Memory extends Server {
  /** Who calls; `null` is signed out. */
  who: Me | null;
  kept: Kept;
  followed: Array<Follow & { owner: string }>;
  given: TopicName[];
  audit: Array<Change & { actorId: string }>;
  /** Names a method that fails with this text, as a refused or broken call would. */
  fail: Partial<Record<keyof Server, string>>;
}

export function memory(who: Me | null = null): Memory {
  let next = 1;
  const signedIn = () => {
    if (!mem.who) throw new Problem(401, "sign in first");
    return mem.who;
  };
  const steward = () => {
    const me = signedIn();
    if (!me.roles.includes("steward")) throw new Problem(403, "a steward names topics");
    return me;
  };
  const failing = (name: keyof Server) => {
    if (mem.fail[name]) throw new Problem(503, mem.fail[name]!);
  };
  const log = (actor: Me, action: Change["action"], target: string, summary: string) =>
    mem.audit.push({ id: next++, at: "2026-10-12T09:00:00Z", actorId: actor.id, actorName: actor.name, action, target, summary });
  const mem: Memory = {
    who,
    kept: { weeks: [], stale: false },
    followed: [],
    given: [],
    audit: [],
    fail: {},
    weeks: async () => {
      failing("weeks");
      return mem.kept;
    },
    corpusUrl: async (week) => {
      failing("corpusUrl");
      return `https://store.test/corpus/${week}.json`;
    },
    me: async () => {
      failing("me");
      return mem.who;
    },
    follows: async () => {
      failing("follows");
      const me = signedIn();
      return mem.followed.filter((follow) => follow.owner === me.id).reverse().map(({ id, term }) => ({ id, term }));
    },
    follow: async (term) => {
      failing("follow");
      const me = signedIn();
      if (mem.followed.some((follow) => follow.owner === me.id && follow.term === term)) throw new Problem(409, "this keyword is followed already");
      const follow = { id: next++, term };
      mem.followed.push({ ...follow, owner: me.id });
      log(me, "insert", `follows/${follow.id}`, `followed ${term}`);
      return follow;
    },
    unfollow: async (id) => {
      failing("unfollow");
      const me = signedIn();
      const at = mem.followed.findIndex((follow) => follow.id === id && follow.owner === me.id);
      if (at < 0) throw new Problem(404, "no such follow");
      mem.followed.splice(at, 1);
      log(me, "delete", `follows/${id}`, "stopped following a keyword");
    },
    names: async () => {
      failing("names");
      return [...mem.given].sort((a, b) => a.term.localeCompare(b.term));
    },
    name: async (term, name) => {
      failing("name");
      const me = steward();
      const before = mem.given.find((entry) => entry.term === term);
      const entry = { id: before?.id ?? next++, term, name, ownerName: me.name };
      mem.given = [...mem.given.filter((other) => other.term !== term), entry];
      log(me, before ? "update" : "insert", `topic_names/${entry.id}`, `named the topic of ${term} ${name}`);
      return entry;
    },
    unname: async (id) => {
      failing("unname");
      const me = steward();
      const at = mem.given.findIndex((entry) => entry.id === id);
      if (at < 0) throw new Problem(404, "no such name");
      mem.given.splice(at, 1);
      log(me, "delete", `topic_names/${id}`, "took back a topic's name");
    },
    changes: async () => {
      failing("changes");
      const me = signedIn();
      const all = me.roles.includes("steward");
      return mem.audit
        .filter((row) => all || row.actorId === me.id)
        .reverse()
        .map(({ actorId: _, ...row }) => row);
    },
  };
  return mem;
}

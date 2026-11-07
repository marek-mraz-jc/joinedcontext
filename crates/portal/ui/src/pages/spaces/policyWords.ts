/**
 * A space's Policies read at a glance: who holds the grant in words and what it lets them do as a
 * handful of kinds (read, history, write, delete, subscriptions, types), not a wall of CIM 009
 * operation names. The names stay one hover away (the chip's title) for whoever needs them.
 */
import { granteeName } from "../../components/AccessWords";

type T = (key: string, options?: Record<string, unknown>) => string;

export const OPERATION_KINDS = ["read", "history", "write", "delete", "subscribe", "types", "other"] as const;
export type OperationKind = (typeof OPERATION_KINDS)[number];

/** The kinds an operation group stands for (ETSI GS CIM 009 Table 4.20-2). */
const GROUPS: Record<string, OperationKind[]> = {
  retrieveOps: ["read"],
  updateOps: ["write"],
  associationOps: ["read", "types", "subscribe"],
  federationOps: ["read", "types", "subscribe"],
  redirectionOps: ["write", "delete"],
};

/** One operation's kind, by the CIM 009 naming pattern. */
export function operationKind(name: string): OperationKind[] {
  const group = GROUPS[name];
  if (group) return group;
  if (/Subscription/.test(name)) return ["subscribe"];
  if (/Temporal/.test(name)) return ["history"];
  if (/^retrieve(Entity|Attr)Type/.test(name)) return ["types"];
  if (/^delete/.test(name)) return ["delete"];
  if (/^(create|update|upsert|append|replace|merge)/.test(name)) return ["write"];
  if (/^(retrieve|query)/.test(name)) return ["read"];
  return ["other"];
}

/** Each kind the operations reach, in a fixed order, with the operations behind it. */
export function operationKinds(operations: readonly string[]): Array<{ kind: OperationKind; operations: string[] }> {
  const by = new Map<OperationKind, string[]>();
  for (const name of operations) {
    for (const kind of operationKind(name)) by.set(kind, [...(by.get(kind) ?? []), name]);
  }
  return OPERATION_KINDS.filter((kind) => by.has(kind)).map((kind) => ({ kind, operations: by.get(kind) ?? [] }));
}

/** Who holds the grant: an endpoint's callers, everyone, a service account, a role, a person. */
export function assigneeWords(assignee: { kind?: string; id?: string } | undefined, t: T): string {
  const kind = assignee?.kind ?? "";
  const id = assignee?.id ?? "";
  if (!id) return "—";
  if (kind === "role" && id === "public") return t("spaces.inside.policyPublic");
  const named = granteeName(kind, id, t);
  if (named !== id) return named;
  return t(`policies.sentence.who.${kind}`, { id, defaultValue: `${kind} ${id}`.trim() });
}
